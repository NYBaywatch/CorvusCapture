//! Pure pixel-acquisition layer (CAP-01/CAP-02/CAP-04/CAP-05). No windowing,
//! no config, no I/O -- every function here operates on a caller-supplied
//! rect or the foreground window and is thread-affine to the UI/pump thread
//! (the screen DC, `GetCursorPos`, and `GetForegroundWindow` all require the
//! calling thread's desktop session).
//!
//! `grab` is the single pixel-acquisition path reused by F9, Ctrl+F9, and
//! (Phase 3) the region overlay -- callers resolve a rect, this module turns
//! it into a top-down 32bpp BGRA buffer and nothing else.

use std::ffi::c_void;
use std::mem::size_of;

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
    GetDC, GetMonitorInfoW, MonitorFromPoint, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowRect, PW_RENDERFULLCONTENT,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Captured pixels: top-down 32bpp BGRA, 4 bytes per pixel, `width * height *
/// 4` bytes total -- exactly the layout `CreateDIBSection` produces below.
/// Consumers (encoder in plan 02-03, clipboard) convert as needed; this
/// module never does a color-order conversion itself.
pub struct RawBitmap {
    pub width: i32,
    pub height: i32,
    pub bgra: Vec<u8>,
}

/// D-20: distinguishes "no usable foreground window" from a real capture
/// target, so callers never have to inspect a possibly-null `HWND`
/// themselves. `offscreen` drives D-21's BitBlt-vs-PrintWindow choice in
/// `grab_window`.
pub enum ActiveWindowTarget {
    None,
    Window {
        rect: RECT,
        hwnd: HWND,
        offscreen: bool,
    },
}

/// Shared DIB-section acquisition + pixel-fill + readback used by both
/// `grab` (BitBlt) and `grab_window`'s off-screen `PrintWindow` branch, so
/// the create/select/restore/delete lifecycle (`toast::paint`'s convention)
/// is written exactly once rather than duplicated between the two capture
/// paths.
unsafe fn capture_via<F>(
    screen_dc: HDC,
    width: i32,
    height: i32,
    fill: F,
) -> windows::core::Result<RawBitmap>
where
    F: FnOnce(HDC) -> windows::core::Result<()>,
{
    let mem_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };

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
    let old = unsafe { SelectObject(mem_dc, dib.into()) };

    let fill_result = fill(mem_dc);

    let bitmap = fill_result.map(|_| {
        let byte_len = (width * height * 4) as usize;
        RawBitmap {
            width,
            height,
            bgra: unsafe { std::slice::from_raw_parts(bits_ptr as *const u8, byte_len).to_vec() },
        }
    });

    unsafe {
        SelectObject(mem_dc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem_dc);
    }

    bitmap
}

/// Captures `rect` (physical-pixel screen coordinates) into a top-down BGRA
/// buffer via a plain `SRCCOPY` BitBlt -- no compositing flag is added, which
/// is what excludes the hardware cursor from the output (CAP-04).
pub fn grab(rect: RECT) -> windows::core::Result<RawBitmap> {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return Err(windows::core::Error::from_thread());
    }

    unsafe {
        let screen_dc = GetDC(None);
        let result = capture_via(screen_dc, width, height, |mem_dc| {
            BitBlt(
                mem_dc,
                0,
                0,
                width,
                height,
                Some(screen_dc),
                rect.left,
                rect.top,
                SRCCOPY,
            )
        });
        ReleaseDC(None, screen_dc);
        result
    }
}

/// Resolves the monitor under the current cursor position to its full
/// physical-pixel rect (CAP-01). Does all math in physical pixels via
/// `rcMonitor` -- never the primary-monitor-only screen-size metrics -- and
/// never assumes a `(0, 0)` origin, since a monitor left of or above the
/// primary produces legitimately negative `rcMonitor.left`/`.top` (CAP-02).
pub fn monitor_under_cursor() -> windows::core::Result<RECT> {
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt)?;
        let hmon: HMONITOR = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);

        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(hmon, &mut info).as_bool() {
            return Err(windows::core::Error::from_thread());
        }
        Ok(info.rcMonitor)
    }
}

