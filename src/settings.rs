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
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, GetMonitorInfoW, MonitorFromPoint, MonitorFromRect,
    COLOR_BTNFACE, HBRUSH, HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemServices::{SS_LEFT, SS_NOPREFIX};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, BST_CHECKED, BST_UNCHECKED, EM_SETLIMITTEXT, EM_SETSEL,
    ICC_BAR_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, TBM_SETPAGESIZE,
    TBM_SETPOS, TBM_SETRANGEMAX, TBM_SETRANGEMIN, TBM_SETTICFREQ, TBS_AUTOTICKS, TBS_HORZ,
    TB_THUMBTRACK, TRACKBAR_CLASS, WC_BUTTONW, WC_COMBOBOXW, WC_EDITW, WC_STATICW,
};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, SystemParametersInfoForDpi, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::Shell::{
    DefSubclassProc, FileOpenDialog, IFileOpenDialog, IShellItem, RemoveWindowSubclass,
    SHCreateItemFromParsingName, SetWindowSubclass, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, FlashWindowEx, GetCursorPos, GetDlgItem,
    GetForegroundWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsDialogMessageW, IsIconic,
    IsWindow, KillTimer, LoadCursorW, LoadIconW, MoveWindow, RegisterClassW, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowPos, SetWindowTextW, ShowWindow, BM_GETCHECK,
    BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_GROUPBOX, BS_PUSHBUTTON,
    CBN_SELCHANGE, CBS_DROPDOWNLIST, CB_ADDSTRING, CB_ERR, CB_GETCURSEL, CB_SETCURSEL,
    EN_CHANGE, EN_KILLFOCUS, ES_AUTOHSCROLL, FLASHWINFO, FLASHW_ALL, HMENU, IDCANCEL,
    IDC_ARROW, IDOK, MB_OK, MSG, SPI_GETNONCLIENTMETRICS, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_HIDE, SW_RESTORE, SW_SHOW, WM_CHAR, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_DPICHANGED,
    WM_HSCROLL, WM_SETFONT, WM_TIMER, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL, WINDOW_EX_STYLE,
    WINDOW_STYLE,
};

use crate::config::{self, Config, Format};
use crate::constants;
use crate::save;
use crate::startup;
use crate::toast;

/// `TBM_GETPOS` is not exported by `windows` 0.62.2 (grep of the whole
/// crate finds nothing); declared locally per commctrl.h (`WM_USER + 0`),
/// consistent with the crate's own `TBM_SETPOS = WM_USER+5 = 1029`.
///
/// Not yet consumed: the `WM_HSCROLL` read-back handler arrives in plan
/// 04-04. Manually confirmed the slider (range 50-100, tick 10, page 5)
/// returns a value inside that range when read back with this constant.
#[allow(dead_code)]
const TBM_GETPOS: u32 = 0x0400;

/// Resource ordinal of the embedded app icon (`resources/app.rc`:
/// `IDI_APPICON 1`), mirroring `tray.rs`'s constant of the same value --
/// the Settings window uses the same crow icon, not a second copy.
const IDI_APPICON: u16 = 1;

/// Combined `WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU` -- no resize-grip
/// style and no min/max boxes -- fixed-size window (D-40, D-44).
const WINDOW_STYLE_FLAGS: WINDOW_STYLE =
    WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0);

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
    /// True for the duration of the modal `IFileOpenDialog::Show` call
    /// (Pitfall 11): `open_or_focus()` is a no-op while this is set, so an
    /// Alt+F9 press during the picker cannot try to foreground a disabled
    /// owner window.
    picker_open: bool,
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
        let background = HBRUSH(
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
    style: WINDOW_STYLE,
    ex: WINDOW_EX_STYLE,
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

/// Computes the top-level window rect for a saved `(cfg.window_x,
/// cfg.window_y)` top-left position, at the DPI of the monitor under that
/// point, validated against currently-connected monitors (D-window-position,
/// UI-01). Returns `None` whenever either coordinate is unset, or the saved
/// position no longer intersects any connected monitor (Pitfall 5) -- the
/// caller falls back to the existing cursor-monitor-centered placement
/// (D-40) in either case.
fn saved_position_rect_and_dpi(cfg: &Config) -> Option<(RECT, u32)> {
    let (x, y) = (cfg.window_x?, cfg.window_y?);

    let pt = POINT { x, y };
    let hmon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    let (mut dx, mut dy) = (96u32, 96u32);
    unsafe {
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    }

    let mut r = RECT {
        left: 0,
        top: 0,
        right: scale(constants::SETTINGS_CLIENT_W, dx),
        bottom: scale(constants::SETTINGS_CLIENT_H, dx),
    };
    unsafe {
        let _ = AdjustWindowRectExForDpi(&mut r, WINDOW_STYLE_FLAGS, false, Default::default(), dx);
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let rect = RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };

    let validate_hmon = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) };
    if validate_hmon.0.is_null() {
        return None;
    }

    Some((rect, dx))
}

