//! Shift+F9 region-selection overlay (REG-01..06).
//!
//! This module owns the pure, HWND-free selection-geometry core (types and
//! functions proven correct by unit tests before any window exists) plus,
//! from this plan onward, the window itself: class registration, the
//! `open()`/`cancel()`/`is_active()` lifecycle, the frozen/dimmed/back-buffer
//! DIBs, and the `WM_PAINT` renderer. Input handling (drag/resize/keyboard,
//! confirm/cancel wiring) arrives in Plans 03-04.

// Selection-state variants (`Moving`, `Resizing`) and a few geometry helpers
// have no call site outside tests until Plans 03-04 wire real mouse/keyboard
// input -- allow dead_code at module level rather than annotate each one.
#![allow(dead_code)]

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows::core::{Result as WinResult, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AlphaBlend, BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject,
    InvalidateRect, Rectangle, RoundRect, SelectObject, SetBkMode, SetTextColor, AC_SRC_OVER,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DEFAULT_QUALITY, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, FF_DONTCARE, FW_NORMAL, NULL_BRUSH, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID,
    SRCCOPY, TRANSPARENT,
};
#[cfg(debug_assertions)]
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetForegroundWindow, RegisterClassW,
    SetForegroundWindow, ShowWindow, CS_DBLCLKS, SW_SHOW, WM_DESTROY, WM_ERASEBKGND, WM_PAINT,
    WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::capture::{self, RawBitmap};
use crate::constants;

// ---------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------

/// One of the 8 resize handles arranged around a selection's corners and
/// edge midpoints (REG-03).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Handle {
    NW,
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
}

/// Result of hit-testing a point against a selection (REG-03). Handles are
/// tested before the interior, so a handle near the border always wins.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    Inside,
    Handle(Handle),
}

/// Overlay interaction state (RESEARCH Pattern 3). Drives both drag
/// semantics and the `WM_SETCURSOR` cursor table in later plans.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayState {
    /// No selection exists yet; crosshair cursor.
    Idle,
    /// Rubber-banding a brand-new selection from `anchor`.
    DraggingNew { anchor: POINT },
    /// A selection exists; handles + readout are visible.
    Selected,
    /// Dragging the selection interior to a new position.
    Moving { grab_offset: POINT },
    /// Dragging one resize handle; the opposite corner/edge is the anchor.
    Resizing { handle: Handle, anchor: POINT },
}

// ---------------------------------------------------------------------
// Pure geometry helpers
// ---------------------------------------------------------------------

/// Normalizes two arbitrary drag endpoints into a valid (non-negative
/// width/height) RECT. REG-03: the user can drag in any of the four
/// directions, so the anchor is not necessarily the top-left corner.
pub fn normalize_rect(a: POINT, b: POINT) -> RECT {
    RECT {
        left: a.x.min(b.x),
        top: a.y.min(b.y),
        right: a.x.max(b.x),
        bottom: a.y.max(b.y),
    }
}

/// Returns the 8 resize-handle rects for `sel`, each `size` pixels square,
/// centered on the 4 corners and 4 edge midpoints (D-25).
pub fn handle_rects(sel: RECT, size: i32) -> [(Handle, RECT); 8] {
    let half = size / 2;
    let cx = (sel.left + sel.right) / 2;
    let cy = (sel.top + sel.bottom) / 2;

    let square = |cx: i32, cy: i32| RECT {
        left: cx - half,
        top: cy - half,
        right: cx - half + size,
        bottom: cy - half + size,
    };

    [
        (Handle::NW, square(sel.left, sel.top)),
        (Handle::N, square(cx, sel.top)),
        (Handle::NE, square(sel.right, sel.top)),
        (Handle::E, square(sel.right, cy)),
        (Handle::SE, square(sel.right, sel.bottom)),
        (Handle::S, square(cx, sel.bottom)),
        (Handle::SW, square(sel.left, sel.bottom)),
        (Handle::W, square(sel.left, cy)),
    ]
}

