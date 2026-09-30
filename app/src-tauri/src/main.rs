//! Attack Shark X11 desktop app: the settings window, the battery in the tray, low-battery alerts,
//! and starting at login.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod icon;
mod startup;
mod theme;
mod tray;
mod worker;

use attack_shark_x11::settings;
use serde_json::json;
use tauri::window::Color;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const MAIN_WINDOW: &str = "main";
/// Passed when the system starts the app at login: stay in the tray.
pub const BACKGROUND_FLAG: &str = "--background";

pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn create_window(app: &AppHandle, visible: bool) -> tauri::Result<()> {
    // The page gets the option lists, version and platform up front, before its first render.
    let boot = json!({
        "meta": settings::meta(),
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "startup_when": startup::WHEN,
    });
    let background = if theme::light_apps() { Color(238, 238, 238, 255) } else { Color(9, 9, 9, 255) };
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::App("index.html".into()))
        .title(tray::TITLE)
        .inner_size(1200.0, 820.0)
        .min_inner_size(980.0, 680.0)
        .center()
        .visible(visible)
        .background_color(background)
        .initialization_script(format!("window.BOOT = {boot};"))
        .build()?;
    Ok(())
}

fn main() {
    let background = std::env::args().any(|arg| arg == BACKGROUND_FLAG);
    let app = tauri::Builder::default()
        // Must come first: a second launch just brings up the running app's window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_window(app)))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::get_live,
            commands::update_dpi,
            commands::update_polling_rate,
            commands::update_lighting,
            commands::update_buttons,
            commands::update_profile,
            commands::set_startup,
        ])
        .setup(move |app| {
            #[cfg(windows)]
            let _ = tray::register_notifications(app.handle());
            app.manage(worker::Worker::start(app.handle().clone()));
            tray::create(app.handle())?;
            create_window(app.handle(), !background)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the app in the tray; Quit in the tray menu exits.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to start the app");
    app.run(|_app, event| match event {
        RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        // A click on the Dock icon brings the window back.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_window(_app),
        _ => {}
    });
}
