//! Save pipeline: filename numbering, path building, and the startup
//! orphan-`.tmp` sweep (SAVE-01, SAVE-02, SAVE-03, SAVE-05, ERR-02, D-19).
//!
//! Locked threading contract (RESEARCH.md Pattern 1 / ARCHITECTURE.md): the
//! filename number is reserved synchronously on the UI thread, immediately
//! after capture and before any job leaves the UI thread -- SAVE-03's race
//! safety depends on this. Only encoding and the disk write (added in a
//! later task in this same file) ever run on the background worker thread;
//! the UI thread's message pump is never blocked by either.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

use image::codecs::bmp::BmpEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::app;
use crate::capture::RawBitmap;
use crate::config::{self, Config, Format};
use crate::constants;

// ---------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------

/// Builds `{base}{n:03}.{ext}`. `{:03}` zero-pads to three digits but never
/// truncates past 999 -- SAVE-01's `corvus1000.png` example.
fn filename_for(base: &str, n: u32, ext: &str) -> String {
    format!("{}{:03}.{}", base, n, ext)
}

/// Appends the tmp suffix onto the FULL final filename via string
/// concatenation -- never the `PathBuf` extension-replacing helper, which
/// would silently replace the real extension (`corvus004.png` ->
/// `corvus004.tmp`) instead of appending to it (RESEARCH.md
/// tmp-extension-collision pitfall).
fn tmp_name_for(final_name: &str) -> String {
    format!("{final_name}{}", constants::TMP_SUFFIX)
}

/// D-19 guard: true only when `file_name` ends with `TMP_SUFFIX` AND the
/// segment before `.tmp` both starts with `base` and ends in one of our own
/// capture extensions. This is what prevents the orphan sweep from ever
/// deleting another process's `.tmp` file or a bare `*.tmp` (T-02-10).
fn is_our_tmp(file_name: &str, base: &str) -> bool {
    let Some(stripped) = file_name.strip_suffix(constants::TMP_SUFFIX) else {
        return false;
    };
    if !stripped.starts_with(base) {
        return false;
    }
    constants::CAPTURE_EXTENSIONS
        .iter()
        .any(|ext| stripped.ends_with(&format!(".{ext}")))
}

// ---------------------------------------------------------------------
// Counter: synchronous, race-free filename number reservation
// ---------------------------------------------------------------------

/// Owns the in-memory "highest number seen" state for one (dir, base) pair.
/// Rescans the directory only at construction and whenever `dir`/`base`
/// change (D-11 re-reads config every press, so either may change between
/// captures) -- never on every capture (RESEARCH.md Pitfall 8 /
/// ARCHITECTURE.md Anti-Pattern 5).
#[derive(Default)]
struct Counter {
    dir: PathBuf,
    base: String,
    highest_seen: u32,
}

impl Counter {
    /// Rescans `dir` for files matching `{base}\d+.{ext}` for any configured
    /// extension, setting `highest_seen` to the max trailing number found
    /// (or 0 if none/the directory doesn't exist yet).
    fn rescan(&mut self) {
        self.highest_seen = 0;
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(n) = Self::parse_number(name, &self.base) {
                if n > self.highest_seen {
                    self.highest_seen = n;
                }
            }
        }
    }

    /// Parses the trailing digit run out of `{base}{digits}.{ext}` for any
    /// of our known capture extensions. Returns `None` for anything else
    /// (no digits, wrong base, unknown extension).
    fn parse_number(file_name: &str, base: &str) -> Option<u32> {
        let rest = file_name.strip_prefix(base)?;
        for ext in constants::CAPTURE_EXTENSIONS {
            let suffix = format!(".{ext}");
            if let Some(digits) = rest.strip_suffix(&suffix) {
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                    return digits.parse::<u32>().ok();
                }
            }
        }
        None
    }

    /// Points the counter at `dir`/`base`, rescanning only if either changed
    /// since the last call (D-11: config -- and therefore the folder/base --
    /// may change on any press).
    fn set_target(&mut self, dir: &Path, base: &str) {
        if self.dir != dir || self.base != base {
            self.dir = dir.to_path_buf();
            self.base = base.to_string();
            self.rescan();
        }
    }

    /// Increments and probes `Path::exists()` in a loop until a free number
    /// is found. Never reuses a deleted/skipped number (SAVE-02) and never
    /// rescans the directory here (Pitfall 8) -- `set_target` is the only
    /// rescan trigger.
    fn next(&mut self, ext: &str) -> u32 {
        loop {
            self.highest_seen += 1;
            let candidate = self.dir.join(filename_for(&self.base, self.highest_seen, ext));
            if !candidate.exists() {
                return self.highest_seen;
            }
        }
    }
}

