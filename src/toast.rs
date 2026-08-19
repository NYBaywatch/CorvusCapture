//! The reusable, non-focus-stealing notification toast (D-02/D-03).
//!
//! `show(text)` is callable from any module on the UI thread. Phase 2
//! reuses it verbatim for save confirmations ("Saved task004.png").
//!
//! A single toast window is reused across calls: if one is already
//! visible, its text is replaced and its auto-dismiss timer restarted
//! instead of stacking a second window on screen.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use windows::core::{PCWSTR, Result};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DEFAULT_QUALITY, DT_CENTER, DT_VCENTER, DT_WORDBREAK, FF_DONTCARE, FW_NORMAL,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    RegisterClassW, SetLayeredWindowAttributes, SetTimer, ShowWindow, SystemParametersInfoW,
    TranslateMessage, LWA_ALPHA, MSG, SPI_GETWORKAREA, SW_SHOWNOACTIVATE,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_DESTROY, WM_LBUTTONDOWN, WM_PAINT, WM_TIMER,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::constants;

/// Process-global handle to the currently-shown toast, if any. Cleared by
/// `wnd_proc` on `WM_DESTROY` so the next `show` call recreates the window.
static TOAST_HWND: Mutex<Option<isize>> = Mutex::new(None);

/// Text rendered by the toast's `WM_PAINT` handler.
static TOAST_TEXT: Mutex<String> = Mutex::new(String::new());

/// Guards one-time registration of the toast window class.
static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

/// Shows (or updates) the toast with `text`. Safe to call repeatedly and
/// rapidly — a visible toast has its text replaced and timer restarted
/// rather than a new window being stacked on top.
pub fn show(text: &str) {
    register_class_once();
    *TOAST_TEXT.lock().unwrap() = text.to_string();

    let existing = *TOAST_HWND.lock().unwrap();
    if let Some(raw) = existing {
        let hwnd = HWND(raw as *mut c_void);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, true);
            let _ = SetTimer(
                Some(hwnd),
                constants::TOAST_TIMER_ID,
                constants::TOAST_DURATION_MS,
                None,
            );
        }
        return;
    }

    if let Some(hwnd) = create_toast_window() {
        *TOAST_HWND.lock().unwrap() = Some(hwnd.0 as isize);
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 230, LWA_ALPHA);
            let _ = SetTimer(
                Some(hwnd),
                constants::TOAST_TIMER_ID,
                constants::TOAST_DURATION_MS,
                None,
            );
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
}

/// Dev-only: pumps messages until the currently-shown toast is destroyed.
/// Used by `main.rs --toast-test` so the process can exit once the toast
/// has fully appeared and auto-dismissed.
pub fn run_until_dismissed() {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if TOAST_HWND.lock().unwrap().is_none() {
                break;
            }
        }
    }
}

fn register_class_once() {
    CLASS_REGISTERED.get_or_init(|| unsafe {
        let hinstance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let class_name = constants::to_wide(constants::TOAST_WINDOW_CLASS);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            panic!("failed to register toast window class");
        }
    });
}

fn create_toast_window() -> Option<HWND> {
    let work_area = get_work_area();
    let x = work_area.right - constants::TOAST_WIDTH - constants::TOAST_MARGIN;
    let y = work_area.bottom - constants::TOAST_HEIGHT - constants::TOAST_MARGIN;

    let result: Result<HWND> = unsafe {
        let hinstance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let class_name = constants::to_wide(constants::TOAST_WINDOW_CLASS);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_POPUP,
            x,
            y,
            constants::TOAST_WIDTH,
            constants::TOAST_HEIGHT,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    };
    result.ok()
}

fn get_work_area() -> RECT {
    let mut rect = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rect as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    rect
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };

    let background = unsafe { CreateSolidBrush(COLORREF(0x00202020)) };
    unsafe { FillRect(hdc, &ps.rcPaint, background) };
    let _ = unsafe { DeleteObject(background.into()) };

    let font_name = constants::to_wide("Segoe UI");
    let font = unsafe {
        CreateFontW(
            18,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
            PCWSTR(font_name.as_ptr()),
        )
    };
    let old_font = unsafe { SelectObject(hdc, font.into()) };
    unsafe { SetBkMode(hdc, TRANSPARENT) };
    unsafe { SetTextColor(hdc, COLORREF(0x00E0E0E0)) };

    let mut text_rect = ps.rcPaint;
    let text = TOAST_TEXT.lock().unwrap().clone();
    let mut wide_text = constants::to_wide(&text);
    // DrawTextW takes the buffer length excluding the trailing NUL when a
    // positive count is supplied; -1 also works but pop the NUL to be explicit.
    wide_text.pop();
    unsafe {
        DrawTextW(
            hdc,
            &mut wide_text,
            &mut text_rect,
            DT_CENTER | DT_VCENTER | DT_WORDBREAK,
        )
    };

    unsafe { SelectObject(hdc, old_font) };
    let _ = unsafe { DeleteObject(font.into()) };

    let _ = unsafe { EndPaint(hwnd, &ps) };
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            unsafe { paint(hwnd) };
            LRESULT(0)
        }
        WM_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), constants::TOAST_TIMER_ID);
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            *TOAST_HWND.lock().unwrap() = None;
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