/// Half-open rectangle intersection test (touching edges do not count as
/// overlapping). Extracted as a private, display-free helper so it is
/// unit-testable without a real monitor configuration.
fn intersects(a: RECT, b: RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

unsafe extern "system" fn monitor_enum_proc(
    _hmonitor: HMONITOR,
    _hdc: HDC,
    rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let monitors = unsafe { &mut *(lparam.0 as *mut Vec<RECT>) };
    monitors.push(unsafe { *rect });
    BOOL(1)
}

/// D-22: counts how many monitors `rect` overlaps, for the multi-monitor
/// span advisory on the save toast (window/region spanning >1 monitor).
pub fn monitor_span_count(rect: RECT) -> u32 {
    let mut monitors: Vec<RECT> = Vec::new();
    unsafe {
        let lparam = LPARAM(&mut monitors as *mut Vec<RECT> as isize);
        let _ = EnumDisplayMonitors(None, None, Some(monitor_enum_proc), lparam);
    }
    monitors.iter().filter(|m| intersects(**m, rect)).count() as u32
}

/// Resolves the Ctrl+F9 capture target: the foreground window's DWM
/// extended-frame-bounds rect (CAP-05), or `None` if there is no usable
/// foreground window (D-20 -- desktop/taskbar focused, null HWND).
///
/// Deliberately uses `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`
/// rather than the plain window-rect API, which includes the invisible
/// drop-shadow margin Windows adds around top-level windows on 10/11.
/// Windows 11 rounded corners will include a few background pixels at the
/// corners; that is accepted behavior (matches Win+PrintScreen) -- no
/// per-window masking.
pub fn active_window_target() -> windows::core::Result<ActiveWindowTarget> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return Ok(ActiveWindowTarget::None);
        }

        let mut rect = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )?;

        // D-21: these four virtual-screen metrics (never the
        // primary-monitor-only screen-size metrics) give the full
        // multi-monitor bounding rect, used only to detect whether the
        // window pokes off the edge of every display.
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vscreen = RECT {
            left: vx,
            top: vy,
            right: vx + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: vy + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        };
        let offscreen = !(rect.left >= vscreen.left
            && rect.top >= vscreen.top
            && rect.right <= vscreen.right
            && rect.bottom <= vscreen.bottom);

        Ok(ActiveWindowTarget::Window {
            rect,
            hwnd,
            offscreen,
        })
    }
}

/// Captures an active-window target (D-21 hybrid): a fully on-screen window
/// goes through the reliable `grab` BitBlt path unchanged; a partially
/// off-screen window is rendered via `PrintWindow(.., PW_RENDERFULLCONTENT)`
/// into the same shared DIB section instead, so the off-screen portion is
/// still captured. The rare black-render risk `PrintWindow` carries on some
/// DirectComposition-heavy apps is accepted per D-21 and confined to this
/// off-screen branch only.
pub fn grab_window(target: &ActiveWindowTarget) -> windows::core::Result<RawBitmap> {
    let (rect, hwnd, offscreen) = match target {
        ActiveWindowTarget::Window {
            rect,
            hwnd,
            offscreen,
        } => (*rect, *hwnd, *offscreen),
        ActiveWindowTarget::None => return Err(windows::core::Error::from_thread()),
    };

    if !offscreen {
        return grab(rect);
    }

    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return Err(windows::core::Error::from_thread());
    }

    // PrintWindow renders the window at its full `GetWindowRect` geometry
    // (which includes the invisible drop-shadow/resize margin), NOT at the
    // smaller DWM extended-frame-bounds `rect`. Size the DIB from the
    // window rect, render, then crop to the frame-bounds sub-rect so this
    // branch yields the same framing as the on-screen BitBlt branch.
    let mut win_rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut win_rect)? };
    let win_width = win_rect.right - win_rect.left;
    let win_height = win_rect.bottom - win_rect.top;
    if win_width <= 0 || win_height <= 0 {
        return Err(windows::core::Error::from_thread());
    }

    let full = unsafe {
        let screen_dc = GetDC(None);
        let result = capture_via(screen_dc, win_width, win_height, |mem_dc| {
            if PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool() {
                Ok(())
            } else {
                Err(windows::core::Error::from_thread())
            }
        });
        ReleaseDC(None, screen_dc);
        result
    }?;

    Ok(crop_bitmap(
        &full,
        rect.left - win_rect.left,
        rect.top - win_rect.top,
        width,
        height,
    ))
}

