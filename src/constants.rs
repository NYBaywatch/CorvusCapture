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

/// Posted by the save worker thread when an encode+write job finishes.
pub const WM_APP_SAVE_DONE: u32 = WM_APP + 4;

// ---------------------------------------------------------------------
// Toast tuning (consumed by plan 01-03)
// ---------------------------------------------------------------------

/// How long a toast stays visible before auto-dismissing (D-03).
pub const TOAST_DURATION_MS: u32 = 2500;

/// Toast window width in pixels (96-DPI logical, scaled at use time).
pub const TOAST_WIDTH: i32 = 420;

/// Toast window height in pixels (96-DPI logical, scaled at use time).
pub const TOAST_HEIGHT: i32 = 80;

/// Margin from the work-area edge for toast placement.
pub const TOAST_MARGIN: i32 = 16;

/// Toast text color: green (UAT UI-06), packed via `rgb()`.
pub const TOAST_TEXT_COLOR: u32 = rgb(100, 220, 80);

/// `SetLayeredWindowAttributes` alpha for the toast window -- lower than the
/// prior 230 for a more visibly translucent background (UAT UI-06).
pub const TOAST_ALPHA: u8 = 195;

/// Toast font character height in 96-DPI logical pixels, scaled by monitor
/// DPI at paint time (UAT UI-06 "visibly larger").
pub const TOAST_FONT_HEIGHT_LOGICAL: i32 = 28;

/// Distance from the work-area top edge to the toast's top edge, in 96-DPI
/// logical pixels (48px at 96dpi = 0.5in), scaled by monitor DPI at
/// placement time (UAT UI-06 "top-center ~0.5in down").
pub const TOAST_TOP_OFFSET_LOGICAL: i32 = 48;

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
// Config store (D-11/D-13)
// ---------------------------------------------------------------------

/// App-data subfolder name; resolves to `%APPDATA%\CorvusCapture\`.
pub const APP_DATA_DIR_NAME: &str = "CorvusCapture";

/// Config file name within `APP_DATA_DIR_NAME`, per SET-03/D-11.
pub const CONFIG_FILE_NAME: &str = "config.json";

// ---------------------------------------------------------------------
// Save pipeline
// ---------------------------------------------------------------------

/// Appended to the FULL final filename (e.g. `corvus004.png.tmp`) via
/// string concatenation -- never via `Path::with_extension`, which would
/// silently replace the real extension instead of appending to it.
pub const TMP_SUFFIX: &str = ".tmp";

/// Extensions this app writes; used by the D-19 orphan `.tmp` sweep to
/// match only our own files, never an unrelated process's temp file.
pub const CAPTURE_EXTENSIONS: [&str; 4] = ["png", "jpg", "bmp", "webp"];

// ---------------------------------------------------------------------
// Overlay (Phase 3)
// ---------------------------------------------------------------------

/// Builds a COLORREF from RGB components. COLORREF byte order is
/// `0x00BBGGRR` (RESEARCH Pitfall 5 -- easy to get backwards).
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (b as u32) << 16 | (g as u32) << 8 | r as u32
}

/// Window class name for the Shift+F9 region-selection overlay.
///
/// Not yet consumed: window creation arrives in Phase 3 Plan 02.
#[allow(dead_code)]
pub const OVERLAY_WINDOW_CLASS: &str = "CorvusCaptureOverlayWnd";

/// Matrix green `#00FF41` (D-24): selection border, resize handles, and
/// readout chip text -- nowhere else.
///
/// Not yet consumed: paint code arrives in Phase 3 Plan 02/03.
#[allow(dead_code)]
pub const OVERLAY_ACCENT: u32 = rgb(0x00, 0xFF, 0x41);

/// Readout chip fill (D-29), matches the existing toast fill exactly.
///
/// Not yet consumed: paint code arrives in Phase 3 Plan 02/03.
#[allow(dead_code)]
pub const OVERLAY_CHIP_FILL: u32 = 0x0020_2020;

/// `AlphaBlend` `SourceConstantAlpha` for the frozen-frame dim (REG-02).
/// ~40% dim; UI-SPEC permits 96-112 discretion, shipped value recorded here.
///
/// Not yet consumed: paint code arrives in Phase 3 Plan 02/03.
#[allow(dead_code)]
pub const OVERLAY_DIM_ALPHA: u8 = 102;

/// Selection border stroke width in physical pixels (D-23).
///
/// Not yet consumed: paint code arrives in Phase 3 Plan 02/03.
#[allow(dead_code)]
pub const OVERLAY_BORDER_WIDTH: i32 = 2;

/// Visual size (in pixels) of each of the 8 resize-handle squares (D-25).
///
/// Not yet consumed outside tests: call sites arrive once window/paint code
/// exists in Phase 3 Plan 02/03.
#[allow(dead_code)]
pub const OVERLAY_HANDLE_SIZE: i32 = 8;

/// Hit-test target size for a resize handle -- inflated beyond the visual
/// square so handles are easy to grab (discretion).
///
/// Not yet consumed outside tests: call sites arrive once window/paint code
/// exists in Phase 3 Plan 02/03.
#[allow(dead_code)]
pub const OVERLAY_HANDLE_HIT_SIZE: i32 = 16;

/// Gap in pixels between the selection's bottom-right corner and the
/// dimension readout chip (D-27).
///
/// Not yet consumed: `overlay::readout_rect` (Task 2 of this plan) is the
/// first call site.
#[allow(dead_code)]
pub const OVERLAY_READOUT_GAP: i32 = 8;