fn point_in_rect(p: POINT, r: RECT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

/// Hit-tests `p` against `sel` (REG-03): handles first (inflated to
/// `hit_size`, not the smaller visual size), then the interior, then
/// `Outside`. Handle-first ordering lets a handle near the border win over
/// the interior.
pub fn hit_test(sel: RECT, p: POINT, hit_size: i32) -> Hit {
    for (handle, rect) in handle_rects(sel, hit_size) {
        if point_in_rect(p, rect) {
            return Hit::Handle(handle);
        }
    }
    if point_in_rect(p, sel) {
        return Hit::Inside;
    }
    Hit::Outside
}

/// Resizes `original` by dragging `handle` from `anchor` (the fixed
/// opposite corner/edge) to `floating` (the current mouse point). Edge
/// handles (N/S/E/W) only move their perpendicular axis; the other axis
/// stays at `original`'s bounds. Always returns a normalized rect, so
/// dragging a corner handle past its anchor re-normalizes cleanly.
pub fn resize_rect(anchor: POINT, floating: POINT, handle: Handle, original: RECT) -> RECT {
    let (p1, p2) = match handle {
        Handle::NW | Handle::NE | Handle::SE | Handle::SW => (anchor, floating),
        Handle::N | Handle::S => (
            POINT {
                x: original.left,
                y: anchor.y,
            },
            POINT {
                x: original.right,
                y: floating.y,
            },
        ),
        Handle::E | Handle::W => (
            POINT {
                x: anchor.x,
                y: original.top,
            },
            POINT {
                x: floating.x,
                y: original.bottom,
            },
        ),
    };
    normalize_rect(p1, p2)
}

/// Pushes `sel` fully inside `bounds`, preserving width/height when they
/// already fit. Never returns a rect smaller than 1x1 (a degenerate
/// `bounds` still yields a valid, if minimal, rect).
pub fn clamp_rect(sel: RECT, bounds: RECT) -> RECT {
    let bw = (bounds.right - bounds.left).max(1);
    let bh = (bounds.bottom - bounds.top).max(1);
    let w = (sel.right - sel.left).max(1).min(bw);
    let h = (sel.bottom - sel.top).max(1).min(bh);

    let mut left = sel.left;
    let mut top = sel.top;
    if left < bounds.left {
        left = bounds.left;
    }
    if left + w > bounds.right {
        left = bounds.right - w;
    }
    if top < bounds.top {
        top = bounds.top;
    }
    if top + h > bounds.bottom {
        top = bounds.bottom - h;
    }

    RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

/// Moves `sel` by `(dx, dy)` (D-30 arrow keys), stopping at the edge of
/// `bounds` instead of walking the selection off-screen.
pub fn nudge_rect(sel: RECT, dx: i32, dy: i32, bounds: RECT) -> RECT {
    let w = sel.right - sel.left;
    let h = sel.bottom - sel.top;
    let max_left = (bounds.right - w).max(bounds.left);
    let max_top = (bounds.bottom - h).max(bounds.top);
    let left = (sel.left + dx).clamp(bounds.left, max_left);
    let top = (sel.top + dy).clamp(bounds.top, max_top);
    RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

/// Grows/shrinks `sel` by moving only the bottom-right edge (D-30
/// Shift+arrows), floored at 1x1 (D-31 -- no minimum-size gate above 1) and
/// clamped so the edge never leaves `bounds`.
pub fn grow_rect(sel: RECT, dx: i32, dy: i32, bounds: RECT) -> RECT {
    let right = (sel.right + dx).clamp(sel.left + 1, bounds.right.max(sel.left + 1));
    let bottom = (sel.bottom + dy).clamp(sel.top + 1, bounds.bottom.max(sel.top + 1));
    RECT {
        left: sel.left,
        top: sel.top,
        right,
        bottom,
    }
}

/// Places the dimension-readout chip (D-27/D-28): `OVERLAY_READOUT_GAP`
/// pixels outside the selection's bottom-right corner by default; flips to
/// hug the inside of that corner when the default placement would cross
/// `bounds`' right/bottom edge within `OVERLAY_READOUT_EDGE_THRESHOLD`.
pub fn readout_rect(sel: RECT, chip_w: i32, chip_h: i32, bounds: RECT) -> RECT {
    let gap = constants::OVERLAY_READOUT_GAP;
    let threshold = constants::OVERLAY_READOUT_EDGE_THRESHOLD;

    let mut left = sel.right + gap;
    let mut top = sel.bottom + gap;

    if left + chip_w + threshold > bounds.right || top + chip_h + threshold > bounds.bottom {
        left = sel.right - gap - chip_w;
        top = sel.bottom - gap - chip_h;
    }

    RECT {
        left,
        top,
        right: left + chip_w,
        bottom: top + chip_h,
    }
}

/// REG-06: distinguishes a click from a drag by total movement against the
/// caller-supplied system drag thresholds (`SM_CXDRAG`/`SM_CYDRAG` --
/// never hardcoded here, per UI-SPEC).
pub fn is_click(start: POINT, end: POINT, cx_drag: i32, cy_drag: i32) -> bool {
    (end.x - start.x).abs() <= cx_drag && (end.y - start.y).abs() <= cy_drag
}

/// Returns the union of the old and new selection + readout-chip rects,
/// inflated by `pad` (handle radius + border), for `InvalidateRect`
/// (RESEARCH Pattern 6 -- never full-screen invalidate per mouse move).
pub fn dirty_union(old_sel: RECT, old_chip: RECT, new_sel: RECT, new_chip: RECT, pad: i32) -> RECT {
    let left = [old_sel.left, old_chip.left, new_sel.left, new_chip.left]
        .into_iter()
        .min()
        .unwrap()
        - pad;
    let top = [old_sel.top, old_chip.top, new_sel.top, new_chip.top]
        .into_iter()
        .min()
        .unwrap()
        - pad;
    let right = [old_sel.right, old_chip.right, new_sel.right, new_chip.right]
        .into_iter()
        .max()
        .unwrap()
        + pad;
    let bottom = [
        old_sel.bottom,
        old_chip.bottom,
        new_sel.bottom,
        new_chip.bottom,
    ]
    .into_iter()
    .max()
    .unwrap()
        + pad;

    RECT {
        left,
        top,
        right,
        bottom,
    }
}

/// Formats the live dimension readout (D-29 / UI-SPEC copywriting
/// contract): `{w} × {h}` using U+00D7, single space each side, no "px"
/// suffix.
pub fn readout_text(sel: RECT) -> String {
    let w = sel.right - sel.left;
    let h = sel.bottom - sel.top;
    format!("{w} \u{00D7} {h}")
}

// ---------------------------------------------------------------------
// Window lifecycle (REG-01/REG-02, Plan 02 Task 1)
// ---------------------------------------------------------------------

/// Live state for the currently-open overlay window. GDI handles are stored
/// as `isize` (toast.rs convention) so the struct can live behind a
/// `Mutex` without a `Send`/`Sync` fight with raw Win32 handle types.
struct OverlayData {
    /// The untouched, saved-pixel source of truth (REG-05): confirm crops
    /// this, never the DIBs and never a re-capture.
    frozen: RawBitmap,
    /// Client rect, origin (0, 0) -- width/height of the monitor.
    bounds: RECT,
    /// The monitor's screen-space left/top, kept for reference only.
    mon_origin: POINT,
    sel: Option<RECT>,
    state: OverlayState,
    closing: bool,
    awaiting_release: bool,
    bright_dc: isize,
    bright_bmp: isize,
    dim_dc: isize,
    dim_bmp: isize,
    back_dc: isize,
    back_bmp: isize,
}

/// Process-global handle to the currently-open overlay window, if any.
/// Cleared by `wnd_proc` on `WM_DESTROY`.
static OVERLAY_HWND: Mutex<Option<isize>> = Mutex::new(None);

/// Live overlay state, guarded so only the pump thread ever touches it
/// (mirrors `toast.rs`'s `TOAST_HWND`/`TOAST_TEXT` convention).
static OVERLAY_DATA: Mutex<Option<OverlayData>> = Mutex::new(None);

/// Guards one-time registration of the overlay window class.
static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

/// Milliseconds elapsed in the most recent `open()` call, from entry to
/// `ShowWindow` returning -- read by `--overlay-selftest` (Task 3).
static LAST_OPEN_MS: Mutex<Option<f64>> = Mutex::new(None);

/// `true` while an overlay window exists (REG-01..06 modality gate for
/// `app::dispatch`, wired in Plan 03/04).
pub fn is_active() -> bool {
    OVERLAY_HWND.lock().unwrap().is_some()
}

/// Runs `f` with mutable access to the live `OverlayData`, or returns
/// `None` if no overlay is open. The single seam Plans 03/04 use instead of
/// locking `OVERLAY_DATA` directly.
fn with_state<R>(f: impl FnOnce(&mut OverlayData) -> R) -> Option<R> {
    let mut guard = OVERLAY_DATA.lock().unwrap();
    guard.as_mut().map(f)
}

/// Reads back the most recent `open()` timing, in milliseconds. Used by
/// `--overlay-selftest` (Task 3).
pub fn last_open_ms() -> Option<f64> {
    *LAST_OPEN_MS.lock().unwrap()
}

/// Reads back the frozen bitmap's dimensions for the currently-open
/// overlay, if any. Used by `--overlay-selftest` (Task 3) so a human can
/// confirm the overlay covered the whole monitor.
pub fn frozen_dims() -> Option<(i32, i32)> {
    with_state(|d| (d.frozen.width, d.frozen.height))
}

/// Dev-only: closes the currently-open overlay immediately, mirroring the
/// same single cancel funnel every real cancel/confirm path uses. No-op if
/// no overlay is open. Used by `--overlay-selftest` (Task 3).
pub fn close_active() {
    // The MutexGuard from `.lock()` must be dropped before `cancel()` runs:
    // `DestroyWindow` delivers `WM_DESTROY` synchronously, which re-locks
    // `OVERLAY_HWND` to clear it -- holding the guard across that call
    // (e.g. via `if let Some(raw) = *OVERLAY_HWND.lock().unwrap() { .. }`,
    // whose temporary lives for the whole `if let` block) deadlocks.
    let hwnd_raw = *OVERLAY_HWND.lock().unwrap();
    if let Some(raw) = hwnd_raw {
        let hwnd = HWND(raw as *mut c_void);
        cancel(hwnd);
    }
}

/// Dev-only: pumps messages until the currently-open overlay has been
/// destroyed. Used by `--overlay-selftest` (Task 3) so the process can exit
/// once teardown is complete.
pub fn run_until_closed() {
    // `close_active()` calls `DestroyWindow`, which delivers `WM_DESTROY`
    // synchronously (same thread) before returning -- by the time this runs
    // the window may already be gone, so check first or `GetMessageW` would
    // block forever waiting for a message that will never arrive.
    let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    unsafe {
        while is_active()
            && windows::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, None, 0, 0)
                .as_bool()
        {
            let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
            if !is_active() {
                break;
            }
        }
    }
}

fn register_class_once() {
    CLASS_REGISTERED.get_or_init(|| unsafe {
        let hinstance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let class_name = constants::to_wide(constants::OVERLAY_WINDOW_CLASS);
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            panic!("failed to register overlay window class");
        }
    });
}

/// Builds a top-down 32bpp `BI_RGB` DIB section selected into a fresh
/// compatible DC -- the exact header `capture.rs`'s `capture_via` uses, so
/// client coords == bitmap coords == frozen-buffer offsets.
///
/// Returns `(dc, bitmap, bits_ptr)`; `bits_ptr` points at `width * height *
/// 4` writable bytes for the duration of the DC/bitmap's lifetime.
unsafe fn create_dib_dc(width: i32, height: i32) -> WinResult<(isize, isize, *mut c_void)> {
    let screen_dc = windows::Win32::Graphics::Gdi::GetDC(None);
    let mem_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
    unsafe { windows::Win32::Graphics::Gdi::ReleaseDC(None, screen_dc) };

    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width;
    bmi.bmiHeader.biHeight = -height; // negative => top-down DIB
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB.0;

    let mut bits_ptr: *mut c_void = std::ptr::null_mut();
    let dib = match unsafe {
        CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut bits_ptr, None, 0)
    } {
        Ok(d) => d,
        Err(e) => {
            let _ = unsafe { DeleteDC(mem_dc) };
            return Err(e);
        }
    };
    unsafe { SelectObject(mem_dc, dib.into()) };

    Ok((mem_dc.0 as isize, dib.0 as isize, bits_ptr))
}

