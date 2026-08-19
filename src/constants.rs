//! Shared, app-wide identifiers and small helpers.
//!
//! Every later plan/phase imports from here instead of duplicating literals
//! (window class names, message ids, toast tuning, the default capture
//! folder). See PLAN 01-02 / D-08.

use windows::Win32::UI::WindowsAndMessaging::WM_APP;

// ---------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------

/// Display name of the application (About box, toasts, window titles).
pub const APP_NAME: &str = "Corvus Capture";

/// Cargo package version, embedded at compile time.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Single source of truth for the project's GitHub URL (About box).
pub const GITHUB_URL: &str = "https://github.com/jfago/corvus-capture";

// ---------------------------------------------------------------------
// Win32 names
// ---------------------------------------------------------------------

/// Window class name for the hidden main hub window.
pub const MAIN_WINDOW_CLASS: &str = "CorvusCaptureMainWnd";

/// Window class name for the toast notification window.
pub const TOAST_WINDOW_CLASS: &str = "CorvusCaptureToastWnd";

/// Named mutex used to enforce single-instance behavior.
pub const SINGLE_INSTANCE_MUTEX: &str = "Local\\CorvusCapture_SingleInstance";

/// Name registered via `RegisterWindowMessageW` so a second launch can
/// notify the already-running instance across processes (plan 01-03).
pub const ALREADY_RUNNING_MSG_NAME: &str = "CorvusCapture_AlreadyRunning_v1";

// ---------------------------------------------------------------------
// WM_APP-relative message ids
// ---------------------------------------------------------------------
// Defined relative to WM_APP so they never collide with system messages.

/// Posted when a registered global hotkey fires.
pub const WM_APP_HOTKEY: u32 = WM_APP + 1;

/// Posted when a tray context-menu item is selected.
pub const WM_APP_MENU: u32 = WM_APP + 2;

/// Posted for tray icon events (e.g. left-click).
pub const WM_APP_TRAY: u32 = WM_APP + 3;

// ---------------------------------------------------------------------
// Toast tuning (consumed by plan 01-03)
// ---------------------------------------------------------------------

/// How long a toast stays visible before auto-dismissing (D-03).
pub const TOAST_DURATION_MS: u32 = 2500;

/// Toast window width in pixels.
pub const TOAST_WIDTH: i32 = 320;

/// Toast window height in pixels.
pub const TOAST_HEIGHT: i32 = 72;

/// Margin from the work-area edge for toast placement.
pub const TOAST_MARGIN: i32 = 16;

/// Timer id used for the toast auto-dismiss `SetTimer` call.
pub const TOAST_TIMER_ID: usize = 1;

// ---------------------------------------------------------------------
// Capture folder (D-08)
// ---------------------------------------------------------------------

/// Default capture folder: `%USERPROFILE%\Pictures\CorvusCapture`.
///
/// Not created here — creation happens on first use.
pub fn default_capture_dir() -> std::path::PathBuf {
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    std::path::PathBuf::from(profile)
        .join("Pictures")
        .join("CorvusCapture")
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// Converts a Rust string into a NUL-terminated UTF-16 buffer suitable for
/// Win32 `PCWSTR` arguments.
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
