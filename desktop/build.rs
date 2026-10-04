fn main() {
    // Declaring the commands puts them under the capability ACL: only the local
    // status page may call them, never the management page loaded from the service.
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "local_state",
            "install_service",
            "start_service",
            "open_dashboard",
        ])),
    )
    .expect("tauri build");
}