/// Opens the overlay: freezes the cursor's monitor, composites the bright
/// and dimmed DIBs plus a back buffer, and creates a focused borderless
/// topmost popup exactly covering that monitor (REG-01, RESEARCH Pitfall
/// 3/6). No-op if the overlay is already open.
pub fn open() {
    if is_active() {
        return;
    }

    let t0 = Instant::now();

    // Step 1-2: freeze strictly before any window exists (Pitfall 3) -- the
    // overlay's own chrome can never end up in the saved pixels.
    let mon = match capture::monitor_under_cursor() {
        Ok(m) => m,
        Err(e) => {
            crate::toast::show(&format!("Capture failed — {e}"));
            return;
        }
    };
    let frozen = match capture::grab(mon) {
        Ok(b) => b,
        Err(e) => {
            crate::toast::show(&format!("Capture failed — {e}"));
            return;
        }
    };

    let width = mon.right - mon.left;
    let height = mon.bottom - mon.top;

    register_class_once();

    // Step 5: bright DIB -- a straight copy of the frozen bytes.
    let (bright_dc, bright_bmp, bright_bits) = match unsafe { create_dib_dc(width, height) } {
        Ok(v) => v,
        Err(e) => {
            crate::toast::show(&format!("Capture failed — {e}"));
            return;
        }
    };
    let byte_len = (width as usize) * (height as usize) * 4;
    unsafe {
        std::ptr::copy_nonoverlapping(frozen.bgra.as_ptr(), bright_bits as *mut u8, byte_len);
    }

    // Step 6: dim DIB -- RESEARCH Pitfall 6 fallback: memcpy the frozen
    // bytes directly into the dim DIB's bits (like the bright DIB above)
    // instead of an extra full-frame `BitBlt` from bright -> dim; this
    // trims one 4K-sized GDI blit off the <50 ms critical path. Then darken
    // with a single `AlphaBlend` of a 1x1 black source stretched over the
    // whole surface.
    let (dim_dc, dim_bmp, dim_bits) = match unsafe { create_dib_dc(width, height) } {
        Ok(v) => v,
        Err(e) => {
            free_dc_bmp(bright_dc, bright_bmp);
            crate::toast::show(&format!("Capture failed — {e}"));
            return;
        }
    };
    unsafe {
        std::ptr::copy_nonoverlapping(frozen.bgra.as_ptr(), dim_bits as *mut u8, byte_len);
    }
    let dim_hdc = windows::Win32::Graphics::Gdi::HDC(dim_dc as *mut c_void);
    let (black_dc, black_bmp, black_bits) = match unsafe { create_dib_dc(1, 1) } {
        Ok(v) => v,
        Err(e) => {
            free_dc_bmp(bright_dc, bright_bmp);
            free_dc_bmp(dim_dc, dim_bmp);
            crate::toast::show(&format!("Capture failed — {e}"));
            return;
        }
    };
    unsafe {
        std::ptr::write_bytes(black_bits as *mut u8, 0, 4);
    }
    let black_hdc = windows::Win32::Graphics::Gdi::HDC(black_dc as *mut c_void);
    let bf = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: constants::OVERLAY_DIM_ALPHA,
        AlphaFormat: 0,
    };
    unsafe {
        let _ = AlphaBlend(dim_hdc, 0, 0, width, height, black_hdc, 0, 0, 1, 1, bf);
    }
    free_dc_bmp(black_dc, black_bmp);

    // Step 7: monitor-sized back buffer used by Task 2's paint.
    let (back_dc, back_bmp, _back_bits) = match unsafe { create_dib_dc(width, height) } {
        Ok(v) => v,
        Err(e) => {
            free_dc_bmp(bright_dc, bright_bmp);
            free_dc_bmp(dim_dc, dim_bmp);
            crate::toast::show(&format!("Capture failed — {e}"));
            return;
        }
    };

    // Step 8: opaque, topmost, focused popup -- NOT layered, NOT
    // no-activate (RESEARCH Anti-Patterns / Pitfall 4). mon.left/top may be
    // negative on a monitor left of primary; pass through unchanged.
    let hwnd = unsafe {
        let hinstance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let class_name = constants::to_wide(constants::OVERLAY_WINDOW_CLASS);
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_POPUP,
            mon.left,
            mon.top,
            width,
            height,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    };
    let hwnd = match hwnd {
        Ok(h) => h,
        Err(e) => {
            free_dc_bmp(bright_dc, bright_bmp);
            free_dc_bmp(dim_dc, dim_bmp);
            free_dc_bmp(back_dc, back_bmp);
            crate::toast::show(&format!("Capture failed — {e}"));
            return;
        }
    };

    // Step 9: store handles + state before showing, so wnd_proc's first
    // WM_PAINT (delivered by ShowWindow) has state to render.
    let bounds = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    *OVERLAY_DATA.lock().unwrap() = Some(OverlayData {
        frozen,
        bounds,
        mon_origin: POINT {
            x: mon.left,
            y: mon.top,
        },
        sel: None,
        state: OverlayState::Idle,
        closing: false,
        awaiting_release: true,
        bright_dc,
        bright_bmp,
        dim_dc,
        dim_bmp,
        back_dc,
        back_bmp,
    });
    *OVERLAY_HWND.lock().unwrap() = Some(hwnd.0 as isize);

    // Step 10: activate with the plain "show" flag (not the toast's
    // background-only show flag) -- the overlay is the one window that
    // must take keyboard focus.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if GetForegroundWindow() != hwnd {
            // Not fatal: the WM_ACTIVATE(WA_INACTIVE) guard (Plan 03, D-33)
            // is the safety net that cancels a never-activated overlay.
        }
    }

    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    *LAST_OPEN_MS.lock().unwrap() = Some(elapsed_ms);
    #[cfg(debug_assertions)]
    {
        let msg = format!("overlay::open took {elapsed_ms:.2} ms\0");
        let wide: Vec<u16> = msg.encode_utf16().collect();
        unsafe { OutputDebugStringW(PCWSTR(wide.as_ptr())) };
    }
}

