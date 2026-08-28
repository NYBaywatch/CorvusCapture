//! Start with Windows — HKCU Run value (SET-05, D-50/D-51/D-52).
//!
//! Pure helpers (`desired_command`, `commands_match`) are unit-tested
//! without touching the registry. The thin Win32 wrappers below them
//! (`read_command`/`write_command`/`delete_command`) follow the same
//! open/check-`WIN32_ERROR`/close-handle shape as `singleinstance.rs`.
//! HKCU only — never HKLM, never elevation.

use std::ffi::c_void;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RRF_RT_REG_SZ,
};

use crate::constants;

const RUN_SUBKEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");

/// Not yet consumed outside `is_registered`/`set_enabled`, reachable from
/// the Settings checkbox in plan 04-04.
#[allow(dead_code)]
const STARTUP_APPROVED_SUBKEY: PCWSTR =
    w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run");

// Pure helpers

/// The quoted command this app would write to the Run value: the current
/// exe path wrapped in double quotes, with no `\\?\` prefix (Pitfall 7,
/// Assumption A7). `std::env::current_exe()` already returns the plain
/// `GetModuleFileNameW` path on Windows — never `canonicalize()` here, that
/// would introduce the `\\?\` extended-length prefix and break the shell's
/// ability to launch it at login.
pub fn desired_command() -> std::io::Result<String> {
    let path = std::env::current_exe()?;
    let path_str = path.to_string_lossy();
    let stripped = path_str.strip_prefix(r"\\?\").unwrap_or(&path_str);
    Ok(format!("\"{stripped}\""))
}

/// True when `registry` and `desired` refer to the same command, ignoring
/// surrounding whitespace, surrounding double quotes, and letter case
/// (D-51 — Windows paths are case-insensitive).
pub fn commands_match(registry: &str, desired: &str) -> bool {
    fn norm(s: &str) -> String {
        s.trim().trim_matches('"').to_lowercase()
    }
    norm(registry) == norm(desired)
}

// Registry wrappers

/// Reads the current Run value for this app. `None` if absent or not a
/// `REG_SZ`.
pub fn read_command() -> Option<String> {
    unsafe {
        let mut len: u32 = 0;
        let err = RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_SUBKEY,
            PCWSTR(constants::to_wide(constants::RUN_VALUE_NAME).as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut len),
        );
        if err != ERROR_SUCCESS || len == 0 {
            return None;
        }
        let mut buf = vec![0u16; (len as usize / 2).max(1)];
        let value_name = constants::to_wide(constants::RUN_VALUE_NAME);
        let err = RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_SUBKEY,
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut len),
        );
        if err != ERROR_SUCCESS {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    }
}

/// Writes `cmd` as the Run value for this app. Minimal SAM rights:
/// `KEY_SET_VALUE | KEY_QUERY_VALUE`. Always closes the key.
pub fn write_command(cmd: &str) -> Result<(), windows::core::Error> {
    unsafe {
        let mut hkey = HKEY::default();
        let err = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_SUBKEY,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            None,
            &mut hkey,
            None,
        );
        if err != ERROR_SUCCESS {
            return Err(err.into());
        }
        let wide = constants::to_wide(cmd); // includes trailing NUL
        let bytes: &[u8] = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
        let value_name = constants::to_wide(constants::RUN_VALUE_NAME);
        let err = RegSetValueExW(
            hkey,
            PCWSTR(value_name.as_ptr()),
            None,
            REG_SZ,
            Some(bytes),
        );
        let _ = RegCloseKey(hkey);
        if err != ERROR_SUCCESS {
            return Err(err.into());
        }
        Ok(())
    }
}

/// Deletes the Run value for this app. Succeeds silently when the value or
/// key was already absent. Minimal SAM rights: `KEY_SET_VALUE`.
///
/// Not yet consumed: reachable from the Settings checkbox in plan 04-04.
#[allow(dead_code)]
pub fn delete_command() -> Result<(), windows::core::Error> {
    unsafe {
        let mut hkey = HKEY::default();
        let err = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_SUBKEY, None, KEY_SET_VALUE, &mut hkey);
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if err != ERROR_SUCCESS {
            return Err(err.into());
        }
        let value_name = constants::to_wide(constants::RUN_VALUE_NAME);
        let err = RegDeleteValueW(hkey, PCWSTR(value_name.as_ptr()));
        let _ = RegCloseKey(hkey);
        if err == ERROR_SUCCESS || err == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(err.into())
        }
    }
}

/// Reads the Task Manager "Disabled" flag Windows stores separately from
/// the Run value itself [MEDIUM: community-documented, not official —
/// tenforums/nutanix/renenyffenegger sources; Pitfall 8, Assumption A4].
/// Task Manager does not delete the Run value when a user disables an
/// entry — it writes this `REG_BINARY` value whose first byte is `0x02`
/// (enabled) or `0x03` (disabled). A read failure (key/value absent) is
/// treated as "not disabled" — the common case where Task Manager was
/// never used to touch this entry.
///
/// Not yet consumed: reachable from `is_registered`, wired to the Settings
/// checkbox in plan 04-04.
#[allow(dead_code)]
fn is_task_manager_disabled() -> bool {
    unsafe {
        let mut len: u32 = 0;
        let value_name = constants::to_wide(constants::RUN_VALUE_NAME);
        let err = RegGetValueW(
            HKEY_CURRENT_USER,
            STARTUP_APPROVED_SUBKEY,
            PCWSTR(value_name.as_ptr()),
            windows::Win32::System::Registry::RRF_RT_REG_BINARY,
            None,
            None,
            Some(&mut len),
        );
        if err != ERROR_SUCCESS || len == 0 {
            return false;
        }
        let mut buf = vec![0u8; len as usize];
        let value_name = constants::to_wide(constants::RUN_VALUE_NAME);
        let err = RegGetValueW(
            HKEY_CURRENT_USER,
            STARTUP_APPROVED_SUBKEY,
            PCWSTR(value_name.as_ptr()),
            windows::Win32::System::Registry::RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut len),
        );
        if err != ERROR_SUCCESS || buf.is_empty() {
            return false;
        }
        buf[0] == 0x03
    }
}

