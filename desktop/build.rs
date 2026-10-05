fn main() {
    // Declaring the commands puts them under the capability ACL: the local status
    // page may call its commands, and the management page loaded from the service
    // only the client's own start at login (granted at runtime for its origin).
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "local_state",
            "install_service",
            "start_service",
            "restart_service",
            "open_dashboard",
            "client_autostart",
            "set_client_autostart",
        ])),
    )
    .expect("tauri build");
}
