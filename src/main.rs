#![cfg_attr(not(test), windows_subsystem = "windows")]
// `cargo test` needs a console test harness with visible output; release
// builds stay windowless (see attribute above).

mod about;
mod app;
mod config;
mod constants;
mod hotkeys;
mod singleinstance;
mod toast;
mod tray;

use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// Runs the DPI-awareness selftest: proves the link-time PerMonitorV2
/// manifest is active and the process is not DPI-virtualized.
///
/// Must run before any window is created. The binary is a GUI-subsystem
/// app with no console, so the process exit code is the result channel;
/// a human-readable result is also emitted via `OutputDebugStringW`.
fn verify_dpi_and_exit() -> ! {
    let active = unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
    }
    .as_bool();

    let message = if active {
        "Corvus Capture: DPI selftest PASSED (PerMonitorV2 active)\0"
    } else {
        "Corvus Capture: DPI selftest FAILED (not PerMonitorV2)\0"
    };
    let wide: Vec<u16> = message.encode_utf16().collect();
    unsafe { OutputDebugStringW(windows::core::PCWSTR(wide.as_ptr())) };

    std::process::exit(if active { 0 } else { 2 });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--verify-dpi") {
        verify_dpi_and_exit();
    }

    // Single-instance enforcement (TRAY-04, D-01) must run before any
    // window, tray icon, or hotkey is created, so a rejected second launch
    // produces zero side effects.
    let _instance_guard = match singleinstance::acquire() {
        Some(guard) => guard,
        None => std::process::exit(0),
    };

    // Hidden dev affordance: `--toast-test <text>` proves the toast path
    // end-to-end without waiting on a real hotkey/tray flow.
    if let Some(idx) = args.iter().position(|a| a == "--toast-test") {
        let text = args.get(idx + 1).cloned().unwrap_or_default();
        let _hwnd = app::create_main_window().expect("failed to create main window");
        toast::show(&text);
        toast::run_until_dismissed();
        std::process::exit(0);
    }

    let _hwnd = app::create_main_window().expect("failed to create main window");
    let _tray = tray::init().expect("failed to create tray icon");

    // Registered strictly after the window and tray exist (RESEARCH.md
    // Pitfall 2). Kept alive for the process lifetime -- dropping it
    // unregisters every hotkey.
    let _hotkeys = hotkeys::init().expect("failed to initialize hotkeys");

    app::run_message_loop();
}
