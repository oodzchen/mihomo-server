//! Commands of the bundled status page (see `capabilities/setup.json`), and
//! the only two the management page may call (see `window::grant_dashboard`).
use crate::autostart;
use crate::controller::{self, Controller};
use crate::i18n::Language;
use crate::local;
use crate::model::{Service, Task};
use crate::window;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Manager as _, State};

#[derive(Serialize)]
pub struct LocalState {
    state: &'static str,
    /// The service's own version.
    version: Option<String>,
    task: Task,
    /// The install or lifecycle task that failed; details are in `log`.
    failed: Option<Task>,
    log: Vec<String>,
    language: &'static str,
    /// The temporary proxy installation downloads through.
    proxy: Option<String>,
}

#[tauri::command]
pub fn local_state(controller: State<'_, Arc<Controller>>) -> LocalState {
    let (state, version) = match controller.snapshot().service {
        Service::Detecting => ("detecting", None),
        Service::NotInstalled => ("not_installed", None),
        Service::Inactive => ("inactive", None),
        Service::Unreachable(_) => ("unreachable", None),
        Service::Running(live) => ("running", live.service_version),
    };
    LocalState {
        state,
        version,
        task: controller.task(),
        failed: controller.failed(),
        log: controller.log.lines(),
        language: controller.language().code(),
        proxy: controller.proxy(),
    }
}

#[tauri::command]
pub async fn install_service(app: AppHandle) -> Result<(), String> {
    start(&app, Task::Installing)
}

#[tauri::command]
pub async fn start_service(app: AppHandle) -> Result<(), String> {
    start(&app, Task::Starting)
}

#[tauri::command]
pub async fn restart_service(app: AppHandle) -> Result<(), String> {
    start(&app, Task::Restarting)
}

#[tauri::command]
pub async fn open_dashboard(app: AppHandle) {
    window::open_dashboard(&app);
}

/// The global interface language, shared with the management page.
#[tauri::command]
pub async fn set_interface_language(app: AppHandle, language: String) -> Result<(), String> {
    let language = Language::from_code(&language).ok_or("unknown language")?;
    controller::choose_language(&app, language)
        .await
        .map_err(|error| format!("{error:#}"))
}

/// Refused as `invalid_proxy`; returns the normalized address (empty: none).
#[tauri::command]
pub fn set_install_proxy(controller: State<'_, Arc<Controller>>, proxy: String) -> Result<Option<String>, String> {
    let proxy = local::parse_proxy(&proxy).map_err(|_| "invalid_proxy")?;
    controller.set_proxy(proxy.clone());
    Ok(proxy)
}

/// Milliseconds the installer's download took through PROXY (empty: none);
/// refused as `invalid_proxy`, else with why it failed.
#[tauri::command]
pub async fn test_install_proxy(proxy: String) -> Result<u64, String> {
    let proxy = local::parse_proxy(&proxy).map_err(|_| "invalid_proxy")?;
    let elapsed = local::test_proxy(proxy.as_deref())
        .await
        .map_err(|error| format!("{error:#}"))?;
    Ok(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

/// Whether this client starts (into the tray) at login.
#[tauri::command]
pub fn client_autostart() -> bool {
    autostart::enabled()
}

/// Returns the state read back after the change.
#[tauri::command]
pub fn set_client_autostart(enabled: bool) -> Result<bool, String> {
    autostart::set(enabled).map_err(|error| format!("{error:#}"))?;
    Ok(autostart::enabled())
}

/// A refusal is also written to the output panel, where the page shows errors.
fn start(app: &AppHandle, task: Task) -> Result<(), String> {
    if controller::start_task(app, task, None) {
        return Ok(());
    }
    let message = "another operation is still running";
    app.state::<Arc<Controller>>().log.push(format!("error: {message}"));
    Err(message.into())
}
