//! Dark theme support for the Settings window (UI-03): OS dark-mode
//! detection, cached dark/light palette brushes, DWM dark title-bar
//! attribute, and per-control text color -- everything `settings.rs` needs
//! to follow the Windows system theme, including a live update when the
//! user changes it while the window is open.
//!
//! Built mostly from documented/stable APIs (DWM title-bar attribute,
//! `WM_CTLCOLOR*` handlers driven from here, `SetWindowTheme` dark
//! classes), plus one best-effort undocumented uxtheme ordinal
//! (`enable_dark_context_menus`) used only to dark-theme the native tray
//! context menu, which has no documented dark-mode API.

use std::ffi::c_void;
use std::sync::Mutex;

use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, GetSysColor, COLOR_BTNFACE, COLOR_WINDOW, COLOR_WINDOWTEXT,
    HBRUSH,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::core::{w, PCSTR, PCWSTR, BOOL};

use crate::constants;

/// Dark window background (COLORREF `0x00BBGGRR` order).
pub const WINDOW_BG: u32 = 0x0020_2020;
/// Dark control (label/edit/listbox) background.
pub const CONTROL_BG: u32 = 0x002B_2B2B;
/// Dark-mode text color: matrix green (`#00FF41`, COLORREF `0x00BBGGRR`
/// order).
pub const TEXT_COLOR: u32 = 0x0041_FF00;

const AUTO_THEME_SUBKEY: PCWSTR =
    w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
const AUTO_THEME_VALUE: &str = "AppsUseLightTheme";

/// `HBRUSH`/`isize` bit patterns for the two cached dark-mode brushes --
/// created once via `CreateSolidBrush`, never recreated per-message
/// (mirrors `settings.rs`'s handle-in-a-static convention: handles are
/// never `Send`, so the raw bit pattern is what's stored).
struct Brushes {
    window: Option<isize>,
    control: Option<isize>,
}

static BRUSHES: Mutex<Brushes> = Mutex::new(Brushes {
    window: None,
    control: None,
});

/// True when Windows apps are set to Dark mode (`AppsUseLightTheme == 0`).
/// Missing key/value or any read failure defaults to light (`false`) --
/// the same "absence -> safe default" convention `startup::read_command`
/// uses.
pub fn is_dark() -> bool {
    unsafe {
        let mut value: u32 = 0;
        let mut len: u32 = size_of::<u32>() as u32;
        let value_name = constants::to_wide(AUTO_THEME_VALUE);
        let err = RegGetValueW(
            HKEY_CURRENT_USER,
            AUTO_THEME_SUBKEY,
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut len),
        );
        if err.is_err() {
            return false;
        }
        value == 0
    }
}

/// Lazily creates (once) and returns the cached dark window-background
/// brush.
pub fn dark_window_brush() -> HBRUSH {
    let mut brushes = BRUSHES.lock().unwrap();
    if brushes.window.is_none() {
        let brush = unsafe { CreateSolidBrush(COLORREF(WINDOW_BG)) };
        brushes.window = Some(brush.0 as isize);
    }
    HBRUSH(brushes.window.unwrap() as *mut c_void)
}

/// Lazily creates (once) and returns the cached dark control-background
/// brush.
pub fn dark_control_brush() -> HBRUSH {
    let mut brushes = BRUSHES.lock().unwrap();
    if brushes.control.is_none() {
        let brush = unsafe { CreateSolidBrush(COLORREF(CONTROL_BG)) };
        brushes.control = Some(brush.0 as isize);
    }
    HBRUSH(brushes.control.unwrap() as *mut c_void)
}

/// Deletes and clears both cached brushes so the next `dark_window_brush`/
/// `dark_control_brush` call recreates them. Called on a theme change
/// (`WM_SETTINGCHANGE`) so a stale brush is never reused, and safe to call
/// even when neither brush was ever created.
pub fn refresh() {
    let mut brushes = BRUSHES.lock().unwrap();
    if let Some(raw) = brushes.window.take() {
        unsafe {
            let _ = DeleteObject(HBRUSH(raw as *mut c_void).into());
        }
    }
    if let Some(raw) = brushes.control.take() {
        unsafe {
            let _ = DeleteObject(HBRUSH(raw as *mut c_void).into());
        }
    }
}

/// Deletes both cached brushes without recreating them -- call once from
/// process/window teardown (`WM_DESTROY`) to avoid a GDI handle leak.
/// Equivalent to `refresh()`; kept as a distinctly-named entry point for
/// callers that mean "tear down for good", not "theme changed".
pub fn teardown() {
    refresh();
}

/// Sets/clears the DWM dark title-bar attribute for `hwnd` (RESEARCH
/// Pattern 1). Call before `ShowWindow` to avoid a light-then-dark flash.
pub fn apply_titlebar(hwnd: HWND, dark: bool) {
    let dark: BOOL = dark.into();
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const c_void,
            size_of::<BOOL>() as u32,
        );
    }
}

/// The text color to paint with: the dark-mode constant when `is_dark()`,
/// otherwise the system `COLOR_WINDOWTEXT` (so light mode always matches
/// the OS, never a guessed constant).
pub fn text_color() -> COLORREF {
    if is_dark() {
        COLORREF(TEXT_COLOR)
    } else {
        COLORREF(unsafe { GetSysColor(COLOR_WINDOWTEXT) })
    }
}

/// The window/control background color to paint with in light mode,
/// queried live from `GetSysColor` rather than a hardcoded COLORREF, so
/// light mode always matches the OS.
#[allow(dead_code)]
pub fn light_window_color() -> COLORREF {
    COLORREF(unsafe { GetSysColor(COLOR_BTNFACE) })
}

#[allow(dead_code)]
pub fn light_control_color() -> COLORREF {
    COLORREF(unsafe { GetSysColor(COLOR_WINDOW) })
}

/// Best-effort dark-theming for native popup menus (the tray right-click
/// context menu), via the undocumented `uxtheme.dll` ordinal 135
/// (`SetPreferredAppMode`). There is no documented API for dark-themed
/// native `HMENU` popups -- owner-drawing the menu was rejected as
/// disproportionate complexity for a 4-item popup, so this mirrors the
/// pattern reference dark-mode implementations (e.g. ysc3839/win32-darkmode)
/// use. Loads the DLL and resolves the ordinal dynamically; if either step
/// fails (missing export on a future/older Windows build) this silently
/// does nothing -- no panic, no error, the menu simply stays light. Must be
/// called before the menu (`HMENU`) is created. `uxtheme.dll` is already
/// resident for a themed Win32 app, so the loaded-module reference is never
/// freed -- consistent with this project's "don't over-engineer lifecycle
/// for OS-resident DLLs" posture elsewhere in this file.
pub fn enable_dark_context_menus() {
    unsafe {
        let name = w!("uxtheme.dll");
        let Ok(module) = LoadLibraryW(name) else {
            return;
        };
        let Some(addr) = GetProcAddress(module, PCSTR(135usize as *const u8)) else {
            return;
        };
        let set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32 =
            std::mem::transmute(addr);
        // 1 == AllowDark.
        let _ = set_preferred_app_mode(1);
    }
}
