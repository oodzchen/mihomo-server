//! Client state shared by the tray, the windows and the status page: the
//! connection to the local instance, the polled snapshot and running tasks.
use crate::{
    i18n::Language,
    local::{self, Detected, Log},
    model::{self, Action, Live, MenuModel, Service, Snapshot, Task},
    notify, settings, tray, window,
};
use anyhow::{Context as _, Result};
use management_client::{Api, Endpoint, events::Feed};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager as _};
use tokio::sync::Notify;

const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Delay tests, downloads and restarts are bounded by the service itself.
const ACTION_TIMEOUT: Duration = Duration::from_secs(180);
/// Proxies and subscriptions change without a status change (URLTest picks,
/// selections made in the dashboard), so they are also re-read on this period.
const FULL_REFRESH: Duration = Duration::from_secs(60);

/// Never implements Debug: it holds the management token.
pub struct Connection {
    pub api: Api,
    pub endpoint: Endpoint,
    pub token: String,
    pub service_version: Option<String>,
}

struct Inner {
    /// The instance's shared interface language, else the saved one, else the
    /// system locale.
    language: Language,
    /// The saved copy of the shared language (see `settings`).
    saved: Option<Language>,
    /// The status page's temporary proxy for installation, never saved.
    proxy: Option<String>,
    service: Service,
    /// Whether the detected-but-stopped unit is enabled at boot.
    enabled: bool,
    connection: Option<Arc<Connection>>,
    task: Task,
    last_error: Option<String>,
    /// The install or lifecycle task that failed last, until the next task.
    failed: Option<Task>,
    force_full: bool,
}

pub struct Controller {
    system_language: Language,
    settings: settings::Store,
    inner: Mutex<Inner>,
    wake: Notify,
    pub log: Log,
}

impl Controller {
    pub fn new(system_language: Language, settings: settings::Store) -> Self {
        let saved = settings.language();
        Self {
            system_language,
            settings,
            inner: Mutex::new(Inner {
                language: saved.unwrap_or(system_language),
                saved,
                proxy: None,
                service: Service::Detecting,
                enabled: false,
                connection: None,
                task: Task::Idle,
                last_error: None,
                failed: None,
                force_full: true,
            }),
            wake: Notify::new(),
            log: Log::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn snapshot(&self) -> Snapshot {
        let inner = self.lock();
        Snapshot {
            service: inner.service.clone(),
            task: inner.task,
            last_error: inner.last_error.clone(),
        }
    }

    pub fn connection(&self) -> Option<Arc<Connection>> {
        self.lock().connection.clone()
    }

    pub fn task(&self) -> Task {
        self.lock().task
    }

    pub fn language(&self) -> Language {
        self.lock().language
    }

    /// Follow the instance's preference (`None`: the system locale) and keep
    /// a copy of it; whether the language changed.
    fn set_language(&self, preference: Option<Language>) -> bool {
        let language = preference.unwrap_or(self.system_language);
        let (changed, save) = {
            let mut inner = self.lock();
            let save = std::mem::replace(&mut inner.saved, preference) != preference;
            (std::mem::replace(&mut inner.language, language) != language, save)
        };
        if save && let Err(error) = self.settings.save_language(preference) {
            eprintln!("cannot save the interface language: {error:#}");
        }
        changed
    }

    fn saved_language(&self) -> Option<Language> {
        self.lock().saved
    }

    pub fn proxy(&self) -> Option<String> {
        self.lock().proxy.clone()
    }

    pub fn set_proxy(&self, proxy: Option<String>) {
        self.lock().proxy = proxy;
    }

    pub fn failed(&self) -> Option<Task> {
        self.lock().failed
    }

    /// Refresh now instead of at the next period.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    fn set_service(&self, service: Service) {
        self.lock().service = service;
    }

    fn drop_connection(&self) {
        self.lock().connection = None;
    }

    pub fn model(&self) -> MenuModel {
        model::derive(&self.snapshot(), self.language().strings())
    }
}

fn controller(app: &AppHandle) -> Arc<Controller> {
    app.state::<Arc<Controller>>().inner().clone()
}

/// Recompute the menu; the tray applies only what changed.
pub fn publish(app: &AppHandle) {
    tray::apply(app, controller(app).model());
}

fn short(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

async fn timed<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(READ_TIMEOUT, future)
        .await
        .context("the management API did not answer in time")?
}

/// What the poller keeps between rounds to decide what to re-read.
#[derive(Default)]
struct Poll {
    fingerprint: String,
    endpoint: Option<Endpoint>,
    full_at: Option<Instant>,
    user: Value,
    proxies: Value,
    profiles: Value,
    failures: u32,
}

impl Poll {
    async fn connect(&self, controller: &Controller) -> Result<Option<Arc<Connection>>> {
        let detected = tokio::task::spawn_blocking(local::detect).await??;
        let (endpoint, service_version) = match detected {
            Detected::Running { endpoint, version } => (endpoint, version),
            Detected::Inactive { enabled } => {
                let mut inner = controller.lock();
                inner.service = Service::Inactive;
                inner.enabled = enabled;
                return Ok(None);
            }
            Detected::NotInstalled => {
                controller.set_service(Service::NotInstalled);
                return Ok(None);
            }
        };
        let token = local::read_token(&endpoint)?;
        let api = Api::with_token(endpoint.clone(), token.clone())?.with_timeout(ACTION_TIMEOUT);
        // Adopt the instance's language before the first menu is shown; older
        // services have no preferences and keep the saved or system language.
        // A language chosen while no instance ran becomes the instance's.
        if let Ok(preferences) = timed(api.command("preferences", json!({}))).await {
            let mut shared = preference(&preferences);
            if shared.is_none()
                && let Some(saved) = controller.saved_language()
                && timed(api.command("set_language", json!({"language": saved.code()})))
                    .await
                    .is_ok()
            {
                shared = Some(saved);
            }
            controller.set_language(shared);
        }
        let api = api.with_language(controller.language().code());
        let connection = Arc::new(Connection {
            api,
            endpoint,
            token,
            service_version,
        });
        controller.lock().connection = Some(connection.clone());
        Ok(Some(connection))
    }

