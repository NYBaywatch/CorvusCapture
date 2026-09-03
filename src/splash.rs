//! Startup splash: shows the Corvus logo for ~2.5s when the app launches, so
//! the user has visible confirmation the tray app is running. Same
//! discipline as `toast.rs` -- plain Win32, no panics, best-effort.
//!
//! The decoded pixel buffer is freed on `WM_DESTROY` (window close or
//! auto-dismiss) so the idle-RAM budget (<10 MB) never retains it after the
//! splash disappears.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Mutex, OnceLock};

use windows::core::{PCWSTR, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, MonitorFromPoint, SetDIBitsToDevice, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    RegisterClassW, SetTimer, ShowWindow, TranslateMessage, MSG, SW_SHOWNOACTIVATE, WM_DESTROY,
    WM_LBUTTONDOWN, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::constants;

/// Splash logo, embedded at compile time (480x480 PNG, ~164 KB).
const SPLASH_PNG: &[u8] = include_bytes!("../resources/splash.png");

/// How long the splash stays visible before auto-dismissing.
const SPLASH_DURATION_MS: u32 = 2500;

/// Timer id for the splash auto-dismiss `SetTimer` call. Distinct from
/// `TOAST_TIMER_ID`/`SETTINGS_HINT_TIMER_BASE` so the three windows' timers
/// can never collide.
const SPLASH_TIMER_ID: usize = 2;

const SPLASH_WINDOW_CLASS: &str = "CorvusCaptureSplashWnd";

/// Decoded top-down 32bpp BGRA pixels for the splash image, plus dimensions.
/// Populated in `show()` right before the window is created, freed on
/// `WM_DESTROY`.
struct SplashImage {
    width: i32,
    height: i32,
    bgra: Vec<u8>,
}

static SPLASH_HWND: Mutex<Option<isize>> = Mutex::new(None);
static SPLASH_IMAGE: Mutex<Option<SplashImage>> = Mutex::new(None);
static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

/// Shows the startup splash. No-op if a splash is already showing. Decodes
/// the embedded PNG at call time (not at compile time) so the decoded buffer
/// only exists while the splash is actually on screen.
pub fn show() {
    if SPLASH_HWND.lock().unwrap().is_some() {
        return;
    }

    let image = match decode_splash() {
        Some(img) => img,
        None => return, // best-effort: no splash if decode fails
    };
    let (width, height) = (image.width, image.height);
    *SPLASH_IMAGE.lock().unwrap() = Some(image);

    register_class_once();

    let (x, y) = compute_centered_position(width, height);

    let result: Result<HWND> = unsafe {
        let hinstance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let class_name = constants::to_wide(SPLASH_WINDOW_CLASS);
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_POPUP,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    };

    if let Ok(hwnd) = result {
        *SPLASH_HWND.lock().unwrap() = Some(hwnd.0 as isize);
        unsafe {
            let _ = SetTimer(Some(hwnd), SPLASH_TIMER_ID, SPLASH_DURATION_MS, None);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    } else {
        // Window creation failed -- drop the decoded buffer immediately
        // rather than leaking it until process exit.
        *SPLASH_IMAGE.lock().unwrap() = None;
    }
}

/// Decodes the embedded PNG into a top-down 32bpp BGRA buffer (the layout
/// `SetDIBitsToDevice` with a negative `biHeight` expects, matching
/// `capture.rs`/`overlay.rs`'s DIB convention).
fn decode_splash() -> Option<SplashImage> {
    let decoded = image::load_from_memory(SPLASH_PNG).ok()?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();

    let mut bgra = Vec::with_capacity(rgba.len());
    for px in rgba.pixels() {
        let [r, g, b, a] = px.0;
        bgra.push(b);
        bgra.push(g);
        bgra.push(r);
        bgra.push(a);
    }

    Some(SplashImage {
        width: width as i32,
        height: height as i32,
        bgra,
    })
}

/// Centers a `width`x`height` window on the work area of the primary
/// monitor -- kept simple per plan (cursor-monitor centering is equally
/// acceptable but the primary monitor is deterministic for a launch-time
/// splash).
fn compute_centered_position(width: i32, height: i32) -> (i32, i32) {
    let hmon = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetMonitorInfoW(hmon, &mut mi);
    }
    let work = mi.rcWork;
    let x = work.left + (work.right - work.left - width) / 2;
    let y = work.top + (work.bottom - work.top - height) / 2;
    (x, y)
}

fn register_class_once() {
    CLASS_REGISTERED.get_or_init(|| unsafe {
        let hinstance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let class_name = constants::to_wide(SPLASH_WINDOW_CLASS);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            panic!("failed to register splash window class");
        }
    });
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };

    if let Some(image) = SPLASH_IMAGE.lock().unwrap().as_ref() {
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = image.width;
        bmi.bmiHeader.biHeight = -image.height; // negative => top-down DIB
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        unsafe {
            SetDIBitsToDevice(
                hdc,
                0,
                0,
                image.width as u32,
                image.height as u32,
                0,
                0,
                0,
                image.height as u32,
                image.bgra.as_ptr() as *const c_void,
                &bmi,
                DIB_RGB_COLORS,
            );
        }
    }

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
                let _ = KillTimer(Some(hwnd), SPLASH_TIMER_ID);
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            unsafe {
                let _ = KillTimer(Some(hwnd), SPLASH_TIMER_ID);
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            *SPLASH_HWND.lock().unwrap() = None;
            // Free the decoded pixel buffer -- must not survive the splash
            // (idle RAM budget).
            *SPLASH_IMAGE.lock().unwrap() = None;
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Dev-only: pumps messages until the splash is destroyed (auto-dismiss or
/// click). Mirrors `toast::run_until_dismissed`; not yet wired to a CLI flag
/// but kept for parity / possible future `--splash-test`.
#[allow(dead_code)]
pub fn run_until_dismissed() {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if SPLASH_HWND.lock().unwrap().is_none() {
                break;
            }
        }
    }
}
