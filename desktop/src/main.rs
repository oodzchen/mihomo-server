//! Desktop window and system tray for this user's local mihomo-server.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod commands;
mod controller;
mod i18n;
mod local;
mod model;
mod notify;
mod tray;
mod window;

use controller::Controller;
use std::sync::Arc;

/// Release builds take the version from the tag.
pub const VERSION: &str = match option_env!("MIHOMO_DESKTOP_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

const USAGE: &str = "Usage: mihomo-server-desktop [--hidden]

Opens the local mihomo-server management page and a tray menu for proxy
mode, TUN, nodes and the service itself. Installs mihomo-server when it is
missing.

  --hidden    Start in the tray without opening a window
  --version   Print the version
  --help      Show this help

Environment (development): MIHOMO_SERVER_API and MIHOMO_SERVER_TOKEN_FILE
select a service outside systemd; MIHOMO_SERVER_INSTALLER replaces the
published installer with a path or URL.";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments
        .iter()
        .any(|argument| argument == "--version" || argument == "-V")
    {
        println!("mihomo-server-desktop {VERSION}");
        return;
    }
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("{USAGE}");
        return;
    }
    let hidden = arguments.iter().any(|argument| argument == "--hidden");
    let controller = Arc::new(Controller::new(i18n::Language::system()));
    let initial = controller.model();
    tauri::Builder::default()
        // Must be registered first: a second launch only focuses this one.
        .plugin(tauri_plugin_single_instance::init(|app, _arguments, _cwd| {
            window::open_preferred(app);
        }))
        .manage(controller)
        .manage(tray::TrayState::default())
        .manage(window::WindowState::default())
        .invoke_handler(tauri::generate_handler![
            commands::local_state,
            commands::install_service,
            commands::start_service,
            commands::restart_service,
            commands::open_dashboard
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            tray::create(&handle, initial)?;
            tauri::async_runtime::spawn(controller::run(handle, !hidden));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start the desktop client")
        .run(|_app, event| {
            // Closing the last window keeps the tray running; Quit exits.
            if let tauri::RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}
