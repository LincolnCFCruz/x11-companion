//! The tray icon: battery level at a glance, the settings window a click away, low-battery alerts.

use std::sync::Mutex;
use std::time::Duration;

use attack_shark_x11::battery::{LowBatteryAlerts, STALE_AFTER, summary};
use attack_shark_x11::protocol::BatteryState;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_notification::NotificationExt;

use crate::worker::LiveView;
use crate::{icon, theme};

const TRAY: &str = "main";
pub const TITLE: &str = "Attack Shark X11";

struct TrayState {
    startup: CheckMenuItem<Wry>,
    /// The tooltip as a menu line, shown on Linux, whose trays have no tooltips.
    status: MenuItem<Wry>,
    alerts: Mutex<LowBatteryAlerts>,
    /// The (level, light taskbar) the icon was last drawn for.
    shown: Mutex<Option<(Option<u8>, bool)>>,
}

fn image(level: Option<u8>, light_taskbar: bool) -> Image<'static> {
    Image::new_owned(icon::render(level, light_taskbar), icon::SIZE, icon::SIZE)
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let startup_on = crate::startup::is_enabled().unwrap_or(false);
    let label = format!("Start {}", crate::startup::WHEN);
    let startup = CheckMenuItem::with_id(app, "startup", label, true, startup_on, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", TITLE, false, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "open", "Open Attack Shark X11", true, None::<&str>)?,
            &startup,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
        ],
    )?;
    if cfg!(target_os = "linux") {
        menu.prepend_items(&[&status, &PredefinedMenuItem::separator(app)?])?;
    }
    TrayIconBuilder::with_id(TRAY)
        .icon(image(None, theme::light_taskbar()))
        // macOS tints a template icon to suit the menu bar; only its alpha counts.
        .icon_as_template(true)
        .tooltip(TITLE)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => crate::show_window(app),
            "startup" => toggle_startup(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                crate::show_window(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(TrayState { startup, status, alerts: Mutex::default(), shown: Mutex::new(None) });
    Ok(())
}

fn toggle_startup(app: &AppHandle) {
    if let Err(error) = crate::startup::set_enabled(!crate::startup::is_enabled().unwrap_or(false)) {
        notify(app, &format!("Couldn't change the startup setting: {error}"));
    }
    sync_startup(app);
}

/// Match the menu's checkbox to the real startup setting.
pub fn sync_startup(app: &AppHandle) {
    if let Some(state) = app.try_state::<TrayState>() {
        let _ = state.startup.set_checked(crate::startup::is_enabled().unwrap_or(false));
    }
}

/// Refresh the icon, tooltip and checkbox from a live snapshot, and raise low-battery alerts.
pub fn update(app: &AppHandle, live: &LiveView) {
    let (Some(state), Some(tray)) = (app.try_state::<TrayState>(), app.tray_by_id(TRAY)) else {
        return;
    };
    let fresh = live.battery.as_ref().filter(|battery| live.connected && battery.age <= STALE_AFTER.as_secs_f64());
    let shown = (fresh.map(|battery| battery.level), theme::light_taskbar());
    {
        let mut last = state.shown.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *last != Some(shown) {
            // A new icon on its own would drop the template flag on macOS.
            let _ = tray.set_icon_with_as_template(Some(image(shown.0, shown.1)), true);
            *last = Some(shown);
        }
    }
    let reading = live.battery.as_ref().map(|battery| (battery.reading, Duration::from_secs_f64(battery.age)));
    let status = summary(TITLE, live.connected, reading);
    let _ = state.status.set_text(&status);
    let _ = tray.set_tooltip(Some(status));
    let _ = state.startup.set_checked(live.startup.unwrap_or(false));

    if let Some(battery) = fresh {
        let discharging = battery.reading.state() == Some(BatteryState::Discharging);
        let alert =
            state.alerts.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).update(battery.level, discharging);
        if let Some(message) = alert {
            notify(app, &message);
        }
    }
}

fn notify(app: &AppHandle, body: &str) {
    let _ = app.notification().builder().title(TITLE).body(body).show();
}

/// Windows only shows notifications from apps it knows. An installer introduces the app with its
/// Start menu shortcut; the standalone exe registers its notification ID itself.
#[cfg(windows)]
pub fn register_notifications(app: &AppHandle) -> std::io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let icon = app.path().app_local_data_dir().map_err(std::io::Error::other)?.join("notification.png");
    std::fs::create_dir_all(icon.parent().expect("the icon has a folder"))?;
    std::fs::write(&icon, include_bytes!("../icons/128x128.png"))?;
    let key = format!(r"Software\Classes\AppUserModelId\{}", app.config().identifier);
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(key)?;
    key.set_value("DisplayName", &TITLE)?;
    key.set_value("IconUri", &icon.as_os_str())
}
