//! Commands of the bundled status page (see `capabilities/setup.json`).
use crate::controller::{self, Controller};
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

/// A refusal is also written to the output panel, where the page shows errors.
fn start(app: &AppHandle, task: Task) -> Result<(), String> {
    if controller::start_task(app, task, None) {
        return Ok(());
    }
    let message = "another operation is still running";
    app.state::<Arc<Controller>>().log.push(format!("error: {message}"));
    Err(message.into())
}
