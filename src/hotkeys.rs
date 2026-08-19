//! Global hotkey registration: F9, Ctrl+F9, Shift+F9, Alt+F9 (TRAY-*, ERR-03).
//!
//! Each hotkey is registered individually via one `register()` call per
//! entry -- never the batch API, which short-circuits on the first
//! failure -- so a single conflict never prevents the rest from
//! registering (RESEARCH.md Pattern 3). The
//! returned `GlobalHotKeyManager` must be kept alive by the caller for the
//! process lifetime; dropping it unregisters every hotkey.

use std::sync::OnceLock;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::app::AppAction;
use crate::constants;
use crate::toast;

/// Maps a registered hotkey's runtime id to its display label and the
/// `AppAction` it will trigger (the Phase 2/3/4 seam).
struct HotKeyBinding {
    id: u32,
    label: &'static str,
    action_kind: ActionKind,
}

/// A `Copy`, non-`AppAction`-holding tag so the id map can live in a
/// `OnceLock` without requiring `AppAction` to implement `Clone`/`Copy`.
#[derive(Clone, Copy)]
pub enum ActionKind {
    CaptureFullscreen,
    CaptureActiveWindow,
    CaptureRegion,
    OpenSettings,
}

impl ActionKind {
    pub fn to_action(self) -> AppAction {
        match self {
            ActionKind::CaptureFullscreen => AppAction::CaptureFullscreen,
            ActionKind::CaptureActiveWindow => AppAction::CaptureActiveWindow,
            ActionKind::CaptureRegion => AppAction::CaptureRegion,
            ActionKind::OpenSettings => AppAction::OpenSettings,
        }
    }
}

/// Process-global id -> (label, action) map, populated once by `init`.
static HOTKEY_MAP: OnceLock<Vec<(u32, &'static str, ActionKind)>> = OnceLock::new();

/// Looks up a fired hotkey's runtime id. Returns `None` for unknown ids
/// (T-01-17: wparam is validated before dispatch).
pub fn lookup(id: u32) -> Option<(&'static str, ActionKind)> {
    HOTKEY_MAP
        .get()
        .and_then(|map| map.iter().find(|(hid, _, _)| *hid == id))
        .map(|(_, label, action)| (*label, *action))
}

/// Registers the four hotkeys individually and wires the crate's event
/// handler to post `WM_APP_HOTKEY` back to the hub window. Must be called
/// on the pump thread, strictly after `app::create_main_window()`
/// (RESEARCH.md Pitfall 2).
pub fn init() -> Result<GlobalHotKeyManager, Box<dyn std::error::Error>> {
    let manager = GlobalHotKeyManager::new()?;

    let definitions: [(HotKey, &'static str, ActionKind); 4] = [
        (
            HotKey::new(None, Code::F9),
            "F9",
            ActionKind::CaptureFullscreen,
        ),
        (
            HotKey::new(Some(Modifiers::CONTROL), Code::F9),
            "Ctrl+F9",
            ActionKind::CaptureActiveWindow,
        ),
        (
            HotKey::new(Some(Modifiers::SHIFT), Code::F9),
            "Shift+F9",
            ActionKind::CaptureRegion,
        ),
        (
            HotKey::new(Some(Modifiers::ALT), Code::F9),
            "Alt+F9",
            ActionKind::OpenSettings,
        ),
    ];

    let mut bindings: Vec<(u32, &'static str, ActionKind)> = Vec::with_capacity(4);
    let mut failed_labels: Vec<&'static str> = Vec::new();

    for (hotkey, label, action) in definitions {
        match manager.register(hotkey) {
            Ok(()) => bindings.push((hotkey.id(), label, action)),
            // `AlreadyRegistered` (another app owns this combination) is
            // named separately for clarity, but is handled identically to
            // every other registration error for the user-facing message
            // -- ERR-03 only requires that the app names the failure and
            // keeps running with the rest.
            Err(global_hotkey::Error::AlreadyRegistered(_)) => failed_labels.push(label),
            Err(_other) => failed_labels.push(label),
        }
    }

    HOTKEY_MAP
        .set(bindings)
        .map_err(|_| "hotkeys::init() called more than once")?;

    if !failed_labels.is_empty() {
        let text = if failed_labels.len() == 1 {
            format!("Hotkey unavailable: {}", failed_labels[0])
        } else {
            format!("Hotkeys unavailable: {}", failed_labels.join(", "))
        };
        toast::show(&text);
    }

    GlobalHotKeyEvent::set_event_handler(Some(|event: GlobalHotKeyEvent| {
        // Only the key-pressed edge fires a capture/action -- ignore the
        // release event so a single tap fires exactly once.
        if event.state() != HotKeyState::Pressed {
            return;
        }
        unsafe {
            let _ = PostMessageW(
                Some(crate::app::main_hwnd()),
                constants::WM_APP_HOTKEY,
                WPARAM(event.id() as usize),
                LPARAM(0),
            );
        }
    }));

    Ok(manager)
}
