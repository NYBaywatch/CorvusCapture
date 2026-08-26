//! CF_DIB clipboard write (SAVE-07 / D-16). Clipboard failure is non-fatal
//! -- the file save always happens regardless; callers of `copy_dib` should
//! log/ignore an error, never block the save path on it.

use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_DIB;

use crate::app;
use crate::capture::RawBitmap;

/// Places `bitmap` on the clipboard as a CF_DIB bitmap (SAVE-07). Builds a
/// 32bpp `BITMAPINFOHEADER` -- reusing the capture buffer's native BGRA
/// format with no second conversion -- immediately followed by the pixel
/// bytes in a single `GlobalAlloc(GMEM_MOVEABLE, ...)` block, per RESEARCH.md
/// Pattern 5.
///
/// Ownership rule: on a SUCCESSFUL `SetClipboardData`, the clipboard takes
/// ownership of the global memory handle -- this function must NOT
/// `GlobalFree` it on that path. On any failure before or during
/// `SetClipboardData`, the handle is freed and the clipboard closed here.
/// This is deliberately asymmetric from the always-delete GDI convention
/// used elsewhere (e.g. `toast::paint`'s font/brush cleanup).
pub fn copy_dib(bitmap: &RawBitmap) -> windows::core::Result<()> {
    let header_len = size_of::<BITMAPINFOHEADER>();
    let pixel_len = bitmap.bgra.len();
    let total_len = header_len + pixel_len;

    let hglobal: HGLOBAL = unsafe { GlobalAlloc(GMEM_MOVEABLE, total_len)? };

    let ptr = unsafe { GlobalLock(hglobal) };
    if ptr.is_null() {
        let _ = unsafe { GlobalFree(Some(hglobal)) };
        return Err(windows::core::Error::from_thread());
    }

    let mut header = BITMAPINFOHEADER {
        biSize: header_len as u32,
        biWidth: bitmap.width,
        // Negative => top-down, matching the capture buffer's own layout
        // (RawBitmap's doc comment) so no row-flip is needed here.
        biHeight: -bitmap.height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: pixel_len as u32,
        ..Default::default()
    };
    let header_ptr = &mut header as *mut BITMAPINFOHEADER as *const u8;

    unsafe {
        std::ptr::copy_nonoverlapping(header_ptr, ptr as *mut u8, header_len);
        std::ptr::copy_nonoverlapping(
            bitmap.bgra.as_ptr(),
            (ptr as *mut u8).add(header_len),
            pixel_len,
        );
        // GlobalUnlock reports an error once the object's lock count drops
        // to zero even on a normal successful unlock (Win32 convention:
        // FALSE + NO_ERROR) -- this is not a real failure and is
        // intentionally ignored, matching the pervasive `let _ =`
        // non-fatal-cleanup convention used throughout this codebase.
        let _ = GlobalUnlock(hglobal);
    }

    if let Err(e) = unsafe { OpenClipboard(Some(app::main_hwnd())) } {
        let _ = unsafe { GlobalFree(Some(hglobal)) };
        return Err(e);
    }

    if let Err(e) = unsafe { EmptyClipboard() } {
        let _ = unsafe { CloseClipboard() };
        let _ = unsafe { GlobalFree(Some(hglobal)) };
        return Err(e);
    }

    let set_result =
        unsafe { SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(hglobal.0 as *mut c_void))) };

    match set_result {
        Ok(_) => {
            // Clipboard now owns `hglobal` -- do not free it.
            let _ = unsafe { CloseClipboard() };
            Ok(())
        }
        Err(e) => {
            let _ = unsafe { GlobalFree(Some(hglobal)) };
            let _ = unsafe { CloseClipboard() };
            Err(e)
        }
    }
}
