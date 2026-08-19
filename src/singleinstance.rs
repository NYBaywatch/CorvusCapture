//! Single-instance enforcement (TRAY-04, D-01).
//!
//! A named mutex is acquired before any window, tray icon, or hotkey is
//! created. If acquisition fails because an instance already holds it, the
//! existing instance is pinged via a registered window message (so it can
//! toast) and this process exits — nothing is opened or focused (D-01).

use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};

use crate::constants;

/// Cached id from `RegisterWindowMessageW` so sender and receiver resolve
/// the same value.
static ALREADY_RUNNING_MSG: OnceLock<u32> = OnceLock::new();

/// Owns the single-instance mutex handle for the process lifetime. Must be
/// bound to a named variable in `main` — a dropped-early guard would let a
/// second copy start.
pub struct InstanceGuard {
    handle: HANDLE,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// The registered message id used to notify an already-running instance.
/// A registered message (vs. a raw `WM_APP + n`) is negotiated system-wide
/// and cannot collide with another app's private message space.
pub fn already_running_message() -> u32 {
    *ALREADY_RUNNING_MSG.get_or_init(|| {
        let name = constants::to_wide(constants::ALREADY_RUNNING_MSG_NAME);
        unsafe { RegisterWindowMessageW(PCWSTR(name.as_ptr())) }
    })
}

/// Attempts to acquire the single-instance mutex.
///
/// Returns `Some(guard)` if this is the first instance. Returns `None` if
/// another instance already holds the mutex — in that case the existing
/// instance has already been notified and the caller should exit quietly.
pub fn acquire() -> Option<InstanceGuard> {
    let name = constants::to_wide(constants::SINGLE_INSTANCE_MUTEX);
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }.ok()?;

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        notify_existing_instance();
        return None;
    }

    Some(InstanceGuard { handle })
}

/// Pings the existing instance's hub window so it can show the "already
/// running" toast. Does NOT call `SetForegroundWindow` or `ShowWindow` —
/// D-01 requires nothing be focused or shown by this path. If the existing
/// instance's window cannot be found yet (still starting up), exits quietly.
fn notify_existing_instance() {
    let class_name = constants::to_wide(constants::MAIN_WINDOW_CLASS);
    let hwnd = unsafe { FindWindowW(PCWSTR(class_name.as_ptr()), PCWSTR::null()) };
    let Ok(hwnd) = hwnd else {
        return;
    };
    if hwnd == HWND(std::ptr::null_mut()) {
        return;
    }
    unsafe {
        let _ = PostMessageW(Some(hwnd), already_running_message(), WPARAM(0), LPARAM(0));
    }
}
