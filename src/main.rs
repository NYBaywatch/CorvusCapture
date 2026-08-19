#![windows_subsystem = "windows"]

mod app;
mod constants;

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
    if std::env::args().any(|a| a == "--verify-dpi") {
        verify_dpi_and_exit();
    }

    let _hwnd = app::create_main_window().expect("failed to create main window");

    // Insertion point: plans 01-03 through 01-05 add single-instance,
    // toast, tray, and hotkey setup here, strictly after the window is
    // created above (RESEARCH.md Pitfall 2 — crate event handlers must
    // not PostMessage to main_hwnd before it exists).

    app::run_message_loop();
}
