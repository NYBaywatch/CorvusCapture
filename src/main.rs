#![cfg_attr(not(test), windows_subsystem = "windows")]
// `cargo test` needs a console test harness with visible output; release
// builds stay windowless (see attribute above).

mod about;
mod app;
mod capture;
mod clipboard;
mod config;
mod constants;
mod hotkeys;
mod overlay;
mod save;
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

/// Dev-only: exercises the whole capture -> clipboard path (grab the
/// monitor under the cursor, verify the buffer shape, copy it as CF_DIB) and
/// exits with a process code, since the binary is a GUI-subsystem app with
/// no console. `OutputDebugStringW` carries the human-readable result.
fn capture_selftest_and_exit() -> ! {
    let _hwnd = app::create_main_window().expect("failed to create main window");

    let outcome: Result<(i32, i32), String> = (|| {
        let rect = capture::monitor_under_cursor().map_err(|e| e.to_string())?;
        let bitmap = capture::grab(rect).map_err(|e| e.to_string())?;

        let expected_len = (bitmap.width as usize) * (bitmap.height as usize) * 4;
        if bitmap.width <= 0 || bitmap.height <= 0 || bitmap.bgra.len() != expected_len {
            return Err(format!(
                "buffer-length assertion failed: {}x{} -> {} bytes (expected {})",
                bitmap.width,
                bitmap.height,
                bitmap.bgra.len(),
                expected_len
            ));
        }

        clipboard::copy_dib(&bitmap).map_err(|e| e.to_string())?;

        Ok((bitmap.width, bitmap.height))
    })();

    let (message, code) = match outcome {
        Ok((width, height)) => (
            format!("Corvus Capture: --capture-selftest PASSED ({width}x{height})\0"),
            0,
        ),
        Err(e) => (
            format!("Corvus Capture: --capture-selftest FAILED: {e}\0"),
            3,
        ),
    };
    let wide: Vec<u16> = message.encode_utf16().collect();
    unsafe { OutputDebugStringW(windows::core::PCWSTR(wide.as_ptr())) };
    std::process::exit(code);
}

/// Dev-only: exercises `overlay::open()` end-to-end and asserts the REG-01
/// sub-50 ms budget (Phase 3 Plan 02 Task 3). Registers the overlay class,
/// opens it, reads the timing recorded by `open()`, immediately closes it,
/// pumps until torn down, then reports PASSED/FAILED with a non-zero exit
/// code on failure so the check is scriptable. Runs before any tray/hotkey
/// init, mirroring `capture_selftest_and_exit`'s shape.
fn overlay_selftest_and_exit() -> ! {
    let _hwnd = app::create_main_window().expect("failed to create main window");

    overlay::open();
    let elapsed_ms = overlay::last_open_ms();
    let dims = overlay::frozen_dims();
    overlay::close_active();
    overlay::run_until_closed();

    let (message, code) = match (elapsed_ms, dims) {
        (Some(ms), Some((w, h))) if ms < 50.0 => (
            format!("Corvus Capture: --overlay-selftest PASSED ({ms:.1} ms, {w}x{h})\0"),
            0,
        ),
        (Some(ms), Some((w, h))) => (
            format!(
                "Corvus Capture: --overlay-selftest FAILED ({ms:.1} ms exceeds 50 ms budget, {w}x{h})\0"
            ),
            3,
        ),
        _ => (
            "Corvus Capture: --overlay-selftest FAILED: overlay did not open\0".to_string(),
            3,
        ),
    };
    let wide: Vec<u16> = message.encode_utf16().collect();
    unsafe { OutputDebugStringW(windows::core::PCWSTR(wide.as_ptr())) };
    std::process::exit(code);
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

    // Hidden dev affordance: `--capture-selftest [out_path]` proves the
    // whole grab -> clipboard path from the command line, with no hotkey
    // involved. Not a shipped feature -- exists purely to make Phase 2
    // implementation and CI verifiable without pressing F9. Runs before any
    // tray/hotkey init so it never registers a hotkey.
    if args.iter().any(|a| a == "--capture-selftest") {
        capture_selftest_and_exit();
    }

    // Hidden dev affordance: `--overlay-selftest` proves the REG-01 <50 ms
    // overlay-appearance budget from the command line. Not a shipped
    // feature. Runs before any tray/hotkey init so it never registers a
    // hotkey and cannot collide with a real Shift+F9 press.
    if args.iter().any(|a| a == "--overlay-selftest") {
        overlay_selftest_and_exit();
    }

    let _hwnd = app::create_main_window().expect("failed to create main window");
    let _tray = tray::init().expect("failed to create tray icon");

    // Phase 2 startup: the orphan `.tmp` sweep must finish and the save
    // worker thread must exist before hotkeys are registered -- hotkey
    // registration is the last gate before a capture can fire, and D-19
    // requires no stale `.tmp` file to be visible to the very first
    // capture. This config load is for the sweep only -- D-11 requires
    // config to be re-read on every capture press, so `app::dispatch` must
    // not reuse this snapshot.
    let cfg = config::load();
    save::sweep_orphan_tmp(
        &config::resolve_save_folder(&cfg),
        &config::sanitize_base_filename(&cfg.base_filename),
    );
    let _saver = save::init();

    // Registered strictly after the window and tray exist (RESEARCH.md
    // Pitfall 2). Kept alive for the process lifetime -- dropping it
    // unregisters every hotkey.
    let _hotkeys = hotkeys::init().expect("failed to initialize hotkeys");

    app::run_message_loop();
}