// ---------------------------------------------------------------------
// Startup orphan .tmp sweep (D-19)
// ---------------------------------------------------------------------

/// Deletes only our own orphaned `.tmp` files (crash mid-write) from `dir`.
/// Every error is ignored (`let _ =`) -- a sweep failure must never block
/// startup -- and no toast is shown (UI-SPEC: the sweep is silent). Never
/// deletes a bare `*.tmp` file belonging to another process (D-19, T-02-10).
pub fn sweep_orphan_tmp(dir: &Path, base: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if is_our_tmp(name, base) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

// ---------------------------------------------------------------------
// Worker thread: encode + atomic write (SAVE-04, SAVE-05, SAVE-06, ERR-02)
// ---------------------------------------------------------------------

/// A reserved capture ready to be encoded and written by the worker thread.
/// The number/path in `final_path` was already reserved synchronously on
/// the UI thread before this job was sent (SAVE-03).
pub struct SaveJob {
    bitmap: RawBitmap,
    final_path: PathBuf,
    format: Format,
    jpg_quality: u8,
    advisory: Option<String>,
}

/// Result of one save job, posted back to the UI thread for the wnd_proc
/// arm (plan 02-04) to drain via `take_result`.
pub struct SaveOutcome {
    pub file_name: String,
    pub path: PathBuf,
    pub error: Option<String>,
    pub advisory: Option<String>,
}

/// Process-global handle to the worker's job sender, set once by `init`.
/// Lets `reserve_and_dispatch` reach the worker without threading a handle
/// through every call (same shape as `app::MAIN_HWND` / `hotkeys::HOTKEY_MAP`).
static SAVE_TX: OnceLock<Sender<SaveJob>> = OnceLock::new();

/// Completed outcomes waiting to be drained by the UI thread's wnd_proc,
/// mirroring `toast.rs`'s `Mutex<String>` payload-alongside-message pattern.
static SAVE_RESULTS: Mutex<VecDeque<SaveOutcome>> = Mutex::new(VecDeque::new());

/// Single UI-thread-owned counter, guarded for interior mutability from the
/// synchronous `reserve_and_dispatch` call path.
static COUNTER: Mutex<Option<Counter>> = Mutex::new(None);

/// Spawns the long-lived save worker thread and stores its job sender in
/// `SAVE_TX`, following the `hotkeys::init()` guard convention. The
/// returned `Sender` is bound to a kept-alive local in `main.rs` (`_saver`)
/// for symmetry with `_tray`/`_hotkeys`, even though the worker thread's
/// lifetime is independent of it.
///
/// # Panics
/// Panics if called more than once.
pub fn init() -> Sender<SaveJob> {
    let (tx, rx) = mpsc::channel::<SaveJob>();

    std::thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            worker_process(job);
        }
    });

    SAVE_TX
        .set(tx.clone())
        .unwrap_or_else(|_| panic!("save::init() called more than once"));

    tx
}