/// True only when the Run value matches this exe AND the entry has not
/// been disabled via Task Manager's Startup tab (D-51 — the state reported
/// to the UI is derived from the registry, not from the config flag).
///
/// Not yet consumed: wired to the Settings checkbox state in plan 04-04.
#[allow(dead_code)]
pub fn is_registered() -> bool {
    match (read_command(), desired_command()) {
        (Some(r), Ok(d)) => commands_match(&r, &d) && !is_task_manager_disabled(),
        _ => false,
    }
}

/// Enables or disables Start with Windows. On enable, writes the desired
/// command and clears a prior Task-Manager disable (so an earlier
/// Task-Manager "Disable" does not silently defeat this toggle); a failure
/// deleting the `StartupApproved` value is ignored. On disable, deletes
/// only the Run value — `StartupApproved` is left untouched.
///
/// Not yet consumed: wired to the Settings checkbox toggle in plan 04-04.
#[allow(dead_code)]
pub fn set_enabled(enabled: bool) -> Result<(), windows::core::Error> {
    if enabled {
        let cmd = desired_command().map_err(|e| {
            windows::core::Error::new(windows::core::HRESULT(0), e.to_string())
        })?;
        write_command(&cmd)?;
        let _ = delete_startup_approved();
        Ok(())
    } else {
        delete_command()
    }
}

/// Deletes the `StartupApproved\Run` value for this app, if present.
/// Errors (including "already absent") are the caller's to ignore per
/// `set_enabled`'s contract.
///
/// Not yet consumed: called from `set_enabled`, wired in plan 04-04.
#[allow(dead_code)]
fn delete_startup_approved() -> Result<(), windows::core::Error> {
    unsafe {
        let mut hkey = HKEY::default();
        let err = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            STARTUP_APPROVED_SUBKEY,
            None,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if err != ERROR_SUCCESS {
            return Err(err.into());
        }
        let value_name = constants::to_wide(constants::RUN_VALUE_NAME);
        let err = RegDeleteValueW(hkey, PCWSTR(value_name.as_ptr()));
        let _ = RegCloseKey(hkey);
        if err == ERROR_SUCCESS || err == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(err.into())
        }
    }
}

/// Called from `main.rs` after `config::load()` when `cfg.start_with_windows`
/// is true (D-50). Rewrites the Run value when it is missing or stale
/// (e.g. the exe was moved); a no-op when it already matches. Errors are
/// ignored silently — this runs before any UI exists to report them.
/// Deliberately does NOT consult `StartupApproved` — respecting a user's
/// Task Manager choice is the safer default.
pub fn self_heal() {
    if let Ok(desired) = desired_command() {
        let needs_write = match read_command() {
            Some(current) => !commands_match(&current, &desired),
            None => true,
        };
        if needs_write {
            let _ = write_command(&desired);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desired_command_is_quoted() {
        let cmd = desired_command().expect("current_exe should resolve in test binary");
        assert!(cmd.starts_with('"'), "expected leading quote: {cmd}");
        assert!(cmd.ends_with('"'), "expected trailing quote: {cmd}");
    }

    #[test]
    fn desired_command_has_no_extended_length_prefix() {
        let cmd = desired_command().expect("current_exe should resolve in test binary");
        assert!(!cmd.contains(r"\\?\"), "unexpected \\\\?\\ prefix: {cmd}");
    }

    #[test]
    fn commands_match_ignores_surrounding_quotes() {
        assert!(commands_match(
            "\"C:\\Apps\\CorvusCapture.exe\"",
            "C:\\Apps\\CorvusCapture.exe"
        ));
    }

    #[test]
    fn commands_match_ignores_whitespace() {
        assert!(commands_match(
            "  \"C:\\Apps\\CorvusCapture.exe\"  ",
            "\"C:\\Apps\\CorvusCapture.exe\""
        ));
    }

    #[test]
    fn commands_match_ignores_case() {
        assert!(commands_match(
            "\"C:\\APPS\\CorvusCapture.EXE\"",
            "\"c:\\apps\\corvuscapture.exe\""
        ));
    }

    #[test]
    fn commands_match_rejects_different_path() {
        assert!(!commands_match(
            "\"C:\\Apps\\Other.exe\"",
            "\"C:\\Apps\\CorvusCapture.exe\""
        ));
    }

    /// Real-registry round trip. Ignored by default so a normal `cargo
    /// test` never touches the real autostart entry — only run explicitly
    /// with `cargo test startup:: -- --ignored`.
    #[test]
    #[ignore]
    fn registry_round_trip_write_read_delete() {
        let cmd = "\"C:\\test\\startup_roundtrip_test.exe\"";
        write_command(cmd).expect("write should succeed");
        let read = read_command().expect("value should be present after write");
        assert!(commands_match(&read, cmd));
        delete_command().expect("delete should succeed");
        assert!(read_command().is_none());
        // Deleting again should still succeed (already absent).
        delete_command().expect("delete of absent value should succeed");
    }
}
