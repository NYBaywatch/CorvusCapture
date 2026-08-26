//! The hidden main hub window: the single window that owns the process's
//! only message pump, and the central `AppAction` dispatcher every later
//! plan/phase routes through.
//!
//! Per RESEARCH.md Pitfall 2, this window must exist and have a valid
//! `HWND` *before* any tray/hotkey crate event handler is wired up.

use std::sync::OnceLock;

use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, CW_USEDEFAULT, MSG, SW_SHOWNORMAL, WM_DESTROY, WNDCLASSW,
    WS_OVERLAPPED,
};

use crate::about;
use crate::constants;
use crate::hotkeys;
use crate::singleinstance;
use crate::toast;
use crate::tray;
use crate::{capture, clipboard, config, save};

/// Which hotkey triggered `run_capture` -- keeps the two dispatch arms thin
/// routers into one shared pipeline (PATTERNS.md guidance).
enum CaptureSource {
    Fullscreen,
    ActiveWindow,
}

/// The locked capture pipeline order (CAP-01/CAP-03/CAP-05, D-11/D-16/D-18/
/// D-20/D-22, ERR-01): re-read config -> resolve geometry -> capture ->
/// build advisory -> optional clipboard copy -> hand off to the save worker.
/// Every branch either ends in a saved file or a toast -- never a silent
/// no-op (D-18/ERR-02) -- and never shows any window/dialog (CAP-01/CAP-05).
fn run_capture(source: CaptureSource) {
    let cfg = config::load();

    let (bitmap, advisory) = match source {
        CaptureSource::Fullscreen => {
            let rect = match capture::monitor_under_cursor() {
                Ok(r) => r,
                Err(e) => {
                    toast::show(&format!("Capture failed — {e}"));
                    return;
                }
            };
            let bitmap = match capture::grab(rect) {
                Ok(b) => b,
                Err(e) => {
                    toast::show(&format!("Capture failed — {e}"));
                    return;
                }
            };
            (bitmap, None)
        }
        CaptureSource::ActiveWindow => {
            let target = match capture::active_window_target() {
                Ok(t) => t,
                Err(e) => {
                    toast::show(&format!("Capture failed — {e}"));
                    return;
                }
            };
            let target = match target {
                capture::ActiveWindowTarget::None => {
                    // D-20: no fullscreen fallback -- nothing is saved.
                    toast::show("No active window to capture");
                    return;
                }
                t @ capture::ActiveWindowTarget::Window { .. } => t,
            };
            let rect = match &target {
                capture::ActiveWindowTarget::Window { rect, .. } => *rect,
                capture::ActiveWindowTarget::None => unreachable!(),
            };
            let bitmap = match capture::grab_window(&target) {
                Ok(b) => b,
                Err(e) => {
                    toast::show(&format!("Capture failed — {e}"));
                    return;
                }
            };
            // D-22: the "two monitors" wording stays literal even for 3+.
            let advisory = if capture::monitor_span_count(rect) > 1 {
                Some(
                    "window spanned two monitors; scaling may look mixed. \
                     Tip: capture on a single monitor for best results."
                        .to_string(),
                )
            } else {
                None
            };
            (bitmap, advisory)
        }
    };

    // D-16: clipboard copy happens before the bitmap moves into the save
    // job, and failure is non-fatal -- the file save proceeds regardless.
    if cfg.clipboard_enabled {
        let _ = clipboard::copy_dib(&bitmap);
    }

    // ERR-01/D-17: a folder-creation failure routes through the existing
    // Settings dispatch seam -- zero new plumbing.
    if save::reserve_and_dispatch(bitmap, &cfg, advisory).is_err() {
        toast::show("Can't create save folder — opening Settings");
        dispatch(AppAction::OpenSettings);
    }
}

/// Reports one completed save job's outcome via the locked copy contract.
/// Failures ALWAYS toast, independent of `toast_enabled` -- UI-SPEC's locked
/// reconciliation of D-12 and D-18 is that the toggle suppresses save
/// confirmations only; failures are never silent (ERR-02).
fn handle_save_outcome(outcome: save::SaveOutcome) {
    if let Some(msg) = &outcome.error {
        toast::show(&format!("Save failed — {msg}"));
        return;
    }

    // Re-read config here (D-11) rather than caching a snapshot from the
    // capture that triggered this save.
    let cfg = config::load();
    if !cfg.toast_enabled {
        return;
    }

    let mut text = format!("Saved {}", outcome.file_name);
    if let Some(advisory) = &outcome.advisory {
        text.push_str(&format!(" — {advisory}"));
    }

    toast::show_for_file(
        &text,
        &outcome.path,
        config::ClickAction::from_str(&cfg.toast_click_action),
    );
}

/// Actions the app can perform. Phase 1 wires the routing seams; Phases
/// 2-4 fill in the real behavior behind each variant.
pub enum AppAction {
    OpenSettings,
    OpenCaptureFolder,
    ShowAbout,
    Exit,
    CaptureFullscreen,
    CaptureActiveWindow,
    CaptureRegion,
}

/// Process-global handle to the hidden main window, set exactly once by
/// `create_main_window`. Later plans' event-handler closures read it
/// rather than capturing a possibly-uninitialized handle.
static MAIN_HWND: OnceLock<isize> = OnceLock::new();

