//! System tray icon and context menu (TRAY-01, TRAY-02, TRAY-03).
//!
//! Created on the pump thread, strictly after `app::create_main_window()`
//! (RESEARCH.md Pitfall 2 -- event handlers must not `PostMessage` to
//! `main_hwnd` before it exists). Event handlers do nothing but forward a
//! small integer index to the hub window; `app.rs` owns what each index
//! means (T-01-12: wparam never carries a pointer).

use std::sync::OnceLock;

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::app;
use crate::constants;

/// Resource ordinal of the embedded app icon (`resources/app.rc`:
/// `IDI_APPICON 1`) -- loaded from the executable's own resources so the
/// shipped exe carries a single icon copy.
const IDI_APPICON: u16 = 1;

/// wparam values posted with `WM_APP_MENU`. `app.rs`'s `wnd_proc` matches
/// against these instead of re-deriving the menu order.
pub const MENU_INDEX_SETTINGS: usize = 0;
pub const MENU_INDEX_OPEN_FOLDER: usize = 1;
pub const MENU_INDEX_ABOUT: usize = 2;
pub const MENU_INDEX_EXIT: usize = 3;

/// wparam value posted with `WM_APP_TRAY` for a left-click release.
pub const TRAY_INDEX_LEFT_CLICK: usize = 0;

/// The four menu items' ids, indexed identically to the `MENU_INDEX_*`
/// constants above, populated once by `init()`. The `MenuEvent` handler
/// looks an incoming `MenuId` up in this array instead of comparing
/// strings, per RESEARCH.md guidance.
static MENU_IDS: OnceLock<[MenuId; 4]> = OnceLock::new();

/// Creates the tray icon and its four-item context menu (Settings, Open
/// capture folder, About, a separator, then Exit) and wires the crate's
/// event callbacks to forward interactions to the hub window.
///
/// The returned `TrayIcon` must be kept alive for the process lifetime --
/// dropping it removes the icon.
pub fn init() -> Result<TrayIcon, Box<dyn std::error::Error>> {
    let settings_item = MenuItem::new("Settings", true, None);
    let open_folder_item = MenuItem::new("Open capture folder", true, None);
    let about_item = MenuItem::new("About", true, None);
    let exit_item = MenuItem::new("Exit", true, None);

    MENU_IDS
        .set([
            settings_item.id().clone(),
            open_folder_item.id().clone(),
            about_item.id().clone(),
            exit_item.id().clone(),
        ])
        .expect("tray::init() called more than once");

    let menu = Menu::new();
    menu.append(&settings_item)?;
    menu.append(&open_folder_item)?;
    menu.append(&about_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&exit_item)?;

    let icon = Icon::from_resource(IDI_APPICON, None)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip(constants::APP_NAME)
        // Left click forwards to the hub window as TRAY_INDEX_LEFT_CLICK
        // (TRAY-03) instead of the default behavior of opening the menu.
        .with_menu_on_left_click(false)
        .build()?;

    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        let ids = MENU_IDS.get().expect("tray::init() not yet called");
        if let Some(index) = ids.iter().position(|id| id == event.id()) {
            unsafe {
                let _ = PostMessageW(
                    Some(app::main_hwnd()),
                    constants::WM_APP_MENU,
                    WPARAM(index),
                    LPARAM(0),
                );
            }
        }
    }));

    TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            unsafe {
                let _ = PostMessageW(
                    Some(app::main_hwnd()),
                    constants::WM_APP_TRAY,
                    WPARAM(TRAY_INDEX_LEFT_CLICK),
                    LPARAM(0),
                );
            }
        }
    }));

    Ok(tray)
}