/// Builds the system message font (`SPI_GETNONCLIENTMETRICS.lfMessageFont`)
/// for `dpi` -- the dialog-convention font (never a hardcoded "Segoe UI"
/// font-by-name create call, that is the toast/overlay convention).
fn message_font_for_dpi(dpi: u32) -> HFONT {
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
    // Pitfall 11: the folder picker's modal loop disables its owner
    // window; foregrounding it here would just fail. No-op instead of
    // fighting the picker for activation.
    if with_state(|d| d.picker_open).unwrap_or(false) {
        return;
    }
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

    let (rect, dpi) = match saved_position_rect_and_dpi(&cfg) {
        Some((rect, dpi)) => (rect, dpi),
        None => {
            let (work, dpi) = target_monitor_rect_and_dpi();
            let rect = window_rect_centered(
                constants::SETTINGS_CLIENT_W,
                constants::SETTINGS_CLIENT_H,
                work,
                dpi,
                WINDOW_STYLE_FLAGS,
                Default::default(),
            );
            (rect, dpi)
        }
    };

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
        picker_open: false,
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
// Child controls / layout (UI-SPEC Layout Contract)
// ---------------------------------------------------------------------
//
// Decorative, non-addressable ids for group boxes and plain labels -- not
// part of the `ID_*` control set in constants.rs (those are only the 15
// interactive/addressable controls), kept well clear of the 100-114 range
// and of IDOK(1)/IDCANCEL(2).
const LBL_BASE: i32 = 150;
const LBL_FOLDER: i32 = 151;
const LBL_FORMAT: i32 = 152;
const LBL_CLICK_ACTION: i32 = 153;
const GRP_FILENAME: i32 = 154;
const GRP_FORMAT: i32 = 155;
const GRP_BEHAVIOR: i32 = 156;

/// One row of the UI-SPEC Layout Contract table: control id, class,
/// caption, extra style bits (beyond `WS_CHILD | WS_VISIBLE` and the
/// `tabstop`-driven `WS_TABSTOP`), and the 96-DPI logical rect. This table
/// -- not measured text -- is the single source of truth `layout` scales
/// from; it is never recomputed from text extents (UI-SPEC).
struct Spec {
    id: i32,
    class: PCWSTR,
    caption: &'static str,
    extra_style: u32,
    tabstop: bool,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    /// Height passed to `CreateWindowExW`/`MoveWindow` instead of `h` for
    /// combo boxes, so the closed-state visual height (`h`) stays correct
    /// while the drop-down list has room (UI-SPEC Spacing Scale exception).
    create_h: Option<i32>,
}

/// Table order = creation order = tab order (UI-SPEC "Tab order = creation
/// order"): each group box is listed immediately before the controls it
/// visually contains (Pitfall 4).
fn ctrl_specs() -> Vec<Spec> {
    vec![
        Spec { id: GRP_FILENAME, class: WC_BUTTONW, caption: "File naming", extra_style: BS_GROUPBOX as u32, tabstop: false, x: 12, y: 12, w: 376, h: 148, create_h: None },
        Spec { id: LBL_BASE, class: WC_STATICW, caption: "&Base filename:", extra_style: SS_LEFT.0, tabstop: false, x: 24, y: 40, w: 88, h: 16, create_h: None },
        Spec { id: constants::ID_BASE_EDIT, class: WC_EDITW, caption: "", extra_style: WS_BORDER.0 | ES_AUTOHSCROLL as u32, tabstop: true, x: 120, y: 36, w: 256, h: 24, create_h: None },
        Spec { id: constants::ID_BASE_HINT, class: WC_STATICW, caption: "", extra_style: SS_LEFT.0 | SS_NOPREFIX.0, tabstop: false, x: 120, y: 64, w: 256, h: 16, create_h: None },
        Spec { id: LBL_FOLDER, class: WC_STATICW, caption: "&Save folder:", extra_style: SS_LEFT.0, tabstop: false, x: 24, y: 88, w: 88, h: 16, create_h: None },
        Spec { id: constants::ID_FOLDER_EDIT, class: WC_EDITW, caption: "", extra_style: WS_BORDER.0 | ES_AUTOHSCROLL as u32, tabstop: true, x: 120, y: 84, w: 176, h: 24, create_h: None },
        Spec { id: constants::ID_BROWSE_BTN, class: WC_BUTTONW, caption: "B&rowse\u{2026}", extra_style: BS_PUSHBUTTON as u32, tabstop: true, x: 304, y: 84, w: 72, h: 24, create_h: None },
        Spec { id: constants::ID_FOLDER_HINT, class: WC_STATICW, caption: "", extra_style: SS_NOPREFIX.0, tabstop: false, x: 120, y: 112, w: 256, h: 16, create_h: None },
        Spec { id: constants::ID_PREVIEW, class: WC_STATICW, caption: "", extra_style: SS_NOPREFIX.0, tabstop: false, x: 120, y: 132, w: 256, h: 16, create_h: None },
        Spec { id: GRP_FORMAT, class: WC_BUTTONW, caption: "Format", extra_style: BS_GROUPBOX as u32, tabstop: false, x: 12, y: 176, w: 376, h: 92, create_h: None },
        Spec { id: LBL_FORMAT, class: WC_STATICW, caption: "&Format:", extra_style: SS_LEFT.0, tabstop: false, x: 24, y: 204, w: 88, h: 16, create_h: None },
        Spec { id: constants::ID_FORMAT_COMBO, class: WC_COMBOBOXW, caption: "", extra_style: CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0, tabstop: true, x: 120, y: 200, w: 96, h: 24, create_h: Some(124) },
        Spec { id: constants::ID_QUALITY_LABEL, class: WC_STATICW, caption: "JPG &quality:", extra_style: SS_LEFT.0, tabstop: false, x: 24, y: 236, w: 88, h: 16, create_h: None },
        Spec { id: constants::ID_QUALITY_SLIDER, class: TRACKBAR_CLASS, caption: "", extra_style: TBS_HORZ | TBS_AUTOTICKS, tabstop: true, x: 120, y: 232, w: 200, h: 24, create_h: None },
        Spec { id: constants::ID_QUALITY_VALUE, class: WC_STATICW, caption: "", extra_style: SS_LEFT.0, tabstop: false, x: 328, y: 236, w: 48, h: 16, create_h: None },
        Spec { id: GRP_BEHAVIOR, class: WC_BUTTONW, caption: "Behavior", extra_style: BS_GROUPBOX as u32, tabstop: false, x: 12, y: 284, w: 376, h: 192, create_h: None },
        Spec { id: constants::ID_TOAST_CHECK, class: WC_BUTTONW, caption: "Show a &toast after each save", extra_style: BS_AUTOCHECKBOX as u32, tabstop: true, x: 24, y: 308, w: 352, h: 20, create_h: None },
        Spec { id: LBL_CLICK_ACTION, class: WC_STATICW, caption: "&When clicking the save toast:", extra_style: SS_LEFT.0, tabstop: false, x: 24, y: 340, w: 176, h: 16, create_h: None },
        Spec { id: constants::ID_CLICK_ACTION_COMBO, class: WC_COMBOBOXW, caption: "", extra_style: CBS_DROPDOWNLIST as u32, tabstop: true, x: 208, y: 336, w: 168, h: 24, create_h: Some(104) },
        Spec { id: constants::ID_CLIPBOARD_CHECK, class: WC_BUTTONW, caption: "&Copy each capture to the clipboard", extra_style: BS_AUTOCHECKBOX as u32, tabstop: true, x: 24, y: 368, w: 352, h: 20, create_h: None },
        Spec { id: constants::ID_STARTUP_CHECK, class: WC_BUTTONW, caption: "Start with &Windows", extra_style: BS_AUTOCHECKBOX as u32, tabstop: true, x: 24, y: 396, w: 352, h: 20, create_h: None },
        Spec { id: constants::ID_STARTUP_HINT, class: WC_STATICW, caption: "", extra_style: SS_NOPREFIX.0, tabstop: false, x: 24, y: 420, w: 352, h: 16, create_h: None },
        Spec { id: constants::ID_SHUTTER_CHECK, class: WC_BUTTONW, caption: "Play a &shutter sound on capture", extra_style: BS_AUTOCHECKBOX as u32, tabstop: true, x: 24, y: 444, w: 352, h: 20, create_h: None },
        Spec { id: constants::ID_DONE_BTN, class: WC_BUTTONW, caption: "&Done", extra_style: BS_PUSHBUTTON as u32, tabstop: true, x: 300, y: 488, w: 88, h: 28, create_h: None },
    ]
}

/// Creates one themed child control, applies the DPI-correct message font
/// via `WM_SETFONT`, and returns its `HWND`. The wide caption buffer is
/// bound to a local (`wide`) that outlives the `CreateWindowExW` call.
unsafe fn create_child(
    parent: HWND,
    class: PCWSTR,
    caption: &str,
    style: WINDOW_STYLE,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font: HFONT,
    hinstance: HINSTANCE,
) -> HWND {
    let wide = constants::to_wide(caption);
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            class,
            PCWSTR(wide.as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            x,
            y,
            w,
            h,
            Some(parent),
            Some(HMENU(id as usize as *mut c_void)),
            Some(hinstance),
            None,
        )
    }
    .expect("CreateWindowExW child");
    unsafe {
        let _ = SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
    }
    hwnd
}