/// Returns the hidden main window's handle.
///
/// # Panics
/// Panics if called before `create_main_window` has run.
pub fn main_hwnd() -> HWND {
    let raw = *MAIN_HWND
        .get()
        .expect("main_hwnd() called before create_main_window()");
    HWND(raw as *mut _)
}

/// Registers the hub window class and creates the hidden top-level window.
///
/// A real top-level window is used (not `HWND_MESSAGE`) because the toast
/// and tray crates need a normal owner and broadcast/registered messages
/// must reach it. It is never passed to `ShowWindow` (TRAY-01: no visible
/// window).
pub fn create_main_window() -> Result<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let class_name = constants::to_wide(constants::MAIN_WINDOW_CLASS);

        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // 0 return means registration failed.
        if RegisterClassW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }

        let hwnd = CreateWindowExW(
            Default::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_OVERLAPPED,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            Some(hinstance.into()),
            None,
        )?;

        MAIN_HWND
            .set(hwnd.0 as isize)
            .expect("create_main_window() called more than once");

        Ok(hwnd)
    }
}

/// The blocking `GetMessage`/`TranslateMessage`/`DispatchMessage` pump.
/// Must block (never `PeekMessage`-spin) to hold the 0% idle CPU budget.
/// Exits when `GetMessageW` returns 0 (WM_QUIT).
pub fn run_message_loop() {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Central action handler. Phase 1 behavior: `Exit` quits the message
/// loop; every other variant is a documented stub for a later phase.
pub fn dispatch(action: AppAction) {
    match action {
        AppAction::Exit => unsafe { PostQuitMessage(0) },
        // Phase 1 stub (TRAY-03): the real Settings window is Phase 4.
        AppAction::OpenSettings => {
            toast::show("Settings — coming in a later version");
        }
        AppAction::OpenCaptureFolder => {
            let dir = constants::default_capture_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                // Same seam ERR-01 formalizes in Phase 2 -- toast the OS
                // error rather than panicking.
                toast::show(&format!("Couldn't open capture folder: {e}"));
            } else {
                let dir_str = dir.to_string_lossy();
                let path = constants::to_wide(&dir_str);
                let verb = constants::to_wide("open");
                unsafe {
                    ShellExecuteW(
                        Some(main_hwnd()),
                        PCWSTR(verb.as_ptr()),
                        PCWSTR(path.as_ptr()),
                        PCWSTR(std::ptr::null()),
                        PCWSTR(std::ptr::null()),
                        SW_SHOWNORMAL,
                    );
                }
            }
        }
        AppAction::ShowAbout => about::show(),
        // Phase 2: F9 fullscreen capture of the monitor under the cursor.
        AppAction::CaptureFullscreen => {
            run_capture(CaptureSource::Fullscreen);
        }
        // Phase 2: Ctrl+F9 capture of the active (focused) window.
        AppAction::CaptureActiveWindow => {
            run_capture(CaptureSource::ActiveWindow);
        }
        // Phase 3: Shift+F9 frozen-overlay region capture.
        AppAction::CaptureRegion => {
            toast::show("Shift+F9 — region capture (coming soon)");
        }
    }
}

/// Window procedure for the hidden hub window. Routes `WM_APP_*` messages
/// to `dispatch`; everything else falls through to `DefWindowProcW`.
///
/// T-01-04 mitigation: four `WM_APP_*` ids are now handled, each arm treats
/// wparam as an opaque id validated before dispatch (or, for the save-done
/// arm, ignores wparam entirely), and no arm dereferences a caller-supplied
/// pointer.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // A registered message id is a runtime value and cannot appear in a
    // `match` pattern, so it is checked ahead of the match below.
    if msg == singleinstance::already_running_message() {
        toast::show("Corvus Capture is already running");
        return LRESULT(0);
    }

    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        m if m == constants::WM_APP_HOTKEY => {
            // wparam is the fired hotkey's runtime id, validated against
            // the registered-hotkey map before dispatch (T-01-17); unknown
            // ids are ignored rather than treated as a panic.
            if let Some((_label, action_kind)) = hotkeys::lookup(wparam.0 as u32) {
                dispatch(action_kind.to_action());
            }
            LRESULT(0)
        }
        m if m == constants::WM_APP_MENU => {
            match wparam.0 {
                tray::MENU_INDEX_SETTINGS => dispatch(AppAction::OpenSettings),
                tray::MENU_INDEX_OPEN_FOLDER => dispatch(AppAction::OpenCaptureFolder),
                tray::MENU_INDEX_ABOUT => dispatch(AppAction::ShowAbout),
                tray::MENU_INDEX_EXIT => dispatch(AppAction::Exit),
                // Unknown indices are ignored rather than treated as a panic.
                _ => {}
            }
            LRESULT(0)
        }
        m if m == constants::WM_APP_TRAY => {
            // TRAY-03: left-click opens (the seam for) Settings.
            if wparam.0 == tray::TRAY_INDEX_LEFT_CLICK {
                dispatch(AppAction::OpenSettings);
            }
            LRESULT(0)
        }
        m if m == constants::WM_APP_SAVE_DONE => {
            // T-02-16: wparam/lparam carry no meaning -- the outcome itself
            // travels through save's own process-internal result queue, so
            // a forged message with an empty queue is a no-op.
            while let Some(outcome) = save::take_result() {
                handle_save_outcome(outcome);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
