//! Two windows with different trust. `main` shows the service's own
//! management page and may call only the client's start-at-login commands,
//! granted for that page's origin; `setup` is the bundled status page and the
//! only window allowed to call the other commands.
use crate::{VERSION, controller::Controller};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use tauri::{
    AppHandle, Manager as _, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent, ipc::CapabilityBuilder,
    webview::NewWindowResponse,
};

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

/// Origins already granted the dashboard capability (capabilities cannot be removed).
#[derive(Default)]
pub struct Granted(Mutex<BTreeSet<String>>);

fn dashboard_capability(index: usize, origin: &str) -> CapabilityBuilder {
    CapabilityBuilder::new(format!("dashboard-{index}"))
        .remote(format!("{}/*", origin.trim_end_matches('/')))
        .local(false)
        .window("main")
        .permission("allow-client-autostart")
        .permission("allow-set-client-autostart")
}

/// Let the management page loaded from ORIGIN, and nothing else, read and
/// change the client's start at login.
fn grant_dashboard(app: &AppHandle, origin: &str) {
    let granted = app.state::<Granted>();
    let mut granted = granted.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if granted.contains(origin) {
        return;
    }
    match app.add_capability(dashboard_capability(granted.len(), origin)) {
        Ok(()) => {
            granted.insert(origin.to_owned());
        }
        Err(error) => eprintln!("cannot grant the dashboard its commands: {error}"),
    }
}

/// Whether a link the management page opens in a new window may go to the
/// system browser: only pages of the instance it was loaded from.
fn opens_externally(url: &Url, origin: &str) -> bool {
    matches!(url.scheme(), "http" | "https") && url.origin().ascii_serialization() == origin
}

/// Hand URL to the desktop's default browser (the page's "open in browser" link).
fn open_in_browser(url: &Url) {
    match std::process::Command::new("xdg-open").arg(url.as_str()).spawn() {
        // Reap it; xdg-open returns once the browser has the URL.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(error) => eprintln!("cannot open the browser: {error}"),
    }
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
    let mut builder = WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("index.html".into()))
        .title(TITLE)
        .inner_size(560.0, 620.0)
        .min_inner_size(420.0, 540.0)
        .on_navigation(is_bundled);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone()).expect("valid default window icon");
    }
    let built = builder.build();
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
    grant_dashboard(app, &origin);
    let allowed = origin.clone();
    let external = origin.clone();
    let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title(TITLE)
        .inner_size(853.0, 573.0)
        .min_inner_size(760.0, 520.0)
        // Besides the start-at-login commands, expose only this immutable
        // build value so the page can identify the host client.
        .initialization_script(desktop_version_script())
        // Let the page's own file inputs receive dropped files.
        .disable_drag_drop_handler()
        .on_navigation(move |url| url.origin().ascii_serialization() == allowed)
        // target="_blank" links open in the browser, never in a client window.
        .on_new_window(move |url, _| {
            if opens_externally(&url, &external) {
                open_in_browser(&url);
            }
            NewWindowResponse::Deny
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone()).expect("valid default window icon");
    }
    let built = builder.build();
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
    fn only_the_instance_own_pages_open_in_the_browser() {
        let origin = "http://127.0.0.1:9090";
        assert!(opens_externally(
            &Url::parse("http://127.0.0.1:9090/#token=ab").unwrap(),
            origin
        ));
        assert!(!opens_externally(
            &Url::parse("http://127.0.0.1:9091/").unwrap(),
            origin
        ));
        assert!(!opens_externally(&Url::parse("https://example.com/").unwrap(), origin));
        assert!(!opens_externally(&Url::parse("file:///etc/passwd").unwrap(), origin));
    }
    #[test]
    fn dashboard_receives_the_desktop_build_version() {
        let script = desktop_version_script();
        assert!(script.contains("__MIHOMO_DESKTOP_VERSION__"));
        assert!(script.contains(&serde_json::to_string(VERSION).unwrap()));
    }
    #[test]
    fn dashboard_may_only_use_start_at_login_from_its_origin() {
        use tauri::{ipc::RuntimeCapability as _, utils::acl::capability::CapabilityFile};
        let CapabilityFile::Capability(capability) = dashboard_capability(0, "http://127.0.0.1:9090").build() else {
            panic!("one capability");
        };
        assert!(!capability.local);
        assert_eq!(capability.windows, ["main"]);
        let remote = capability.remote.expect("remote origin");
        assert_eq!(remote.urls, ["http://127.0.0.1:9090/*"]);
        let permissions: Vec<String> = capability
            .permissions
            .iter()
            .map(|permission| permission.identifier().get().to_owned())
            .collect();
        assert_eq!(permissions, ["allow-client-autostart", "allow-set-client-autostart"]);
    }
}