fn free_dc_bmp(dc: isize, bmp: isize) {
    unsafe {
        let hdc = windows::Win32::Graphics::Gdi::HDC(dc as *mut c_void);
        let hbmp = windows::Win32::Graphics::Gdi::HBITMAP(bmp as *mut c_void);
        let _ = DeleteObject(hbmp.into());
        let _ = DeleteDC(hdc);
    }
}

/// The single cancel funnel every path in Plans 03/04 calls: sets `closing`
/// then destroys the window. Idempotent-safe against `WM_ACTIVATE`
/// re-entrancy during teardown (Pitfall 4) -- a second call while already
/// closing is a no-op.
fn cancel(hwnd: HWND) {
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
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe { paint(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // Single cleanup point: free every GDI object, drop `frozen`,
            // clear the statics.
            let data = OVERLAY_DATA.lock().unwrap().take();
            if let Some(d) = data {
                free_dc_bmp(d.back_dc, d.back_bmp);
                free_dc_bmp(d.dim_dc, d.dim_bmp);
                free_dc_bmp(d.bright_dc, d.bright_bmp);
            }
            *OVERLAY_HWND.lock().unwrap() = None;
            LRESULT(0)
        }
        // wparam/lparam are opaque -- never dereferenced (T-01-04
        // convention). Everything else falls through unhandled.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// ---------------------------------------------------------------------
// Painting (REG-02/REG-03/REG-04, Plan 02 Task 2)
// ---------------------------------------------------------------------

/// Renders `OverlayData` into the back buffer restricted to `ps.rcPaint`,
/// then blits the back buffer to the window in one call. Plan 03 only
/// mutates state and calls `invalidate_change` -- this is the sole place
/// pixels are produced.
unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };

    let rendered = with_state(|d| {
        let back_hdc = windows::Win32::Graphics::Gdi::HDC(d.back_dc as *mut c_void);
        let dim_hdc = windows::Win32::Graphics::Gdi::HDC(d.dim_dc as *mut c_void);
        let bright_hdc = windows::Win32::Graphics::Gdi::HDC(d.bright_dc as *mut c_void);
        let r = ps.rcPaint;
        let w = r.right - r.left;
        let h = r.bottom - r.top;

        // 1. Dim background for the dirty region.
        unsafe {
            let _ = BitBlt(back_hdc, r.left, r.top, w, h, Some(dim_hdc), r.left, r.top, SRCCOPY);
        }

        if let Some(sel) = d.sel {
            // 2. Full-brightness selection interior, intersected with the
            // dirty region.
            let ix = intersect_rect(sel, r);
            if ix.right > ix.left && ix.bottom > ix.top {
                unsafe {
                    let _ = BitBlt(
                        back_hdc,
                        ix.left,
                        ix.top,
                        ix.right - ix.left,
                        ix.bottom - ix.top,
                        Some(bright_hdc),
                        ix.left,
                        ix.top,
                        SRCCOPY,
                    );
                }
            }

            // 3. Border stroked on the selection boundary.
            unsafe {
                let pen = CreatePen(PS_SOLID, constants::OVERLAY_BORDER_WIDTH, COLORREF(constants::OVERLAY_ACCENT));
                let old_pen = SelectObject(back_hdc, pen.into());
                let old_brush = SelectObject(back_hdc, GetStockObject(NULL_BRUSH));
                let _ = Rectangle(back_hdc, sel.left, sel.top, sel.right, sel.bottom);
                SelectObject(back_hdc, old_pen);
                SelectObject(back_hdc, old_brush);
                let _ = DeleteObject(pen.into());
            }

            // 4. Filled handle squares.
            unsafe {
                let brush = CreateSolidBrush(COLORREF(constants::OVERLAY_ACCENT));
                for (_, hr) in handle_rects(sel, constants::OVERLAY_HANDLE_SIZE) {
                    FillRect(back_hdc, &hr, brush);
                }
                let _ = DeleteObject(brush.into());
            }

            // 5. Readout chip.
            let text = readout_text(sel);
            let mut wide_text = constants::to_wide(&text);
            wide_text.pop();
            let font_name = constants::to_wide("Segoe UI");
            unsafe {
                let font = CreateFontW(
                    constants::OVERLAY_FONT_HEIGHT,
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
                );
                let old_font = SelectObject(back_hdc, font.into());

                let mut measure_rect = RECT::default();
                DrawTextW(back_hdc, &mut wide_text, &mut measure_rect, DT_CALCRECT | DT_SINGLELINE);
                let chip_w = (measure_rect.right - measure_rect.left) + 2 * constants::OVERLAY_READOUT_PAD_X;
                let chip_h = (measure_rect.bottom - measure_rect.top) + 2 * constants::OVERLAY_READOUT_PAD_Y;
                let chip = readout_rect(sel, chip_w, chip_h, d.bounds);

                let chip_brush = CreateSolidBrush(COLORREF(constants::OVERLAY_CHIP_FILL));
                let old_brush = SelectObject(back_hdc, chip_brush.into());
                let old_pen2 = SelectObject(back_hdc, GetStockObject(NULL_BRUSH));
                let _ = RoundRect(
                    back_hdc,
                    chip.left,
                    chip.top,
                    chip.right,
                    chip.bottom,
                    constants::OVERLAY_CHIP_RADIUS * 2,
                    constants::OVERLAY_CHIP_RADIUS * 2,
                );
                SelectObject(back_hdc, old_brush);
                SelectObject(back_hdc, old_pen2);
                let _ = DeleteObject(chip_brush.into());

                SetBkMode(back_hdc, TRANSPARENT);
                SetTextColor(back_hdc, COLORREF(constants::OVERLAY_ACCENT));
                let mut text_rect = chip;
                DrawTextW(back_hdc, &mut wide_text, &mut text_rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

                SelectObject(back_hdc, old_font);
                let _ = DeleteObject(font.into());
            }
        }

        // Single BitBlt from the back buffer to the window for the dirty
        // region.
        unsafe {
            let _ = BitBlt(hdc, r.left, r.top, w, h, Some(back_hdc), r.left, r.top, SRCCOPY);
        }
    });
    let _ = rendered;

    let _ = unsafe { EndPaint(hwnd, &ps) };
}

fn intersect_rect(a: RECT, b: RECT) -> RECT {
    RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    }
}

