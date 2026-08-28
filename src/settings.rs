//! The Settings window (SET-01, SET-02): a raw Win32, DPI-aware, modeless
//! window reachable from Alt+F9, the tray left-click, the tray menu
//! "Settings" item, and the ERR-01 failure path (D-38). Standard native
//! look throughout (D-41) -- system dialog colors, stock common controls,
//! zero owner-drawing, fixed size (D-40/D-44).
//!
//! This plan builds the window shell, the child-control set, and
//! population from the current config/registry snapshot (read-only
//! display). Writing changes back, validation, the folder picker, and the
//! Start-with-Windows toggle handler land in plan 04-04.

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Mutex, OnceLock};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, GetMonitorInfoW, MonitorFromPoint, COLOR_BTNFACE,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{InitCommonControlsEx, ICC_BAR_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, SystemParametersInfoForDpi, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, FlashWindowEx, GetCursorPos,
    GetForegroundWindow, IsDialogMessageW, IsIconic, IsWindow, KillTimer, LoadCursorW,
    LoadIconW, RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindow,
    FLASHWINFO, FLASHW_ALL, IDCANCEL, IDC_ARROW, MSG,
    SPI_GETNONCLIENTMETRICS, SWP_NOACTIVATE, SWP_NOZORDER, SW_RESTORE, SW_SHOW, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_DPICHANGED, WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_OVERLAPPED,
    WS_SYSMENU,
};

use crate::config::{self, Config};
use crate::constants;

/// Resource ordinal of the embedded app icon (`resources/app.rc`:
/// `IDI_APPICON 1`), mirroring `tray.rs`'s constant of the same value --
/// the Settings window uses the same crow icon, not a second copy.
const IDI_APPICON: u16 = 1;

/// Combined `WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU` -- no resize-grip
/// style and no min/max boxes -- fixed-size window (D-40, D-44).
const WINDOW_STYLE_FLAGS: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE =
    windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
        WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0,
    );

/// Process-global handle to the currently-open Settings window, if any.
/// Cleared by `wnd_proc` on `WM_DESTROY`, mirroring `toast.rs`'s
/// `TOAST_HWND` convention.
static SETTINGS_HWND: Mutex<Option<isize>> = Mutex::new(None);

/// Guards one-time registration of the Settings window class.
static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

/// Guards the one-time `InitCommonControlsEx` call -- required before the
/// trackbar class (`msctls_trackbar32`) is registered (RESEARCH Pattern 0).
static COMMON_CONTROLS_INIT: OnceLock<()> = OnceLock::new();

/// Live Settings window state, guarded so only the pump thread ever
/// touches it (mirrors `overlay.rs`'s `OVERLAY_DATA`/`with_state`
/// convention). Every mutation goes through `with_state`; the guard is
/// never held across `SendMessageW`, `DestroyWindow`, `SetWindowTextW`,
/// `config::save`, or `toast::show` (the overlay.rs copy-out-then-call
/// rule, avoiding reentrant-lock deadlocks).
struct SettingsData {
    /// The config snapshot loaded once at open (D-39) -- never reloaded
    /// inside a notification handler.
    ///
    /// Not yet read: population arrives in Task 3.
    #[allow(dead_code)]
    cfg: Config,
    /// The current message font, as an `HFONT` bit pattern (`isize`) --
    /// handles are not `Send`, so raw values are stored instead.
    font: isize,
    /// Every child control's `HWND` (also as `isize`), keyed by its
    /// `ID_*` control id (`constants::ID_BASE_EDIT` etc.). Decorative,
    /// non-addressable children (group boxes, plain labels) are tracked
    /// only in `all_children` below.
    controls: HashMap<i32, isize>,
    /// Every child `HWND` created for this window, addressable or not --
    /// the single list `WM_DPICHANGED` walks to re-send `WM_SETFONT` after
    /// rebuilding the font for the new DPI (Task 1 action).
    all_children: Vec<isize>,
    /// The monitor DPI this window was last laid out for.
    dpi: u32,
    /// Set once by `close()`; guards against a second `WM_CLOSE`/
    /// `IDCANCEL` re-entering the close path mid-teardown (overlay.rs
    /// Pitfall 3 cancel-funnel shape).
    closing: bool,
    /// True from creation until `populate()` finishes; notification
    /// handlers added in plan 04-04 must return early while this is set
    /// (`SetWindowTextW` on an EDIT fires `EN_CHANGE` even during
    /// programmatic population).
    initializing: bool,
    /// The last-committed base filename, for plan 04-04's revert-on-invalid
    /// path.
    #[allow(dead_code)]
    last_base: String,
    /// The last-committed (resolved) save folder, for plan 04-04's
    /// revert-on-invalid path.
    #[allow(dead_code)]
    last_folder: String,
}

