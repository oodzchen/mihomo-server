//! Commands of the bundled status page (see `capabilities/setup.json`).
use crate::controller::{self, Controller, Task};
use crate::model::Service;
use crate::window;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, State};

#[derive(Serialize)]
pub struct LocalState {
    state: &'static str,
    detail: Option<String>,
    address: Option<String>,
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
    let (state, detail, address, version) = match snapshot.service {
        Service::Detecting => ("detecting", None, None, None),
        Service::NotInstalled => ("not_installed", None, None, None),
        Service::Inactive => ("inactive", None, None, None),
        Service::Unreachable(reason) => ("unreachable", Some(reason), None, None),
        Service::Running(live) => (
            "running",
            None,
            Some(live.address.clone()),
            live.status
                .get("version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        ),
    };
    LocalState {
        state,
        detail,
        address,
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
pub async fn open_dashboard(app: AppHandle) {
    window::open_dashboard(&app);
}

fn started(accepted: bool) -> Result<(), String> {
    accepted
        .then_some(())
        .ok_or_else(|| "another operation is still running".into())
}