/// Copies the `width` x `height` sub-rect at `(x, y)` out of `src` into a
/// new top-down BGRA buffer. The requested rect is clamped to `src`'s
/// bounds, so a degenerate/misreported window rect can shrink the output
/// but never read out of bounds.
fn crop_bitmap(src: &RawBitmap, x: i32, y: i32, width: i32, height: i32) -> RawBitmap {
    let x = x.clamp(0, src.width);
    let y = y.clamp(0, src.height);
    let width = width.clamp(0, src.width - x);
    let height = height.clamp(0, src.height - y);

    let src_stride = src.width as usize * 4;
    let row_bytes = width as usize * 4;
    let mut bgra = Vec::with_capacity(row_bytes * height as usize);
    for row in y..y + height {
        let start = row as usize * src_stride + x as usize * 4;
        bgra.extend_from_slice(&src.bgra[start..start + row_bytes]);
    }

    RawBitmap {
        width,
        height,
        bgra,
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

    #[test]
    fn disjoint_rects_do_not_intersect() {
        assert!(!intersects(rect(0, 0, 10, 10), rect(20, 20, 30, 30)));
    }

    #[test]
    fn touching_edge_is_not_intersecting() {
        assert!(!intersects(rect(0, 0, 10, 10), rect(10, 0, 20, 10)));
    }

    #[test]
    fn overlapping_rects_intersect() {
        assert!(intersects(rect(0, 0, 10, 10), rect(5, 5, 15, 15)));
    }

    #[test]
    fn crop_bitmap_extracts_subrect() {
        // 3x3 bitmap whose pixels are numbered 0..9 in every channel.
        let bgra: Vec<u8> = (0u8..9).flat_map(|n| [n; 4]).collect();
        let src = RawBitmap {
            width: 3,
            height: 3,
            bgra,
        };
        // 2x2 crop at (1, 1) -> pixels 4, 5, 7, 8.
        let out = crop_bitmap(&src, 1, 1, 2, 2);
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 2);
        let expected: Vec<u8> = [4u8, 5, 7, 8].iter().flat_map(|n| [*n; 4]).collect();
        assert_eq!(out.bgra, expected);
    }

    #[test]
    fn crop_bitmap_clamps_out_of_bounds_request() {
        let bgra: Vec<u8> = (0u8..9).flat_map(|n| [n; 4]).collect();
        let src = RawBitmap {
            width: 3,
            height: 3,
            bgra,
        };
        // Requesting past the right/bottom edge shrinks, never panics.
        let out = crop_bitmap(&src, 2, 2, 5, 5);
        assert_eq!(out.width, 1);
        assert_eq!(out.height, 1);
        assert_eq!(out.bgra, vec![8u8; 4]);
        // Negative offsets are clamped to 0.
        let out = crop_bitmap(&src, -1, -1, 2, 2);
        assert_eq!((out.width, out.height), (2, 2));
    }

    #[test]
    fn negative_coordinate_rects_intersect_correctly() {
        // A monitor left of primary has legitimately negative rcMonitor
        // coordinates (CAP-02, PITFALLS Pitfall 1) -- span detection must
        // handle this the same as positive coordinates.
        assert!(intersects(rect(-100, -100, -50, -50), rect(-60, -60, -10, -10)));
        assert!(!intersects(rect(-100, -100, -50, -50), rect(-40, -40, -10, -10)));
    }
}
