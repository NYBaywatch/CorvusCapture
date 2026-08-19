//! About dialog (D-06): a standard Win32 `MessageBox` naming the app,
//! version, a one-line description, and the GitHub URL as plain text --
//! deliberately not a custom dialog or a clickable link.

use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

use crate::app;
use crate::constants;

/// Shows the About `MessageBox`, owned by the hidden hub window.
pub fn show() {
    let body = format!(
        "{}\nVersion {}\nHotkey screenshot utility for Windows 11\n{}",
        constants::APP_NAME,
        constants::APP_VERSION,
        constants::GITHUB_URL,
    );
    let title = constants::to_wide(constants::APP_NAME);
    let text = constants::to_wide(&body);
    unsafe {
        MessageBoxW(
            Some(app::main_hwnd()),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}
