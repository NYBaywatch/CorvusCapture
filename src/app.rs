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
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, CW_USEDEFAULT, MSG, WM_DESTROY, WNDCLASSW, WS_OVERLAPPED,
};

use crate::constants;

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
        // Phase 4: open (or focus) the raw Win32 Settings window.
        AppAction::OpenSettings => {}
        // Phase 2: open constants::default_capture_dir() in Explorer.
        AppAction::OpenCaptureFolder => {}
        // Phase 1/5: show the standard MessageBox About dialog (D-06).
        AppAction::ShowAbout => {}
        // Phase 2: F9 fullscreen capture of the monitor under the cursor.
        AppAction::CaptureFullscreen => {}
        // Phase 2: Ctrl+F9 capture of the active (focused) window.
        AppAction::CaptureActiveWindow => {}
        // Phase 3: Shift+F9 frozen-overlay region capture.
        AppAction::CaptureRegion => {}
    }
}

/// Window procedure for the hidden hub window. Routes `WM_APP_*` messages
/// to `dispatch`; everything else falls through to `DefWindowProcW`.
///
/// T-01-04 mitigation: only the three known `WM_APP_*` ids are handled,
/// each arm treats wparam as an opaque id validated before dispatch, and
/// no arm dereferences a caller-supplied pointer in Phase 1.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        m if m == constants::WM_APP_HOTKEY => {
            // Plan 01-04/01-05 fill in the wparam -> AppAction mapping
            // (which registered hotkey fired).
            LRESULT(0)
        }
        m if m == constants::WM_APP_MENU => {
            // Plan 01-04 fills in the wparam -> AppAction mapping
            // (which tray menu item was selected).
            LRESULT(0)
        }
        m if m == constants::WM_APP_TRAY => {
            // Plan 01-04 fills in tray icon event handling (e.g. left-click).
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