/// Creates every child control from the UI-SPEC layout table, in table
/// (= tab) order, and returns the addressable (`ID_*`-keyed) and full
/// child-HWND lists.
fn create_children(hwnd: HWND, dpi: u32, font: HFONT) -> (HashMap<i32, isize>, Vec<isize>) {
    let hinstance: HINSTANCE = unsafe {
        GetModuleHandleW(None).expect("GetModuleHandleW failed").into()
    };
    let mut controls = HashMap::new();
    let mut all_children = Vec::new();

    for spec in ctrl_specs() {
        let h = spec.create_h.unwrap_or(spec.h);
        let style = WINDOW_STYLE(spec.extra_style) | if spec.tabstop { WS_TABSTOP } else { WINDOW_STYLE(0) };
        let child = unsafe {
            create_child(
                hwnd,
                spec.class,
                spec.caption,
                style,
                spec.id,
                scale(spec.x, dpi),
                scale(spec.y, dpi),
                scale(spec.w, dpi),
                scale(h, dpi),
                font,
                hinstance,
            )
        };

        match spec.id {
            id if id == constants::ID_BASE_EDIT => unsafe {
                let _ = SendMessageW(child, EM_SETLIMITTEXT, Some(WPARAM(64)), None);
                // Keystroke filter (D-45, T-04-16): swallows invalid chars
                // before they render, flicker-free. Removed in WM_DESTROY.
                let _ = SetWindowSubclass(child, Some(base_edit_subclass), BASE_EDIT_SUBCLASS_ID, 0);
            },
            id if id == constants::ID_FOLDER_EDIT => unsafe {
                let _ = SendMessageW(child, EM_SETLIMITTEXT, Some(WPARAM(260)), None);
            },
            id if id == constants::ID_QUALITY_SLIDER => unsafe {
                let _ = SendMessageW(child, TBM_SETRANGEMIN, Some(WPARAM(0)), Some(LPARAM(50)));
                let _ = SendMessageW(child, TBM_SETRANGEMAX, Some(WPARAM(1)), Some(LPARAM(100)));
                let _ = SendMessageW(child, TBM_SETTICFREQ, Some(WPARAM(10)), None);
                let _ = SendMessageW(child, TBM_SETPAGESIZE, Some(WPARAM(0)), Some(LPARAM(5)));
            },
            _ => {}
        }

        controls.insert(spec.id, child.0 as isize);
        all_children.push(child.0 as isize);
    }

    (controls, all_children)
}

/// Scales and repositions every child control for `dpi` via `MoveWindow`,
/// walking the same table `create_children` used. Called once after
/// creation and again from the `WM_DPICHANGED` handler.
fn layout(hwnd: HWND, dpi: u32) {
    let _ = hwnd;
    let controls = with_state(|d| d.controls.clone()).unwrap_or_default();
    for spec in ctrl_specs() {
        let Some(&raw) = controls.get(&spec.id) else {
            continue;
        };
        let child = HWND(raw as *mut c_void);
        let h = spec.create_h.unwrap_or(spec.h);
        unsafe {
            let _ = MoveWindow(
                child,
                scale(spec.x, dpi),
                scale(spec.y, dpi),
                scale(spec.w, dpi),
                scale(h, dpi),
                true,
            );
        }
    }
}

// ---------------------------------------------------------------------
// Population (D-39, D-43, D-49, D-51)
// ---------------------------------------------------------------------

