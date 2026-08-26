//! Save pipeline: filename numbering, path building, and the startup
//! orphan-`.tmp` sweep (SAVE-01, SAVE-02, SAVE-03, SAVE-05, ERR-02, D-19).
//!
//! Locked threading contract (RESEARCH.md Pattern 1 / ARCHITECTURE.md): the
//! filename number is reserved synchronously on the UI thread, immediately
//! after capture and before any job leaves the UI thread -- SAVE-03's race
//! safety depends on this. Only encoding and the disk write (added in a
//! later task in this same file) ever run on the background worker thread;
//! the UI thread's message pump is never blocked by either.

use std::path::{Path, PathBuf};

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
}
