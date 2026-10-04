//! Two windows with different trust. `main` shows the service's own
//! management page and is granted no capability; `setup` is the bundled
//! status page and is the only window allowed to call the app's commands.
use crate::{VERSION, controller::Controller};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager as _, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

const TITLE: &str = "Mihomo Server";

/// Which instance (origin and token) the dashboard window was opened for.
#[derive(Default)]
pub struct WindowState(Mutex<Option<String>>);

fn reveal(window: &WebviewWindow) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// The status page's own origin (custom protocol, or its http form).
fn is_bundled(url: &Url) -> bool {
    url.scheme() == "tauri" || (matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost"))
}

fn desktop_version_script() -> String {
    format!(
        "Object.defineProperty(window, '__MIHOMO_DESKTOP_VERSION__', {{ value: {} }});",
        serde_json::to_string(VERSION).expect("desktop version is serializable")
    )
}

pub fn open_service_page(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("setup") {
        reveal(&window);
        return;
    }
    let built = WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("index.html".into()))
        .title(TITLE)
        .inner_size(560.0, 520.0)
        .min_inner_size(420.0, 460.0)
        .on_navigation(is_bundled)
        .build();
    if let Err(error) = built {
        eprintln!("cannot open the status page: {error}");
    }
}

/// The management page, logged in through the URL fragment the web UI reads
/// and immediately removes. Falls back to the status page when not connected.
pub fn open_dashboard(app: &AppHandle) {
    let Some(connection) = app.state::<Arc<Controller>>().connection() else {
        open_service_page(app);
        return;
    };
    let origin = connection.endpoint.management_url.clone();
    let key = format!("{origin}\n{}", connection.token);
    let state = app.state::<WindowState>();
    let mut opened = state.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(window) = app.get_webview_window("main") {
        if opened.as_deref() == Some(key.as_str()) {
            reveal(&window);
            return;
        }
        // Another instance or a rotated token: start a fresh session.
        let _ = window.destroy();
    }
    let Ok(url) = Url::parse(&format!("{origin}/#token={}", connection.token)) else {
        return;
    };
    let allowed = origin.clone();
    let built = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title(TITLE)
        .inner_size(1280.0, 860.0)
        .min_inner_size(760.0, 520.0)
        // The remote management page has no desktop IPC capability. Expose
        // only this immutable build value so it can identify the host client.
        .initialization_script(desktop_version_script())
        // Let the page's own file inputs receive dropped files.
        .disable_drag_drop_handler()
        .on_navigation(move |url| url.origin().ascii_serialization() == allowed)
        .build();
    match built {
        Ok(window) => {
            let hidden = window.clone();
            window.on_window_event(move |event| {
                // Keep the page (and any unsaved edits) while living in the tray.
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = hidden.hide();
                }
            });
            *opened = Some(key);
        }
        Err(error) => eprintln!("cannot open the dashboard: {error}"),
    }
}

/// The dashboard when the service is reachable, else the status page.
pub fn open_preferred(app: &AppHandle) {
    if app.state::<Arc<Controller>>().connection().is_some() {
        open_dashboard(app);
    } else {
        open_service_page(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_bundled_origin_counts_as_local() {
        assert!(is_bundled(&Url::parse("tauri://localhost/index.html").unwrap()));
        assert!(is_bundled(&Url::parse("http://tauri.localhost/index.html").unwrap()));
        assert!(!is_bundled(&Url::parse("http://127.0.0.1:9090/").unwrap()));
        assert!(!is_bundled(&Url::parse("http://tauri.localhost.example/").unwrap()));
    }
    #[test]
    fn dashboard_receives_the_desktop_build_version() {
        let script = desktop_version_script();
        assert!(script.contains("__MIHOMO_DESKTOP_VERSION__"));
        assert!(script.contains(&serde_json::to_string(VERSION).unwrap()));
    }
}