/// Fixed combo item order (UI-SPEC Copywriting Contract) -- index maps
/// 1:1 to the `Format` variant order.
const FORMAT_ITEMS: [&str; 4] = ["PNG", "JPG", "BMP", "WebP (lossless)"];

/// Fixed combo item order (UI-SPEC Copywriting Contract, D-53) -- index
/// maps 1:1 to the `ClickAction` variant order.
const CLICK_ACTION_ITEMS: [&str; 3] = ["Dismiss", "Open file", "Reveal in Explorer"];

fn format_index(fmt: Format) -> usize {
    match fmt {
        Format::Png => 0,
        Format::Jpg => 1,
        Format::Bmp => 2,
        Format::Webp => 3,
    }
}

fn click_action_index(action: config::ClickAction) -> usize {
    match action {
        config::ClickAction::Dismiss => 0,
        config::ClickAction::OpenFile => 1,
        config::ClickAction::RevealExplorer => 2,
    }
}

/// `SetWindowTextW` on the control addressed by `id`, if it exists.
fn set_text(hwnd: HWND, id: i32, text: &str) {
    if let Ok(ctrl) = unsafe { GetDlgItem(Some(hwnd), id) } {
        let wide = constants::to_wide(text);
        unsafe {
            let _ = SetWindowTextW(ctrl, PCWSTR(wide.as_ptr()));
        }
    }
}

/// `BM_SETCHECK` on the checkbox addressed by `id`, if it exists.
fn set_check(hwnd: HWND, id: i32, checked: bool) {
    if let Ok(ctrl) = unsafe { GetDlgItem(Some(hwnd), id) } {
        let state = if checked { BST_CHECKED.0 } else { BST_UNCHECKED.0 };
        unsafe {
            let _ = SendMessageW(ctrl, BM_SETCHECK, Some(WPARAM(state as usize)), None);
        }
    }
}

/// Fills the format combo with the fixed item order and selects `fmt`.
/// `CB_SETCURSEL` does not emit `CBN_SELCHANGE` (safe to call while
/// `initializing`).
fn fill_format_combo(hwnd: HWND, fmt: Format) {
    if let Ok(combo) = unsafe { GetDlgItem(Some(hwnd), constants::ID_FORMAT_COMBO) } {
        for item in FORMAT_ITEMS {
            let wide = constants::to_wide(item);
            unsafe {
                let _ = SendMessageW(
                    combo,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(wide.as_ptr() as isize)),
                );
            }
        }
        unsafe {
            let _ = SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(format_index(fmt))), None);
        }
    }
}

/// Fills the toast-click-action combo (D-53) with the fixed item order and
/// selects `action`.
fn fill_click_action_combo(hwnd: HWND, action: config::ClickAction) {
    if let Ok(combo) = unsafe { GetDlgItem(Some(hwnd), constants::ID_CLICK_ACTION_COMBO) } {
        for item in CLICK_ACTION_ITEMS {
            let wide = constants::to_wide(item);
            unsafe {
                let _ = SendMessageW(
                    combo,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(wide.as_ptr() as isize)),
                );
            }
        }
        unsafe {
            let _ = SendMessageW(
                combo,
                CB_SETCURSEL,
                Some(WPARAM(click_action_index(action))),
                None,
            );
        }
    }
}

/// Shows the JPG quality label/slider/value only when `fmt` is JPG (D-43);
/// the window's fixed size means the row's space is always reserved.
/// Hidden controls are automatically skipped by `IsDialogMessageW` tab
/// navigation.
fn apply_format_visibility(hwnd: HWND, fmt: Format) {
    let cmd = if fmt == Format::Jpg { SW_SHOW } else { SW_HIDE };
    for id in [
        constants::ID_QUALITY_LABEL,
        constants::ID_QUALITY_SLIDER,
        constants::ID_QUALITY_VALUE,
    ] {
        if let Ok(ctrl) = unsafe { GetDlgItem(Some(hwnd), id) } {
            unsafe {
                let _ = ShowWindow(ctrl, cmd);
            }
        }
    }
}

/// Sets the read-only next-file preview line (D-49), sharing the save
/// pipeline's own numbering (`save::peek_next_filename`) so the preview
/// can never disagree with a real reservation.
fn refresh_preview(hwnd: HWND, cfg: &Config) {
    let text = format!("Next: {}", save::peek_next_filename(cfg));
    set_text(hwnd, constants::ID_PREVIEW, &text);
}

/// Populates every control from the single config snapshot loaded at open
/// (D-39) plus the registry for the Start-with-Windows checkbox (D-51).
/// Called once, at the end of window creation, while `initializing` is
/// still true.
fn populate(hwnd: HWND) {
    let Some(cfg) = with_state(|d| d.cfg.clone()) else {
        return;
    };

    set_text(hwnd, constants::ID_BASE_EDIT, &cfg.base_filename);
    // Never blank: the resolved default is shown even when the stored
    // value is empty or was rejected.
    let folder = config::resolve_save_folder(&cfg).to_string_lossy().into_owned();
    set_text(hwnd, constants::ID_FOLDER_EDIT, &folder);

    let fmt = Format::from_str(&cfg.format);
    fill_format_combo(hwnd, fmt);
    fill_click_action_combo(hwnd, config::ClickAction::from_str(&cfg.toast_click_action));

    if let Ok(slider) = unsafe { GetDlgItem(Some(hwnd), constants::ID_QUALITY_SLIDER) } {
        unsafe {
            let _ = SendMessageW(
                slider,
                TBM_SETPOS,
                Some(WPARAM(1)),
                Some(LPARAM(cfg.jpg_quality as isize)),
            );
        }
    }
    set_text(hwnd, constants::ID_QUALITY_VALUE, &cfg.jpg_quality.to_string());

    set_check(hwnd, constants::ID_TOAST_CHECK, cfg.toast_enabled);
    set_check(hwnd, constants::ID_CLIPBOARD_CHECK, cfg.clipboard_enabled);
    // D-51: the checkbox reflects the registry, not the config flag.
    set_check(hwnd, constants::ID_STARTUP_CHECK, startup::is_registered());
    set_check(hwnd, constants::ID_SHUTTER_CHECK, cfg.shutter_sound);

    apply_format_visibility(hwnd, fmt);
    refresh_preview(hwnd, &cfg);
}