static SETTINGS_DATA: Mutex<Option<SettingsData>> = Mutex::new(None);

/// Runs `f` with mutable access to the live `SettingsData`, or returns
/// `None` if no Settings window is open. The single seam every accessor
/// below uses instead of locking `SETTINGS_DATA` directly (overlay.rs
/// `with_state` convention).
fn with_state<R>(f: impl FnOnce(&mut SettingsData) -> R) -> Option<R> {
    let mut guard = SETTINGS_DATA.lock().unwrap();
    guard.as_mut().map(f)
}

// ---------------------------------------------------------------------
// Class / common-controls registration
// ---------------------------------------------------------------------

fn register_class_once() {
    CLASS_REGISTERED.get_or_init(|| unsafe {
        let hinstance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let class_name = constants::to_wide(constants::SETTINGS_WINDOW_CLASS);
        // Classic Win32 idiom: a system color index + 1, cast to HBRUSH,
        // paints the dialog face background (D-41) with zero owner-drawing.
        let background = windows::Win32::Graphics::Gdi::HBRUSH(
            (COLOR_BTNFACE.0 as usize + 1) as *mut c_void,
        );
        let icon = LoadIconW(Some(hinstance.into()), PCWSTR(IDI_APPICON as usize as *const u16))
            .unwrap_or_default();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hbrBackground: background,
            hIcon: icon,
            hCursor: cursor,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            panic!("failed to register Settings window class");
        }
    });
}

/// `InitCommonControlsEx(ICC_STANDARD_CLASSES | ICC_BAR_CLASSES)`, required
/// once before `msctls_trackbar32` (the JPG quality slider) exists.
fn init_common_controls_once() {
    COMMON_CONTROLS_INIT.get_or_init(|| unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_BAR_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    });
}

// ---------------------------------------------------------------------
// DPI / geometry helpers (D-40, D-44, Pitfall 5)
// ---------------------------------------------------------------------

/// The work-area rect and effective DPI of the monitor under the cursor
/// (D-40 -- "opens centered on the monitor under the cursor").
fn target_monitor_rect_and_dpi() -> (RECT, u32) {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let hmon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetMonitorInfoW(hmon, &mut mi);
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    unsafe {
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    }
    (mi.rcWork, dx)
}

/// Rounded integer scaling from the 96-DPI logical value `v` to the target
/// `dpi` (UI-SPEC "Spacing Scale" formula).
fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}

/// Computes the top-level window rect (including non-client frame) for a
/// `client_w` x `client_h` (96-DPI logical) client area, centered in
/// `work` at `dpi`.
fn window_rect_centered(
    client_w: i32,
    client_h: i32,
    work: RECT,
    dpi: u32,
    style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE,
    ex: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
) -> RECT {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: scale(client_w, dpi),
        bottom: scale(client_h, dpi),
    };
    unsafe {
        let _ = AdjustWindowRectExForDpi(&mut r, style, false, ex, dpi);
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let x = work.left + (work.right - work.left - w) / 2;
    let y = work.top + (work.bottom - work.top - h) / 2;
    RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

/// Builds the system message font (`SPI_GETNONCLIENTMETRICS.lfMessageFont`)
/// for `dpi` -- the dialog-convention font (never a hardcoded "Segoe UI"
/// font-by-name create call, that is the toast/overlay convention).
fn message_font_for_dpi(dpi: u32) -> windows::Win32::Graphics::Gdi::HFONT {
    let mut ncm = windows::Win32::UI::WindowsAndMessaging::NONCLIENTMETRICSW {
        cbSize: size_of::<windows::Win32::UI::WindowsAndMessaging::NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            ncm.cbSize,
            Some(&mut ncm as *mut _ as *mut c_void),
            0,
            dpi,
        );
        CreateFontIndirectW(&ncm.lfMessageFont)
    }
}