    async fn read(&mut self, connection: &Connection, force: bool) -> Result<Live> {
        let api = &connection.api;
        let (status, access) = tokio::try_join!(
            timed(api.command("status", json!({}))),
            timed(api.command("proxy_access", json!({})))
        )?;
        let fingerprint = format!("{status}{access}");
        let stale = force
            || fingerprint != self.fingerprint
            || self.endpoint.as_ref() != Some(&connection.endpoint)
            || self.full_at.is_none_or(|at| at.elapsed() >= FULL_REFRESH);
        if stale {
            // A stopped core has no proxies; that is a state, not a failure.
            let (proxies, profiles, user) = tokio::join!(
                timed(api.command("proxies", json!({}))),
                timed(api.command("profiles", json!({}))),
                timed(api.command("multi_user", json!({})))
            );
            self.proxies = proxies.unwrap_or(Value::Null);
            self.profiles = profiles?;
            self.user = user.unwrap_or(Value::Null);
            self.fingerprint = fingerprint;
            self.endpoint = Some(connection.endpoint.clone());
            self.full_at = Some(Instant::now());
        }
        Ok(Live {
            service_version: connection.service_version.clone(),
            status,
            access,
            user: self.user.clone(),
            proxies: self.proxies.clone(),
            profiles: self.profiles.clone(),
        })
    }

    fn backoff(&mut self) -> Duration {
        self.failures = (self.failures + 1).min(5);
        Duration::from_secs(2u64.pow(self.failures).min(30))
    }