/// Horizontal padding inside the readout chip, per side (UI-SPEC typography).
///
/// Not yet consumed: paint code arrives in Phase 3 Plan 02/03.
#[allow(dead_code)]
pub const OVERLAY_READOUT_PAD_X: i32 = 8;

/// Vertical padding inside the readout chip, per side (UI-SPEC typography).
///
/// Not yet consumed: paint code arrives in Phase 3 Plan 02/03.
#[allow(dead_code)]
pub const OVERLAY_READOUT_PAD_Y: i32 = 4;

/// Distance in pixels from a client edge within which the readout chip's
/// default placement flips to avoid clipping (D-27 corner flip).
///
/// Not yet consumed: `overlay::readout_rect` (Task 2 of this plan) is the
/// first call site.
#[allow(dead_code)]
pub const OVERLAY_READOUT_EDGE_THRESHOLD: i32 = 24;

/// Readout chip corner radius; `RoundRect` ellipse is 2x this (UI-SPEC).
///
/// Not yet consumed: paint code arrives in Phase 3 Plan 02/03.
#[allow(dead_code)]
pub const OVERLAY_CHIP_RADIUS: i32 = 6;

/// Readout chip font height in pixels, matches the existing toast (UI-SPEC).
///
/// Not yet consumed: paint code arrives in Phase 3 Plan 02/03.
#[allow(dead_code)]
pub const OVERLAY_FONT_HEIGHT: i32 = 18;

// ---------------------------------------------------------------------
// Settings window (Phase 4)
// ---------------------------------------------------------------------

/// Window class name for the raw Win32 Settings window.
///
/// Not yet consumed: window creation arrives in a later Phase 4 plan.
#[allow(dead_code)]
pub const SETTINGS_WINDOW_CLASS: &str = "CorvusCaptureSettingsWnd";

/// Settings window title bar text.
///
/// Not yet consumed: window creation arrives in a later Phase 4 plan.
#[allow(dead_code)]
pub const SETTINGS_WINDOW_TITLE: &str = "Corvus Capture Settings";

/// Fixed client width in logical (96-DPI) pixels (UI-SPEC Layout Contract).
///
/// Not yet consumed: layout code arrives in a later Phase 4 plan.
#[allow(dead_code)]
pub const SETTINGS_CLIENT_W: i32 = 400;

/// Fixed client height in logical (96-DPI) pixels (UI-SPEC Layout Contract).
///
/// Not yet consumed: layout code arrives in a later Phase 4 plan.
#[allow(dead_code)]
pub const SETTINGS_CLIENT_H: i32 = 528;

/// Inline validation/error hint auto-dismiss duration (D-45/D-46/D-48/D-52).
///
/// Not yet consumed: hint mechanism arrives in a later Phase 4 plan.
#[allow(dead_code)]
pub const SETTINGS_HINT_MS: u32 = 2500;

/// Base timer id for Settings hint auto-dismiss timers. Offset well clear of
/// `TOAST_TIMER_ID` so the two windows' `SetTimer` ids can never collide.
///
/// Not yet consumed: hint mechanism arrives in a later Phase 4 plan.
#[allow(dead_code)]
pub const SETTINGS_HINT_TIMER_BASE: usize = 100;

/// HKCU `...\Run` value name written for Start with Windows (D-50/D-51).
///
/// Not yet consumed: registry module arrives in a later Phase 4 plan.
#[allow(dead_code)]
pub const RUN_VALUE_NAME: &str = "CorvusCapture";

// Control IDs, starting at 100 (never 1/2 -- those are IDOK/IDCANCEL,
// delivered by IsDialogMessageW). One per interactive/addressable control
// in the UI-SPEC layout table.
//
// Not yet consumed: window creation arrives in a later Phase 4 plan.
#[allow(dead_code)]
pub const ID_BASE_EDIT: i32 = 100;
#[allow(dead_code)]
pub const ID_BASE_HINT: i32 = 101;
#[allow(dead_code)]
pub const ID_FOLDER_EDIT: i32 = 102;
#[allow(dead_code)]
pub const ID_FOLDER_HINT: i32 = 103;
#[allow(dead_code)]
pub const ID_BROWSE_BTN: i32 = 104;
#[allow(dead_code)]
pub const ID_PREVIEW: i32 = 105;
#[allow(dead_code)]
pub const ID_FORMAT_COMBO: i32 = 106;
#[allow(dead_code)]
pub const ID_QUALITY_LABEL: i32 = 107;
#[allow(dead_code)]
pub const ID_QUALITY_SLIDER: i32 = 108;
#[allow(dead_code)]
pub const ID_QUALITY_VALUE: i32 = 109;
#[allow(dead_code)]
pub const ID_TOAST_CHECK: i32 = 110;
#[allow(dead_code)]
pub const ID_CLICK_ACTION_COMBO: i32 = 111;
#[allow(dead_code)]
pub const ID_CLIPBOARD_CHECK: i32 = 112;
#[allow(dead_code)]
pub const ID_STARTUP_CHECK: i32 = 113;
#[allow(dead_code)]
pub const ID_STARTUP_HINT: i32 = 114;
pub const ID_SHUTTER_CHECK: i32 = 115;
pub const ID_DONE_BTN: i32 = 116;

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// Converts a Rust string into a NUL-terminated UTF-16 buffer suitable for
/// Win32 `PCWSTR` arguments.
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_packs_colorref_byte_order() {
        // COLORREF is 0x00BBGGRR (RESEARCH Pitfall 5) -- matrix green
        // #00FF41 must pack to 0x0041FF00, not 0x0000FF41.
        assert_eq!(rgb(0x00, 0xFF, 0x41), 0x0041_FF00);
    }
}
