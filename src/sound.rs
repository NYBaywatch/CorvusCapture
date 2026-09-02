//! Best-effort capture-feedback shutter sound (UI-05).
//!
//! `play_shutter()` is a fire-and-forget side effect, mirroring `toast.rs`'s
//! `open_file` discipline: one `unsafe` FFI call, result discarded with
//! `let _ =`, never panics, no `Result` propagated to the caller.

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

/// Synthetically generated (`tools/make_shutter.py`) camera-shutter click,
/// embedded at build time -- never sourced from a runtime/user-writable
/// path.
static SHUTTER_WAV: &[u8] = include_bytes!("../resources/shutter.wav");

/// Plays the embedded shutter-click WAV asynchronously. Best-effort: a
/// failure to play is silently ignored (`SND_NODEFAULT` also suppresses any
/// fallback system sound).
pub fn play_shutter() {
    unsafe {
        let _ = PlaySoundW(
            PCWSTR(SHUTTER_WAV.as_ptr() as *const u16),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
}
