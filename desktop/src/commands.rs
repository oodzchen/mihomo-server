//! Commands of the bundled status page (see `capabilities/setup.json`).
use crate::controller::{self, Controller};
use crate::model::{Service, Task};
use crate::window;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, State};

#[derive(Serialize)]
pub struct LocalState {
    state: &'static str,
    detail: Option<String>,
    /// The service's own version.
    version: Option<String>,
    task: Task,
    log: Vec<String>,
    last_error: Option<String>,
    language: &'static str,
    app_version: &'static str,
}

#[tauri::command]
pub fn local_state(controller: State<'_, Arc<Controller>>) -> LocalState {
    let snapshot = controller.snapshot();
    let (state, detail, version) = match snapshot.service {
        Service::Detecting => ("detecting", None, None),
        Service::NotInstalled => ("not_installed", None, None),
        Service::Inactive => ("inactive", None, None),
        Service::Unreachable(reason) => ("unreachable", Some(reason), None),
        Service::Running(live) => ("running", None, live.service_version),
    };
    LocalState {
        state,
        detail,
        version,
        task: controller.task(),
        log: controller.log.lines(),
        last_error: snapshot.last_error,
        language: controller.language.code(),
        app_version: crate::VERSION,
    }
}

#[tauri::command]
pub async fn install_service(app: AppHandle) -> Result<(), String> {
    started(controller::start_task(&app, Task::Installing, None))
}

#[tauri::command]
pub async fn start_service(app: AppHandle) -> Result<(), String> {
    started(controller::start_task(&app, Task::Starting, None))
}

#[tauri::command]
pub async fn restart_service(app: AppHandle) -> Result<(), String> {
    started(controller::start_task(&app, Task::Restarting, None))
}

#[tauri::command]
pub async fn open_dashboard(app: AppHandle) {
    window::open_dashboard(&app);
}

fn started(accepted: bool) -> Result<(), String> {
    accepted
        .then_some(())
        .ok_or_else(|| "another operation is still running".into())
}