// ---------------------------------------------------------------------
// Open / focus lifecycle (SET-01, D-38)
// ---------------------------------------------------------------------

/// Opens the Settings window, or focuses it if one is already open --
/// every seam (Alt+F9, tray left-click, tray menu, ERR-01) calls this and
/// only this (SET-01, D-38).
pub fn open_or_focus() {
    let existing = *SETTINGS_HWND.lock().unwrap();
    if let Some(raw) = existing {
        let hwnd = HWND(raw as *mut c_void);
        if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            unsafe {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = ShowWindow(hwnd, SW_SHOW);
                if !SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() != hwnd {
                    // Foreground lock denied (Pitfall 6): flash instead of
                    // fighting it with an AttachThreadInput hack.
                    let fwi = FLASHWINFO {
                        cbSize: size_of::<FLASHWINFO>() as u32,
                        hwnd,
                        dwFlags: FLASHW_ALL,
                        uCount: 3,
                        dwTimeout: 0,
                    };
                    let _ = FlashWindowEx(&fwi);
                }
            }
            return;
        }
    }
    create_and_show();
}

fn create_and_show() {
    register_class_once();
    init_common_controls_once();

    // D-39: load config exactly once, at open. Handlers added in plan
    // 04-04 must never call config::load() again while this window lives.
    let cfg = config::load();

    let (work, dpi) = target_monitor_rect_and_dpi();
    let rect = window_rect_centered(
        constants::SETTINGS_CLIENT_W,
        constants::SETTINGS_CLIENT_H,
        work,
        dpi,
        WINDOW_STYLE_FLAGS,
        Default::default(),
    );

    let class_name = constants::to_wide(constants::SETTINGS_WINDOW_CLASS);
    let title = constants::to_wide(constants::SETTINGS_WINDOW_TITLE);
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE_FLAGS,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(GetModuleHandleW(None).expect("GetModuleHandleW failed").into()),
            None,
        )
    };
    let Ok(hwnd) = hwnd else {
        return;
    };

    let font = message_font_for_dpi(dpi);
    let last_folder = config::resolve_save_folder(&cfg).to_string_lossy().into_owned();
    *SETTINGS_DATA.lock().unwrap() = Some(SettingsData {
        last_base: cfg.base_filename.clone(),
        last_folder,
        cfg,
        font: font.0 as isize,
        controls: HashMap::new(),
        all_children: Vec::new(),
        dpi,
        closing: false,
        initializing: true,
    });
    *SETTINGS_HWND.lock().unwrap() = Some(hwnd.0 as isize);

    let (controls, all_children) = create_children(hwnd, dpi, font);
    with_state(|d| {
        d.controls = controls;
        d.all_children = all_children;
    });
    layout(hwnd, dpi);
    populate(hwnd);
    with_state(|d| d.initializing = false);

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    set_initial_focus(hwnd);
}

/// `IsDialogMessageW` against the Settings window if one is open --
/// gives Tab/Shift+Tab/arrows/mnemonics/Enter/Esc the standard dialog
/// keyboard interface (RESEARCH Pattern 2). Called from `app::
/// run_message_loop` between `GetMessageW` and `TranslateMessage`; a
/// message it consumes must never reach `TranslateMessage`/
/// `DispatchMessageW`.
pub fn is_dialog_message(msg: &MSG) -> bool {
    let Some(raw) = *SETTINGS_HWND.lock().unwrap() else {
        return false;
    };
    let hwnd = HWND(raw as *mut c_void);
    unsafe { IsDialogMessageW(hwnd, msg).as_bool() }
}