    /// One round; returns how long to wait before the next one.
    async fn refresh(&mut self, controller: &Controller) -> Duration {
        let force = std::mem::take(&mut controller.lock().force_full);
        let busy = controller.task() != Task::Idle;
        let mut failure = None;
        // A failed read reconnects once: the service may have restarted on
        // another port, stopped, or rotated its token.
        for _ in 0..2 {
            let connection = match controller.connection() {
                Some(connection) => connection,
                None => match self.connect(controller).await {
                    Ok(Some(connection)) => connection,
                    Ok(None) => {
                        self.failures = 0;
                        return Duration::from_secs(if busy { 2 } else { 5 });
                    }
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                },
            };
            match self.read(&connection, force || failure.is_some()).await {
                Ok(live) => {
                    let transitional = !matches!(
                        live.status.get("phase").and_then(Value::as_str),
                        Some("running" | "stopped" | "failed")
                    );
                    controller.set_service(Service::Running(Box::new(live)));
                    self.failures = 0;
                    return Duration::from_secs(if transitional || busy { 1 } else { 5 });
                }
                Err(error) => {
                    controller.drop_connection();
                    failure = Some(error);
                }
            }
        }
        let reason = failure.map_or_else(|| "unknown error".into(), |error| short(&error));
        // The status page shows no reasons; its output panel gets each new one.
        let repeated = matches!(&controller.lock().service, Service::Unreachable(previous) if *previous == reason);
        if !repeated {
            controller.log.push(format!("error: {reason}"));
        }
        controller.set_service(Service::Unreachable(reason));
        self.backoff()
    }
}

/// Poll the local instance for the life of the app.
pub async fn run(app: AppHandle, open_window: bool) {
    let controller = controller(&app);
    let mut poll = Poll::default();
    let mut first = open_window;
    loop {
        let delay = poll.refresh(&controller).await;
        publish(&app);
        if std::mem::take(&mut first) {
            window::open_preferred(&app);
        }
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = controller.wake.notified() => {}
        }
    }
}

/// The language in a `preferences` value; unset or unknown is `None`.
fn preference(preferences: &Value) -> Option<Language> {
    preferences
        .get("language")
        .and_then(Value::as_str)
        .and_then(Language::from_code)
}

/// Follow the instance's interface language as the service pushes it, so a
/// change made in the Web UI (or any other client) reaches the tray at once.
pub async fn follow_preferences(app: AppHandle) {
    let controller = controller(&app);
    let mut failures = 0u32;
    loop {
        let Some(connection) = controller.connection() else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        match Feed::connect(&connection.endpoint, &connection.token, Some("preferences")).await {
            Ok(mut feed) => {
                failures = 0;
                while let Ok(Some(event)) = feed.next().await {
                    if event["type"] == "preferences" {
                        adopt_language(&app, preference(&event["data"]));
                    }
                }
            }
            // Stopped, restarting, or an older service without the feed.
            Err(_) => failures = (failures + 1).min(5),
        }
        tokio::time::sleep(Duration::from_secs(2u64.pow(failures).min(30))).await;
    }
}

fn adopt_language(app: &AppHandle, preference: Option<Language>) {
    let controller = controller(app);
    if controller.set_language(preference) {
        // Service messages follow too: reconnect with the new Accept-Language.
        controller.drop_connection();
        publish(app);
        controller.wake();
    }
}

/// The status page's language choice: the instance's shared preference when
/// one runs (as the Web UI's settings do), else saved until one does.
pub async fn choose_language(app: &AppHandle, language: Language) -> Result<()> {
    if let Some(connection) = controller(app).connection() {
        timed(
            connection
                .api
                .command("set_language", json!({"language": language.code()})),
        )
        .await?;
    }
    adopt_language(app, Some(language));
    Ok(())
}

fn command(action: &Action) -> Option<(&'static str, Value)> {
    Some(match action {
        Action::Mode(mode) => ("set_proxy_mode", json!({"mode": mode})),
        Action::Tun(enabled) => ("set_tun_enabled", json!({"enabled": enabled})),
        Action::Select { group, node } => ("select_node", json!({"group": group, "node": node})),
        Action::Unfix { group } => ("unfix_node", json!({"group": group})),
        Action::TestGroup { group } => ("delay_group", json!({"group": group})),
        Action::Profile { uid } => ("select_profile", json!({"uid": uid})),
        _ => return None,
    })
}