/// Runs on the worker thread: encodes + writes one job, then posts
/// `WM_APP_SAVE_DONE` back to the UI thread. `wparam` carries no pointer and
/// no meaning (T-01-12/T-02-13 rule) -- the outcome itself travels through
/// `SAVE_RESULTS`, a process-internal queue only our own wnd_proc drains.
fn worker_process(job: SaveJob) {
    let final_path = job.final_path.clone();
    let advisory = job.advisory.clone();

    let outcome = match encode_and_write(job) {
        Ok(file_name) => SaveOutcome {
            file_name,
            path: final_path,
            error: None,
            advisory,
        },
        Err(e) => {
            let file_name = final_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            SaveOutcome {
                file_name,
                path: final_path,
                error: Some(e),
                advisory,
            }
        }
    };

    SAVE_RESULTS.lock().unwrap().push_back(outcome);
    unsafe {
        let _ = PostMessageW(
            Some(app::main_hwnd()),
            constants::WM_APP_SAVE_DONE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// Encodes `job.bitmap` into `job.format` and writes it atomically:
/// encode into memory, write to a `.tmp` file, then `std::fs::rename` to the
/// final path. Uses plain `std::fs::rename` -- never `MoveFileExW` with a
/// replace-existing flag, because Windows' `rename` already refuses an
/// existing destination, which is exactly SAVE-02's never-overwrite
/// guarantee for free. On any failure, the tmp file is deleted best-effort
/// and the `std::io::Error` Display string is returned verbatim (ERR-02).
fn encode_and_write(job: SaveJob) -> Result<String, String> {
    let SaveJob {
        bitmap,
        final_path,
        format,
        jpg_quality,
        ..
    } = job;

    let width = bitmap.width as u32;
    let height = bitmap.height as u32;

    // BGRA -> RGBA channel swap (bytes 0 and 2 of every 4-byte pixel) --
    // this MUST happen before any `image` encoder call. This is the single
    // most likely correctness bug in the phase (RESEARCH.md assumption A3).
    // The alpha byte from GDI capture is undefined (BitBlt/PrintWindow into
    // a 32bpp DIB commonly leaves it 0x00), so it is forced opaque here --
    // otherwise PNG/WebP/BMP output can render fully transparent.
    let mut rgba = bitmap.bgra;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 0xFF; // GDI capture alpha is undefined; screenshots are opaque
    }

    let mut buf: Vec<u8> = Vec::new();
    let encode_result: image::ImageResult<()> = match format {
        Format::Png => {
            // D-10: CompressionType::Fast, never Best/zopfli -- the 100 ms
            // budget depends on it.
            let encoder =
                PngEncoder::new_with_quality(&mut buf, CompressionType::Fast, FilterType::NoFilter);
            encoder.write_image(&rgba, width, height, ExtendedColorType::Rgba8)
        }
        Format::Jpg => {
            // JPEG has no alpha channel -- convert to RGB8 first.
            let rgb: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            let encoder = JpegEncoder::new_with_quality(&mut buf, jpg_quality);
            encoder.write_image(&rgb, width, height, ExtendedColorType::Rgb8)
        }
        Format::Bmp => {
            let encoder = BmpEncoder::new(&mut buf);
            encoder.write_image(&rgba, width, height, ExtendedColorType::Rgba8)
        }
        Format::Webp => {
            // SAVE-06 requires lossless only -- the only constructor the
            // crate offers anyway.
            let encoder = WebPEncoder::new_lossless(&mut buf);
            encoder.write_image(&rgba, width, height, ExtendedColorType::Rgba8)
        }
    };
    encode_result.map_err(|e| e.to_string())?;

    let final_file_name = final_path
        .file_name()
        .ok_or_else(|| "final path has no file name".to_string())?
        .to_string_lossy()
        .into_owned();
    let tmp_file_name = tmp_name_for(&final_file_name);
    let tmp_path = final_path
        .parent()
        .map(|p| p.join(&tmp_file_name))
        .ok_or_else(|| "final path has no parent directory".to_string())?;

    if let Err(e) = std::fs::write(&tmp_path, &buf) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.to_string());
    }

    if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.to_string());
    }

    Ok(final_file_name)
}