/// Computes the dirty union of `old_sel`/`new_sel` (plus their readout
/// chips) and invalidates exactly that region -- never a full-screen
/// invalidate on a geometry change (RESEARCH Pattern 6 perf trap). `false`
/// for erase: the back buffer covers the region and `WM_ERASEBKGND` is
/// suppressed.
fn invalidate_change(hwnd: HWND, old_sel: Option<RECT>, new_sel: Option<RECT>) {
    let bounds = match with_state(|d| d.bounds) {
        Some(b) => b,
        None => return,
    };
    let pad = constants::OVERLAY_HANDLE_SIZE / 2 + constants::OVERLAY_BORDER_WIDTH + 1;
    let chip_of = |sel: Option<RECT>| -> RECT {
        match sel {
            Some(s) => {
                let text = readout_text(s);
                // Conservative fixed-size estimate avoids a GDI text
                // measurement here; paint() recomputes the exact chip.
                let approx_w = (text.chars().count() as i32) * constants::OVERLAY_FONT_HEIGHT
                    + 2 * constants::OVERLAY_READOUT_PAD_X;
                let approx_h = constants::OVERLAY_FONT_HEIGHT + 2 * constants::OVERLAY_READOUT_PAD_Y;
                readout_rect(s, approx_w, approx_h, bounds)
            }
            None => RECT::default(),
        }
    };
    let old_sel_r = old_sel.unwrap_or_default();
    let new_sel_r = new_sel.unwrap_or_default();
    let old_chip = chip_of(old_sel);
    let new_chip = chip_of(new_sel);
    let dirty = dirty_union(old_sel_r, old_chip, new_sel_r, new_chip, pad);
    unsafe {
        let _ = InvalidateRect(Some(hwnd), Some(&dirty), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    fn pt(x: i32, y: i32) -> POINT {
        POINT { x, y }
    }

    #[test]
    fn normalize_rect_handles_reverse_drag() {
        let r = normalize_rect(pt(100, 100), pt(40, 30));
        assert_eq!(r, rect(40, 30, 100, 100));
    }

    #[test]
    fn normalize_rect_handles_forward_drag() {
        let r = normalize_rect(pt(10, 20), pt(50, 60));
        assert_eq!(r, rect(10, 20, 50, 60));
    }

    #[test]
    fn handle_rects_returns_eight_centered_squares() {
        let sel = rect(100, 100, 200, 200);
        let handles = handle_rects(sel, 8);
        assert_eq!(handles.len(), 8);
        for (_, r) in handles {
            assert_eq!(r.right - r.left, 8);
            assert_eq!(r.bottom - r.top, 8);
        }
        // SE handle centered on the selection's bottom-right corner.
        let (_, se) = handles.iter().find(|(h, _)| *h == Handle::SE).unwrap();
        assert_eq!(*se, rect(196, 196, 204, 204));
    }

    #[test]
    fn hit_test_prefers_handle_over_interior() {
        let sel = rect(100, 100, 200, 200);
        // Point inside the inflated SE handle hit box, but also inside the
        // selection interior -- handle must win.
        let hit = hit_test(sel, pt(198, 198), constants::OVERLAY_HANDLE_HIT_SIZE);
        assert_eq!(hit, Hit::Handle(Handle::SE));
    }

    #[test]
    fn hit_test_returns_inside_away_from_handles() {
        let sel = rect(100, 100, 200, 200);
        let hit = hit_test(sel, pt(150, 150), constants::OVERLAY_HANDLE_HIT_SIZE);
        assert_eq!(hit, Hit::Inside);
    }

    #[test]
    fn hit_test_returns_outside() {
        let sel = rect(100, 100, 200, 200);
        let hit = hit_test(sel, pt(0, 0), constants::OVERLAY_HANDLE_HIT_SIZE);
        assert_eq!(hit, Hit::Outside);
    }

    #[test]
    fn clamp_rect_preserves_size_when_it_fits() {
        let sel = rect(10, 10, 50, 40);
        let bounds = rect(0, 0, 1000, 1000);
        let out = clamp_rect(sel, bounds);
        assert_eq!(out, sel);
    }

    #[test]
    fn clamp_rect_pushes_fully_inside_bounds() {
        let sel = rect(-10, -10, 30, 30);
        let bounds = rect(0, 0, 100, 100);
        let out = clamp_rect(sel, bounds);
        assert_eq!(out.left, 0);
        assert_eq!(out.top, 0);
        assert_eq!(out.right - out.left, 40);
        assert_eq!(out.bottom - out.top, 40);
    }

    #[test]
    fn clamp_rect_never_smaller_than_1x1() {
        let sel = rect(0, 0, 10, 10);
        let bounds = rect(0, 0, 0, 0);
        let out = clamp_rect(sel, bounds);
        assert_eq!(out.right - out.left, 1);
        assert_eq!(out.bottom - out.top, 1);
    }

    #[test]
    fn nudge_rect_moves_and_stops_at_bounds() {
        let sel = rect(0, 0, 10, 10);
        let bounds = rect(0, 0, 100, 100);
        let moved = nudge_rect(sel, 5, 5, bounds);
        assert_eq!(moved, rect(5, 5, 15, 15));

        // Walking far past the right/bottom edge stops flush with bounds.
        let stopped = nudge_rect(sel, 1000, 1000, bounds);
        assert_eq!(stopped, rect(90, 90, 100, 100));
    }

    #[test]
    fn grow_rect_moves_only_bottom_right_edge() {
        let sel = rect(10, 10, 20, 20);
        let bounds = rect(0, 0, 100, 100);
        let grown = grow_rect(sel, 5, 3, bounds);
        assert_eq!(grown, rect(10, 10, 25, 23));
    }

    #[test]
    fn grow_rect_floors_at_1x1() {
        let sel = rect(10, 10, 20, 20);
        let bounds = rect(0, 0, 100, 100);
        let shrunk = grow_rect(sel, -1000, -1000, bounds);
        assert_eq!(shrunk.right - shrunk.left, 1);
        assert_eq!(shrunk.bottom - shrunk.top, 1);
    }

    #[test]
    fn resize_rect_corner_handle_renormalizes_past_anchor() {
        // SE handle anchored at the NW corner (100, 100); dragging past
        // the anchor (to the upper-left of it) must still yield a valid,
        // non-negative rect.
        let original = rect(100, 100, 200, 200);
        let anchor = pt(100, 100);
        let floating = pt(50, 60);
        let out = resize_rect(anchor, floating, Handle::SE, original);
        assert_eq!(out, rect(50, 60, 100, 100));
    }

    #[test]
    fn resize_rect_edge_handle_only_moves_perpendicular_axis() {
        let original = rect(100, 100, 200, 200);
        // E handle: anchor is the left edge; only the right edge (x) moves.
        let anchor = pt(100, 100);
        let floating = pt(250, 9999);
        let out = resize_rect(anchor, floating, Handle::E, original);
        assert_eq!(out, rect(100, 100, 250, 200));
    }

    #[test]
    fn readout_rect_places_outside_bottom_right_by_default() {
        let sel = rect(100, 100, 200, 200);
        let bounds = rect(0, 0, 1000, 1000);
        let chip = readout_rect(sel, 60, 24, bounds);
        assert_eq!(chip.left, 200 + constants::OVERLAY_READOUT_GAP);
        assert_eq!(chip.top, 200 + constants::OVERLAY_READOUT_GAP);
    }

    #[test]
    fn readout_rect_flips_inside_near_edge() {
        // Selection's bottom-right corner is close to the bounds edge, so
        // the default outside placement would clip -- must flip inside.
        let sel = rect(900, 900, 990, 990);
        let bounds = rect(0, 0, 1000, 1000);
        let chip = readout_rect(sel, 60, 24, bounds);
        assert!(chip.right <= sel.right);
        assert!(chip.bottom <= sel.bottom);
    }

    #[test]
    fn is_click_within_threshold() {
        assert!(is_click(pt(100, 100), pt(102, 101), 4, 4));
        assert!(!is_click(pt(100, 100), pt(110, 100), 4, 4));
    }

    #[test]
    fn dirty_union_contains_old_and_new_rects() {
        let old_sel = rect(0, 0, 10, 10);
        let old_chip = rect(12, 12, 30, 20);
        let new_sel = rect(5, 5, 20, 20);
        let new_chip = rect(22, 22, 40, 30);
        let union = dirty_union(old_sel, old_chip, new_sel, new_chip, 2);
        assert_eq!(union, rect(-2, -2, 42, 32));
    }

    #[test]
    fn readout_text_formats_multiplication_sign() {
        let sel = rect(0, 0, 1280, 720);
        let text = readout_text(sel);
        assert_eq!(text, "1280 \u{00D7} 720");
        assert!(text.contains('\u{00D7}'));
        assert!(!text.contains('x'));
    }

    #[test]
    fn overlay_state_variants_are_distinguishable() {
        assert_ne!(OverlayState::Idle, OverlayState::Selected);
        let a = OverlayState::DraggingNew { anchor: pt(0, 0) };
        let b = OverlayState::DraggingNew { anchor: pt(0, 0) };
        assert_eq!(a, b);
        let moving = OverlayState::Moving {
            grab_offset: pt(1, 1),
        };
        let resizing = OverlayState::Resizing {
            handle: Handle::NW,
            anchor: pt(0, 0),
        };
        assert_ne!(moving, resizing);
    }
}