/// Sets focus to the base filename edit with its contents selected, once
/// the window is shown and foregrounded (UI-SPEC A8).
fn set_initial_focus(hwnd: HWND) {
    if let Ok(edit) = unsafe { GetDlgItem(Some(hwnd), constants::ID_BASE_EDIT) } {
        unsafe {
            let _ = SetFocus(Some(edit));
            let _ = SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
    }
}

// ---------------------------------------------------------------------
// Instant-apply commit model (D-37, D-39, SET-03)
// ---------------------------------------------------------------------

/// Copies the current `Config` snapshot out of `SETTINGS_DATA`, drops the
/// guard, then calls `config::save`. On failure, toasts the OS error --
/// the in-memory snapshot (already mutated by the caller before this is
/// called) keeps the user's value regardless of whether the write
/// succeeded (D-39). Never call this while holding the `SETTINGS_DATA`
/// guard (`with_state`'s closure must not call `persist`).
fn persist() {
    let Some(cfg) = with_state(|d| d.cfg.clone()) else {
        return;
    };
    if let Err(e) = config::save(&cfg) {
        toast::show(&format!("Couldn't save settings: {e}"));
    }
}

/// `BM_GETCHECK` against `BST_CHECKED` on the control addressed by `id` --
/// reads the control's own current state rather than trusting message
/// parameters (T-04-23).
fn get_check(hwnd: HWND, id: i32) -> bool {
    let Ok(ctrl) = (unsafe { GetDlgItem(Some(hwnd), id) }) else {
        return false;
    };
    let state = unsafe { SendMessageW(ctrl, BM_GETCHECK, None, None) };
    state.0 as i32 == BST_CHECKED.0 as i32
}

/// `CB_GETCURSEL` on the combo addressed by `id`. `None` on `CB_ERR` (no
/// selection) so callers can ignore a spurious notification.
fn get_cursel(hwnd: HWND, id: i32) -> Option<usize> {
    let ctrl = unsafe { GetDlgItem(Some(hwnd), id) }.ok()?;
    let sel = unsafe { SendMessageW(ctrl, CB_GETCURSEL, None, None) };
    if sel.0 as i32 == CB_ERR {
        None
    } else {
        Some(sel.0 as usize)
    }
}

/// Dispatches a `WM_COMMAND` notification (high word = notification code,
/// low word = control id) once the window has finished `populate()` and is
/// not tearing down. Every control this window creates that mutates
/// `Config` is handled here.
fn handle_command(hwnd: HWND, wparam: WPARAM) {
    let notify_code = ((wparam.0 >> 16) & 0xFFFF) as u32;
    let id = (wparam.0 & 0xFFFF) as i32;

    match (notify_code, id) {
        (BN_CLICKED, cid) if cid == constants::ID_TOAST_CHECK => {
            let checked = get_check(hwnd, cid);
            with_state(|d| d.cfg.toast_enabled = checked);
            persist();
        }
        (BN_CLICKED, cid) if cid == constants::ID_CLIPBOARD_CHECK => {
            let checked = get_check(hwnd, cid);
            with_state(|d| d.cfg.clipboard_enabled = checked);
            persist();
        }
        (BN_CLICKED, cid) if cid == constants::ID_STARTUP_CHECK => {
            handle_startup_toggle(hwnd);
        }
        (BN_CLICKED, cid) if cid == constants::ID_SHUTTER_CHECK => {
            let checked = get_check(hwnd, cid);
            with_state(|d| d.cfg.shutter_sound = checked);
            persist();
        }
        (BN_CLICKED, cid) if cid == constants::ID_DONE_BTN => close(hwnd),
        (BN_CLICKED, cid) if cid == constants::ID_BROWSE_BTN => {
            handle_browse(hwnd);
        }
        (CBN_SELCHANGE, cid) if cid == constants::ID_FORMAT_COMBO => {
            let Some(idx) = get_cursel(hwnd, cid) else {
                return;
            };
            let fmt = match idx {
                1 => Format::Jpg,
                2 => Format::Bmp,
                3 => Format::Webp,
                _ => Format::Png,
            };
            let fmt_str = match fmt {
                Format::Png => "png",
                Format::Jpg => "jpg",
                Format::Bmp => "bmp",
                Format::Webp => "webp",
            };
            with_state(|d| d.cfg.format = fmt_str.to_string());
            persist();
            apply_format_visibility(hwnd, fmt);
            if let Some(cfg) = with_state(|d| d.cfg.clone()) {
                refresh_preview(hwnd, &cfg);
            }
        }
        (CBN_SELCHANGE, cid) if cid == constants::ID_CLICK_ACTION_COMBO => {
            let Some(idx) = get_cursel(hwnd, cid) else {
                return;
            };
            let action_str = match idx {
                1 => "open_file",
                2 => "reveal_explorer",
                _ => "dismiss",
            };
            with_state(|d| d.cfg.toast_click_action = action_str.to_string());
            persist();
        }
        (EN_KILLFOCUS, cid) if cid == constants::ID_BASE_EDIT => {
            commit_base_filename(hwnd);
        }
        (EN_KILLFOCUS, cid) if cid == constants::ID_FOLDER_EDIT => {
            commit_folder(hwnd);
        }
        (EN_CHANGE, cid) if cid == constants::ID_BASE_EDIT => {
            sweep_base_filename(hwnd);
        }
        _ if id == IDOK.0 => {
            // Enter (via IsDialogMessageW): commit whichever edit has
            // focus. Never closes the window, never "clicks" Browse
            // (Pitfall 2 -- no BS_DEFPUSHBUTTON exists to steal this).
            let focused = unsafe { GetFocus() };
            if let Ok(base_edit) = unsafe { GetDlgItem(Some(hwnd), constants::ID_BASE_EDIT) } {
                if focused == base_edit {
                    commit_base_filename(hwnd);
                    return;
                }
            }
            if let Ok(folder_edit) = unsafe { GetDlgItem(Some(hwnd), constants::ID_FOLDER_EDIT) } {
                if focused == folder_edit {
                    commit_folder(hwnd);
                }
            }
        }
        _ if id == IDCANCEL.0 => {
            close(hwnd);
        }
        _ => {}
    }
}

/// `WM_HSCROLL` from the JPG quality trackbar: always updates the live
/// value static, but only writes config.json when the drag has completed
/// (`LOWORD(wParam) != TB_THUMBTRACK`) -- one write per finished change,
/// not one per mouse pixel (T-04-24).
fn handle_hscroll(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let Ok(slider) = (unsafe { GetDlgItem(Some(hwnd), constants::ID_QUALITY_SLIDER) }) else {
        return;
    };
    if lparam.0 != slider.0 as isize {
        return;
    }
    let pos = unsafe { SendMessageW(slider, TBM_GETPOS, None, None) };
    let value = (pos.0 as i32).clamp(50, 100) as u8;
    set_text(hwnd, constants::ID_QUALITY_VALUE, &value.to_string());

    let code = (wparam.0 & 0xFFFF) as u32;
    if code != TB_THUMBTRACK {
        with_state(|d| d.cfg.jpg_quality = value);
        persist();
    }
}

// ---------------------------------------------------------------------
// Hints (D-45..D-48): one STATIC per field, a per-hint SetTimer/WM_TIMER
// pair reusing TOAST_DURATION_MS-equivalent timing (SETTINGS_HINT_MS).
// ---------------------------------------------------------------------

/// Sets the hint static addressed by `id` to `text` and (re)starts its
/// auto-clear timer. Re-showing a hint while its timer is already running
/// restarts the timer (`SetTimer` on an existing id replaces it).
fn show_hint(hwnd: HWND, id: i32, text: &str) {
    set_text(hwnd, id, text);
    unsafe {
        let _ = SetTimer(
            Some(hwnd),
            constants::SETTINGS_HINT_TIMER_BASE + id as usize,
            constants::SETTINGS_HINT_MS,
            None,
        );
    }
}

/// `MessageBeep(MB_OK)` plus the filename hint (D-45 copy, verbatim).
/// Reads the process-global Settings HWND rather than taking a parameter,
/// so both the `WM_CHAR` subclass (whose own `hwnd` is the edit, not the
/// parent) and the `EN_CHANGE` sweep can share one implementation.
fn reject_feedback() {
    unsafe {
        let _ = MessageBeep(MB_OK);
    }
    if let Some(hwnd) = current_hwnd() {
        show_hint(
            hwnd,
            constants::ID_BASE_HINT,
            "Not allowed: \\ / : * ? \" < > | .",
        );
    }
}

/// Unique id passed to `SetWindowSubclass`/`RemoveWindowSubclass` for the
/// base filename edit's keystroke filter (Pattern 7).
const BASE_EDIT_SUBCLASS_ID: usize = 1;

/// `WM_CHAR` subclass on the base filename edit (D-45, T-04-16): blocks
/// only printable invalid characters before they are ever inserted.
/// Control characters (Backspace, Ctrl+A/C/V/X/Z) always pass through --
/// `is_control()` is checked first -- so editing shortcuts keep working.
unsafe extern "system" fn base_edit_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _ref: usize,
) -> LRESULT {
    if msg == WM_CHAR {
        if let Some(c) = char::from_u32(wparam.0 as u32) {
            if !c.is_control() && config::is_invalid_filename_char(c) {
                reject_feedback();
                return LRESULT(0);
            }
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// `EN_CHANGE` sweep (T-04-17): covers paste, drag-drop, and IME
/// composition, none of which are seen by the `WM_CHAR` subclass. Never
/// writes config -- D-37 reserves that for commit (kill-focus/Enter/close).
fn sweep_base_filename(hwnd: HWND) {
    let text = read_text(hwnd, constants::ID_BASE_EDIT);
    if text.chars().all(|c| !config::is_invalid_filename_char(c)) {
        return;
    }
    let filtered: String = text
        .chars()
        .filter(|c| !config::is_invalid_filename_char(*c))
        .collect();
    set_text(hwnd, constants::ID_BASE_EDIT, &filtered);
    if let Ok(edit) = unsafe { GetDlgItem(Some(hwnd), constants::ID_BASE_EDIT) } {
        let len = unsafe { GetWindowTextLengthW(edit) };
        unsafe {
            let _ = SendMessageW(
                edit,
                EM_SETSEL,
                Some(WPARAM(len.max(0) as usize)),
                Some(LPARAM(len as isize)),
            );
        }
    }
    reject_feedback();
}

/// Commits the base filename edit (`EN_KILLFOCUS`, `IDOK` while focused,
/// or the close path): empty/whitespace-only reverts to `last_base` with a
/// hint and does not persist (D-46 -- `corvus` is never substituted here).
/// Interior whitespace is preserved, matching `sanitize_base_filename`.
/// The text is run through `config::is_invalid_filename_char` first (D-45,
/// the single shared authority), so invalid characters from a hand-edited
/// config -- populated while `initializing` gated the `EN_CHANGE` sweep --
/// can't round-trip back into config.json.
fn commit_base_filename(hwnd: HWND) {
    let text = read_text(hwnd, constants::ID_BASE_EDIT);
    let cleaned: String = text
        .chars()
        .filter(|c| !config::is_invalid_filename_char(*c))
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        let last = with_state(|d| d.last_base.clone()).unwrap_or_default();
        set_text(hwnd, constants::ID_BASE_EDIT, &last);
        show_hint(hwnd, constants::ID_BASE_HINT, "Name can't be empty");
        return;
    }
    let trimmed = trimmed.to_string();
    if cleaned != text {
        // Filtering removed invalid characters: reflect the committed
        // value in the edit so the field and config.json agree.
        set_text(hwnd, constants::ID_BASE_EDIT, &trimmed);
    }
    // D-37: persist only on a real change. Tab-through, the close-funnel
    // double commit, and Esc on a defaults-loaded (malformed, D-13) config
    // must not rewrite the user's file.
    let changed = with_state(|d| {
        if d.last_base == trimmed {
            return false;
        }
        d.cfg.base_filename = trimmed.clone();
        d.last_base = trimmed.clone();
        true
    })
    .unwrap_or(false);
    if changed {
        persist();
        if let Some(cfg) = with_state(|d| d.cfg.clone()) {
            refresh_preview(hwnd, &cfg);
        }
    }
}

/// Commits the save folder edit (`EN_KILLFOCUS`, `IDOK` while focused, or
/// the close path). Rejects on the same rules `config::validate_save_folder`
/// enforces, then probes creatability; config.json is never written with a
/// path that failed either check (D-47/D-48).
fn commit_folder(hwnd: HWND) {
    let text = read_text(hwnd, constants::ID_FOLDER_EDIT);
    let trimmed = text.trim();
    let revert = |hwnd: HWND, msg: &str| {
        let last = with_state(|d| d.last_folder.clone()).unwrap_or_default();
        set_text(hwnd, constants::ID_FOLDER_EDIT, &last);
        show_hint(hwnd, constants::ID_FOLDER_HINT, msg);
    };
    let path = match config::validate_save_folder(trimmed) {
        Ok(path) => path,
        Err(e) => {
            revert(hwnd, &e.to_string());
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&path) {
        revert(hwnd, &format!("Couldn't create folder: {e}"));
        return;
    }
    let path_str = path.to_string_lossy().into_owned();
    // D-37: persist only on a real change (see commit_base_filename).
    let changed = with_state(|d| {
        if d.last_folder == path_str {
            return false;
        }
        d.cfg.save_folder = path_str.clone();
        d.last_folder = path_str.clone();
        true
    })
    .unwrap_or(false);
    if changed {
        persist();
        if let Some(cfg) = with_state(|d| d.cfg.clone()) {
            refresh_preview(hwnd, &cfg);
        }
    }
}

// ---------------------------------------------------------------------
// Browse folder picker (D-47, Pattern 6) and Start with Windows (D-50..D-52)
// ---------------------------------------------------------------------

/// Shows the shell folder picker owned by `owner`, seeded at `initial` when
/// that resolves. Returns `None` on cancel (a `Show` `Err` -- silent no-op
/// per UI-SPEC A10) or any COM failure along the way. The `PWSTR` from
/// `GetDisplayName` is converted to a Rust `String` BEFORE `CoTaskMemFree`
/// and always freed, even when conversion fails (Pitfall 9, T-04-22).
fn pick_folder(owner: HWND, initial: &str) -> Option<std::path::PathBuf> {
    unsafe {
        let dlg: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().unwrap_or_default();
        dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        let title = constants::to_wide("Choose capture folder");
        let _ = dlg.SetTitle(PCWSTR(title.as_ptr()));
        if !initial.is_empty() {
            let wide = constants::to_wide(initial);
            if let Ok(item) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide.as_ptr()), None)
            {
                let _ = dlg.SetFolder(&item);
            }
        }
        dlg.Show(Some(owner)).ok()?;
        let item = dlg.GetResult().ok()?;
        let pw = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = pw.to_string().ok();
        CoTaskMemFree(Some(pw.0 as *const c_void));
        s.map(std::path::PathBuf::from)
    }
}

/// `BN_CLICKED` on Browse: shows the picker owned by this window, guarded
/// by `picker_open` (Pitfall 11), and runs the result through the same
/// `commit_folder` path typed text uses so creatability is checked in
/// exactly one place (D-47).
fn handle_browse(hwnd: HWND) {
    let initial = with_state(|d| d.last_folder.clone()).unwrap_or_default();
    with_state(|d| d.picker_open = true);
    let result = pick_folder(hwnd, &initial);
    with_state(|d| d.picker_open = false);
    if let Some(path) = result {
        set_text(hwnd, constants::ID_FOLDER_EDIT, &path.to_string_lossy());
        commit_folder(hwnd);
    }
}

/// `BN_CLICKED` on Start with Windows: reads the control's own new state,
/// calls `startup::set_enabled`. On failure the checkbox reverts to the
/// actual registry state and the OS error shows next to it; nothing is
/// persisted as enabled (D-52). On success the config snapshot picks up the user's
/// intent for the next launch's self-heal (D-50).
fn handle_startup_toggle(hwnd: HWND) {
    let checked = get_check(hwnd, constants::ID_STARTUP_CHECK);
    match startup::set_enabled(checked) {
        Ok(()) => {
            with_state(|d| d.cfg.start_with_windows = checked);
            persist();
        }
        Err(e) => {
            // Revert to the registry-derived state (D-51): a failed
            // *disable* leaves the Run value in place, so a hardcoded
            // `false` would display the opposite of the truth.
            set_check(hwnd, constants::ID_STARTUP_CHECK, startup::is_registered());
            show_hint(hwnd, constants::ID_STARTUP_HINT, &e.to_string());
        }
    }
}

// ---------------------------------------------------------------------
// Window procedure
// ---------------------------------------------------------------------

/// The single cancel/close funnel: sets `closing` then commits both edits
/// (Pitfall 3 -- `WM_COMMAND` dispatch is gated on `!closing`, so no
/// `EN_KILLFOCUS` fired by the coming `DestroyWindow` can double-commit or
/// flash a hint) before destroying the window. Idempotent -- a second call
/// while already closing is a no-op.
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
    commit_base_filename(hwnd);
    commit_folder(hwnd);

    // D-window-position/UI-01: capture the window's current top-left and
    // persist only when it actually changed (skip-unchanged, WR-04
    // precedent) -- never persist on every close.
    let mut rect = RECT::default();
    let got_rect = unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok();
    if got_rect {
        let changed = with_state(|d| {
            if d.cfg.window_x == Some(rect.left) && d.cfg.window_y == Some(rect.top) {
                return false;
            }
            d.cfg.window_x = Some(rect.left);
            d.cfg.window_y = Some(rect.top);
            true
        })
        .unwrap_or(false);
        if changed {
            persist();
        }
    }

    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

// ---------------------------------------------------------------------
// Dev-only: --settings-selftest (main.rs)
// ---------------------------------------------------------------------

fn current_hwnd() -> Option<HWND> {
    SETTINGS_HWND.lock().unwrap().map(|raw| HWND(raw as *mut c_void))
}

/// Closes the currently-open Settings window immediately, mirroring
/// `overlay.rs`'s `close_active` dev affordance.
fn close_active() {
    if let Some(hwnd) = current_hwnd() {
        close(hwnd);
    }
}

/// Reads back a control's current text via `GetDlgItem` + `GetWindowTextW`.
/// Empty string if the control doesn't exist or has no text.
fn read_text(hwnd: HWND, id: i32) -> String {
    let Ok(ctrl) = (unsafe { GetDlgItem(Some(hwnd), id) }) else {
        return String::new();
    };
    let len = unsafe { GetWindowTextLengthW(ctrl) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len as usize) + 1];
    let n = unsafe { GetWindowTextW(ctrl, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Every addressable (`ID_*`) control the window creates -- the exact set
/// `--settings-selftest` proves exists via `GetDlgItem`.
const ALL_CONTROL_IDS: [i32; 17] = [
    constants::ID_BASE_EDIT,
    constants::ID_BASE_HINT,
    constants::ID_FOLDER_EDIT,
    constants::ID_FOLDER_HINT,
    constants::ID_BROWSE_BTN,
    constants::ID_PREVIEW,
    constants::ID_FORMAT_COMBO,
    constants::ID_QUALITY_LABEL,
    constants::ID_QUALITY_SLIDER,
    constants::ID_QUALITY_VALUE,
    constants::ID_TOAST_CHECK,
    constants::ID_CLICK_ACTION_COMBO,
    constants::ID_CLIPBOARD_CHECK,
    constants::ID_STARTUP_CHECK,
    constants::ID_STARTUP_HINT,
    constants::ID_SHUTTER_CHECK,
    constants::ID_DONE_BTN,
];

/// Dev-only (`--settings-selftest`, main.rs): opens the window, asserts
/// every `ID_*` control exists via `GetDlgItem`, and returns a one-line
/// summary (resolved DPI, scaled client size, preview text) on success or
/// a failure reason. Always closes the window before returning.
pub fn run_selftest() -> Result<String, String> {
    open_or_focus();
    let result = (|| {
        let hwnd = current_hwnd().ok_or_else(|| "Settings window did not open".to_string())?;
        for id in ALL_CONTROL_IDS {
            unsafe { GetDlgItem(Some(hwnd), id) }
                .map_err(|_| format!("control id {id} not found"))?;
        }
        let dpi = with_state(|d| d.dpi).ok_or_else(|| "no DPI recorded".to_string())?;
        let w = scale(constants::SETTINGS_CLIENT_W, dpi);
        let h = scale(constants::SETTINGS_CLIENT_H, dpi);
        let preview = read_text(hwnd, constants::ID_PREVIEW);
        Ok(format!("dpi={dpi} client={w}x{h} preview=\"{preview}\""))
    })();
    close_active();
    result
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            close(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            let initializing_or_closing =
                with_state(|d| d.initializing || d.closing).unwrap_or(true);
            if !initializing_or_closing {
                handle_command(hwnd, wparam);
            } else {
                // Even while initializing/closing, IDCANCEL (Esc) must
                // still close the window.
                let id = (wparam.0 & 0xFFFF) as i32;
                if id == IDCANCEL.0 {
                    close(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_HSCROLL => {
            let initializing = with_state(|d| d.initializing).unwrap_or(true);
            if !initializing {
                handle_hscroll(hwnd, wparam, lparam);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            let timer_id = wparam.0;
            for id in [
                constants::ID_BASE_HINT,
                constants::ID_FOLDER_HINT,
                constants::ID_STARTUP_HINT,
            ] {
                if timer_id == constants::SETTINGS_HINT_TIMER_BASE + id as usize {
                    set_text(hwnd, id, "");
                    unsafe {
                        let _ = KillTimer(Some(hwnd), timer_id);
                    }
                    break;
                }
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // T-01-04: never dereference a caller-supplied pointer without
            // a guard. Any local process can forge this message with
            // lparam = 0; ignore it rather than crash the tray app.
            let rect_ptr = lparam.0 as *const RECT;
            if rect_ptr.is_null() {
                return LRESULT(0);
            }
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
                            HFONT(old as *mut c_void).into(),
                        );
                    }
                }
            }
            layout(hwnd, new_dpi);
            let suggested = unsafe { *rect_ptr };
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
            // Single cleanup point (Pitfall 10): remove the keystroke
            // filter subclass, free the font, kill any pending hint
            // timers, clear the process-global HWND.
            if let Ok(base_edit) = unsafe { GetDlgItem(Some(hwnd), constants::ID_BASE_EDIT) } {
                unsafe {
                    let _ = RemoveWindowSubclass(
                        base_edit,
                        Some(base_edit_subclass),
                        BASE_EDIT_SUBCLASS_ID,
                    );
                }
            }
            let data = SETTINGS_DATA.lock().unwrap().take();
            if let Some(d) = data {
                if d.font != 0 {
                    unsafe {
                        let _ = DeleteObject(
                            HFONT(d.font as *mut c_void).into(),
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