// ---------------------------------------------------------------------
// Child controls / layout -- filled in by Task 2
// ---------------------------------------------------------------------

/// Creates every child control from the UI-SPEC layout table and returns
/// the addressable (`ID_*`-keyed) and full child-HWND lists. Empty in this
/// task -- the window is chrome-only until Task 2 fills this in.
fn create_children(
    _hwnd: HWND,
    _dpi: u32,
    _font: windows::Win32::Graphics::Gdi::HFONT,
) -> (HashMap<i32, isize>, Vec<isize>) {
    (HashMap::new(), Vec::new())
}

/// Scales and positions every child control for `dpi`. No-op in this task
/// -- there are no children yet.
fn layout(_hwnd: HWND, _dpi: u32) {}

// ---------------------------------------------------------------------
// Population -- filled in by Task 3
// ---------------------------------------------------------------------

/// Populates every control from the config/registry snapshot. No-op in
/// this task -- there are no children yet.
fn populate(_hwnd: HWND) {}

/// Sets initial focus. No-op in this task -- there is no base-filename
/// edit yet to focus.
fn set_initial_focus(_hwnd: HWND) {}

// ---------------------------------------------------------------------
// Window procedure
// ---------------------------------------------------------------------

/// The single cancel/close funnel: sets `closing` then destroys the
/// window. Idempotent -- a second call while already closing is a no-op
/// (overlay.rs Pitfall 3 shape). Committing uncommitted edits before
/// `DestroyWindow` is added in plan 04-04.
fn close(hwnd: HWND) {
    let already_closing = with_state(|d| {
        let was = d.closing;
        d.closing = true;
        was
    })
    .unwrap_or(true);
    if already_closing {
        return;
    }
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            close(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            // T-04-11: wparam is opaque, matched only against the known
            // IDCANCEL id delivered by IsDialogMessageW for Esc; every
            // other id falls through (plan 04-04 adds real handlers).
            let id = (wparam.0 & 0xFFFF) as i32;
            if id == IDCANCEL.0 {
                close(hwnd);
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let new_dpi = (wparam.0 >> 16) as u32;
            let new_font = message_font_for_dpi(new_dpi);
            let children = with_state(|d| d.all_children.clone()).unwrap_or_default();
            for raw in &children {
                let child = HWND(*raw as *mut c_void);
                unsafe {
                    let _ = SendMessageW(
                        child,
                        WM_SETFONT,
                        Some(WPARAM(new_font.0 as usize)),
                        Some(LPARAM(1)),
                    );
                }
            }
            let old_font = with_state(|d| {
                let old = d.font;
                d.font = new_font.0 as isize;
                d.dpi = new_dpi;
                old
            });
            if let Some(old) = old_font {
                if old != 0 {
                    unsafe {
                        let _ = DeleteObject(
                            windows::Win32::Graphics::Gdi::HFONT(old as *mut c_void).into(),
                        );
                    }
                }
            }
            layout(hwnd, new_dpi);
            let suggested = unsafe { *(lparam.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // Single cleanup point (Pitfall 10): free the font, kill any
            // pending hint timers, clear the process-global HWND.
            let data = SETTINGS_DATA.lock().unwrap().take();
            if let Some(d) = data {
                if d.font != 0 {
                    unsafe {
                        let _ = DeleteObject(
                            windows::Win32::Graphics::Gdi::HFONT(d.font as *mut c_void).into(),
                        );
                    }
                }
                for id in [
                    constants::ID_BASE_HINT,
                    constants::ID_FOLDER_HINT,
                    constants::ID_STARTUP_HINT,
                ] {
                    unsafe {
                        let _ = KillTimer(
                            Some(hwnd),
                            constants::SETTINGS_HINT_TIMER_BASE + id as usize,
                        );
                    }
                }
            }
            *SETTINGS_HWND.lock().unwrap() = None;
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