/// Start one background task; refuses (returns false) while another runs.
pub fn start_task(app: &AppHandle, task: Task, action: Option<Action>) -> bool {
    let controller = controller(app);
    {
        let mut inner = controller.lock();
        if inner.task != Task::Idle {
            return false;
        }
        inner.task = task;
        inner.last_error = None;
        inner.failed = None;
    }
    publish(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let controller = self::controller(&app);
        let result = match task {
            Task::Installing => local::install(&controller.log, controller.proxy().as_deref()).await,
            // A never-enabled instance is enabled: started now and at boot.
            Task::Starting => {
                let verb = if controller.lock().enabled { "start" } else { "enable" };
                local::run_helper(verb, &controller.log).await
            }
            Task::Stopping => local::run_helper("stop", &controller.log).await,
            Task::Restarting => local::run_helper("restart", &controller.log).await,
            Task::Command => run_command(&controller, action).await,
            Task::Idle => Ok(()),
        };
        {
            let mut inner = controller.lock();
            inner.task = Task::Idle;
            inner.force_full = true;
            if task != Task::Command {
                // Installation and service lifecycle change the endpoint and
                // token; detect again instead of showing the state from before.
                inner.connection = None;
                inner.service = Service::Detecting;
            }
            if let Err(error) = &result {
                inner.last_error = Some(short(error));
                inner.failed = (task != Task::Command).then_some(task);
            }
        }
        if let Err(error) = &result {
            controller.log.push(format!("error: {}", short(error)));
        }
        publish(&app);
        controller.wake();
        if let Err(error) = &result {
            notify::failure(controller.language().strings().failed, &short(error)).await;
        }
    });
    true
}

async fn run_command(controller: &Controller, action: Option<Action>) -> Result<()> {
    let (name, fields) = action.as_ref().and_then(command).context("not a service command")?;
    let connection = controller.connection().context("mihomo-server is not connected")?;
    connection.api.command(name, fields).await.map(drop)
}

/// Run a tray action.
pub fn dispatch(app: &AppHandle, action: Action) {
    match action {
        Action::Quit => app.exit(0),
        Action::OpenDashboard => window::open_dashboard(app),
        Action::OpenServicePage => window::open_service_page(app),
        Action::StartService => {
            start_task(app, Task::Starting, None);
        }
        Action::StopService => {
            start_task(app, Task::Stopping, None);
        }
        Action::RestartService => {
            start_task(app, Task::Restarting, None);
        }
        action => {
            start_task(app, Task::Command, Some(action));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_actions_map_to_management_commands() {
        assert_eq!(
            command(&Action::Mode("global")),
            Some(("set_proxy_mode", json!({"mode": "global"})))
        );
        assert_eq!(
            command(&Action::Select {
                group: "Proxies".into(),
                node: "A".into()
            }),
            Some(("select_node", json!({"group": "Proxies", "node": "A"})))
        );
        assert_eq!(
            command(&Action::Tun(true)),
            Some(("set_tun_enabled", json!({"enabled": true})))
        );
        assert_eq!(
            command(&Action::RestartService),
            None,
            "service lifecycle is not an API command"
        );
        assert_eq!(command(&Action::Quit), None);
    }

    #[test]
    fn the_instance_language_wins_and_unset_falls_back_to_the_system() {
        let controller = Controller::new(Language::En, settings::Store::memory());
        assert!(controller.set_language(preference(&json!({"language": "zhtw"}))));
        assert_eq!(controller.language(), Language::Zhtw);
        assert!(
            !controller.set_language(preference(&json!({"language": "zhtw"}))),
            "unchanged"
        );
        assert!(controller.set_language(preference(&json!({"language": null}))));
        assert_eq!(controller.language(), Language::En);
        assert!(!controller.set_language(preference(&json!({"language": "klingon"}))));
    }

    #[test]
    fn the_shared_language_is_saved_for_the_next_start() {
        let directory = std::env::temp_dir().join(format!("mihomo-desktop-controller-{}", std::process::id()));
        let path = directory.join("settings.json");
        let controller = Controller::new(Language::En, settings::Store::at(path.clone()));
        assert_eq!(
            (controller.language(), controller.saved_language()),
            (Language::En, None)
        );
        controller.set_language(Some(Language::Zh));
        let restarted = Controller::new(Language::En, settings::Store::at(path.clone()));
        assert_eq!(
            (restarted.language(), restarted.saved_language()),
            (Language::Zh, Some(Language::Zh))
        );
        // A cleared preference falls back to the system locale, also after a restart.
        restarted.set_language(None);
        let cleared = Controller::new(Language::En, settings::Store::at(path));
        assert_eq!((cleared.language(), cleared.saved_language()), (Language::En, None));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failures_back_off_to_thirty_seconds() {
        let mut poll = Poll::default();
        let delays: Vec<u64> = (0..7).map(|_| poll.backoff().as_secs()).collect();
        assert_eq!(delays, [2, 4, 8, 16, 30, 30, 30]);
    }
}