/// Drains the oldest pending `SaveOutcome`, if any, for the wnd_proc arm
/// (plan 02-04) to toast/report.
pub fn take_result() -> Option<SaveOutcome> {
    SAVE_RESULTS.lock().unwrap().pop_front()
}

/// Runs on the UI thread: resolves the save folder, reserves the next
/// filename number synchronously (SAVE-03 -- this MUST complete before this
/// function returns, so a second hotkey press can never compute the same
/// number), then hands the job to the worker thread. `advisory` (D-22's
/// multi-monitor text) is carried through into the eventual `SaveOutcome` so
/// it can be appended to the success toast.
pub fn reserve_and_dispatch(
    bitmap: RawBitmap,
    cfg: &Config,
    advisory: Option<String>,
) -> Result<(), String> {
    let dir = config::resolve_save_folder(cfg);
    // Each failure path names its cause so the caller's toast can surface
    // it verbatim (ERR-02: failures are never silent about their cause).
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't create save folder: {e}"))?;

    let base = config::sanitize_base_filename(&cfg.base_filename);
    let format = Format::from_str(&cfg.format);
    let ext = format.ext();

    let n = {
        let mut guard = COUNTER.lock().unwrap();
        let counter = guard.get_or_insert_with(Counter::default);
        counter.set_target(&dir, &base);
        counter.next(ext)
    };

    let final_path = dir.join(filename_for(&base, n, ext));

    let job = SaveJob {
        bitmap,
        final_path,
        format,
        jpg_quality: cfg.jpg_quality,
        advisory,
    };

    let tx = SAVE_TX
        .get()
        .ok_or_else(|| "save worker not initialized".to_string())?;
    tx.send(job)
        .map_err(|_| "save worker channel closed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_ID: AtomicU32 = AtomicU32::new(0);

    /// Throwaway per-test directory under the OS temp folder. `temp_dir()`
    /// is used ONLY here, inside `#[cfg(test)]` -- production code in this
    /// file must never call it (threat T-02-03 / RESEARCH.md V12).
    fn temp_test_dir() -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "corvus_save_test_{}_{}",
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn peek_matches_next_and_is_idempotent() {
        let dir = temp_test_dir();
        std::fs::write(dir.join("corvus001.png"), b"x").unwrap();
        std::fs::write(dir.join("corvus007.png"), b"x").unwrap();
        let mut counter = Counter::default();
        counter.set_target(&dir, "corvus");
        let first_peek = counter.peek("png");
        let second_peek = counter.peek("png");
        assert_eq!(first_peek, 8);
        assert_eq!(second_peek, 8);
        assert_eq!(counter.next("png"), 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn peek_next_filename_on_fresh_dir_returns_corvus001() {
        let dir = temp_test_dir();
        let mut cfg = Config::default();
        cfg.save_folder = dir.to_string_lossy().into_owned();
        assert_eq!(peek_next_filename(&cfg), "corvus001.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filename_for_basic() {
        assert_eq!(filename_for("corvus", 4, "png"), "corvus004.png");
    }

    #[test]
    fn filename_for_padding_grows_past_999() {
        assert_eq!(filename_for("corvus", 1000, "png"), "corvus1000.png");
    }

    #[test]
    fn tmp_name_for_concatenates_never_replaces_extension() {
        assert_eq!(tmp_name_for("corvus004.png"), "corvus004.png.tmp");
    }

    #[test]
    fn is_our_tmp_matches_only_our_own_files() {
        assert!(is_our_tmp("corvus004.png.tmp", "corvus"));
        assert!(!is_our_tmp("corvus004.tmp", "corvus"));
        assert!(!is_our_tmp("otherapp.png.tmp", "corvus"));
        assert!(!is_our_tmp("notes.tmp", "corvus"));
    }

    #[test]
    fn first_reservation_in_empty_dir_returns_one() {
        let dir = temp_test_dir();
        let mut counter = Counter::default();
        counter.set_target(&dir, "corvus");
        assert_eq!(counter.next("png"), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reservation_skips_to_lowest_unused_above_highest() {
        let dir = temp_test_dir();
        std::fs::write(dir.join("corvus001.png"), b"x").unwrap();
        std::fs::write(dir.join("corvus007.png"), b"x").unwrap();
        let mut counter = Counter::default();
        counter.set_target(&dir, "corvus");
        assert_eq!(counter.next("png"), 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleted_number_is_never_reused() {
        let dir = temp_test_dir();
        std::fs::write(dir.join("corvus001.png"), b"x").unwrap();
        std::fs::write(dir.join("corvus007.png"), b"x").unwrap();
        let mut counter = Counter::default();
        counter.set_target(&dir, "corvus");
        assert_eq!(counter.next("png"), 8);
        std::fs::remove_file(dir.join("corvus007.png")).unwrap();
        assert_eq!(counter.next("png"), 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ten_consecutive_reservations_are_distinct() {
        let dir = temp_test_dir();
        let mut counter = Counter::default();
        counter.set_target(&dir, "corvus");
        let mut seen: HashSet<u32> = HashSet::new();
        for _ in 0..10 {
            let n = counter.next("png");
            assert!(seen.insert(n), "duplicate reservation: {n}");
        }
        assert_eq!(seen.len(), 10);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_candidate_path_is_skipped() {
        let dir = temp_test_dir();
        // Pre-create the very next candidate so the reservation must skip it.
        std::fs::write(dir.join("corvus001.png"), b"x").unwrap();
        let mut counter = Counter::default();
        counter.set_target(&dir, "corvus");
        assert_eq!(counter.next("png"), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_deletes_only_our_own_orphans() {
        let dir = temp_test_dir();
        std::fs::write(dir.join("corvus999.png.tmp"), b"x").unwrap();
        std::fs::write(dir.join("notes.tmp"), b"x").unwrap();
        sweep_orphan_tmp(&dir, "corvus");
        assert!(!dir.join("corvus999.png.tmp").exists());
        assert!(dir.join("notes.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Proves the BGRA -> RGBA swap in `encode_and_write`: a synthetic 2x2
    /// bitmap whose top-left pixel is BGRA-order pure red (`(0, 0, 255,
    /// 255)`) must decode back out of the PNG as RGB red (`(255, 0, 0)`),
    /// not blue -- catching exactly the color-swap bug RESEARCH.md flags as
    /// the phase's most likely correctness defect.
    #[test]
    fn png_roundtrip_proves_bgra_to_rgba_swap() {
        use image::GenericImageView;

        // Top-left pixel BGRA(0, 0, 255, 0) = pure red once swapped to RGBA.
        // Its alpha byte is 0 -- exactly what GDI capture commonly produces
        // (undefined alpha) -- and must come out of the PNG as 255 (CR-01).
        let bgra = vec![
            0, 0, 255, 0, // top-left: B=0 G=0 R=255 A=0 (undefined GDI alpha)
            0, 255, 0, 255, // top-right: green
            255, 0, 0, 255, // bottom-left: blue
            255, 255, 255, 255, // bottom-right: white
        ];

        let mut rgba = bgra.clone();
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 0xFF;
        }

        let mut buf = Vec::new();
        let encoder =
            PngEncoder::new_with_quality(&mut buf, CompressionType::Fast, FilterType::NoFilter);
        encoder
            .write_image(&rgba, 2, 2, ExtendedColorType::Rgba8)
            .unwrap();

        let decoded = image::load_from_memory(&buf).unwrap();
        let top_left = decoded.get_pixel(0, 0);
        assert_eq!(top_left.0, [255, 0, 0, 255], "expected opaque red, got {:?} -- BGRA/RGBA swap or alpha-force missing/wrong", top_left.0);
    }
}
