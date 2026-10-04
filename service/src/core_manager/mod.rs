mod apply;
mod backup;
mod dispatch;
mod geo;
mod lifecycle;
mod messages;
mod profiles;
mod scheduler;
mod selection;
mod settings;
mod upgrade;

use lifecycle::ManagedProcess;
use messages::*;
use selection::Restoration;

use std::{
    collections::{HashMap, VecDeque},
    fs::{File, OpenOptions},
    future::Future,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use headless_core::config::{
    IProfiles, PrfItem, PrfSelected,
    dns::{DnsOverrideOutcome, DnsOverrideState, ProfileDnsSettings},
    profile_store::{
        DEFAULT_GLOBAL_SCRIPT, EnhancementContent, EnhancementPlan, GenerationPlan, GlobalEnhancementKind,
        ProfilePatch, ProfileStore, RawContent, ScriptContent, SequenceKind,
    },
    resources::validate_resource_declarations,
    runtime::{self, RuntimeStore},
    settings::{RuntimeSettings, ServiceSettings, SettingsStore},
};
use mihomo_client::{
    Builder, Mihomo,
    models::{Protocol, Proxies, ProxyType},
};
use parking_lot::Mutex;
use serde::Serialize;
use serde_yaml_ng::Mapping;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncRead, BufReader},
    process::{Child, Command},
    sync::{broadcast, mpsc, oneshot, watch},
    task::JoinHandle,
    time::{Instant, sleep, sleep_until, timeout},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CorePhase {
    Stopped,
    Starting,
    Running,
    Stopping,
    Recovering,
    Failed,
    Shutdown,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoreStatus {
    pub phase: CorePhase,
    pub pid: Option<u32>,
    pub version: Option<String>,
    pub error: Option<String>,
    pub exit_code: Option<i32>,
    pub recovery_attempt: u32,
    pub generation: u64,
    pub config_revision: Option<String>,
    pub active_profile: Option<String>,
    pub selection_pending: Vec<String>,
    pub selection_error: Option<String>,
}

impl Default for CoreStatus {
    fn default() -> Self {
        Self {
            phase: CorePhase::Stopped,
            pid: None,
            version: None,
            error: None,
            exit_code: None,
            recovery_attempt: 0,
            generation: 0,
            config_revision: None,
            active_profile: None,
            selection_pending: Vec::new(),
            selection_error: None,
        }
    }
}

pub(crate) fn same_proxy_snapshot(before: &CoreStatus, after: &CoreStatus) -> bool {
    after.phase == CorePhase::Running
        && before.generation == after.generation
        && before.pid == after.pid
        && before.config_revision == after.config_revision
}

#[derive(Debug, Clone, Serialize)]
pub struct CoreActivation {
    pub upgraded: bool,
    pub from: String,
    pub to: String,
    pub installation: crate::core_upgrade::CoreInstallation,
    pub status: CoreStatus,
}

/// Preserve upstream's stable-upgrade result shape.
#[derive(Debug, Clone, Serialize)]
pub struct CoreUpgradeReport {
    pub upgraded: bool,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoreLog {
    pub stream: &'static str,
    pub message: String,
}

#[derive(Clone)]
struct Logs {
    tail: Arc<Mutex<VecDeque<CoreLog>>>,
    events: broadcast::Sender<CoreLog>,
}

impl Logs {
    fn append(&self, stream: &'static str, message: String) {
        let line = CoreLog { stream, message };
        let mut tail = self.tail.lock();
        if tail.len() == 200 {
            tail.pop_front();
        }
        tail.push_back(line.clone());
        drop(tail);
        let _ = self.events.send(line);
    }
}

#[derive(Debug, Clone)]
pub struct LifecyclePolicy {
    pub selection_timeout: Duration,
    pub selection_first_pass: Duration,
    pub selection_settle: Duration,
    pub selection_interval: Duration,
    pub validation_timeout: Duration,
    pub script_timeout: Duration,
    pub readiness_attempts: usize,
    pub probe_timeout: Duration,
    pub probe_interval: Duration,
    pub stop_timeout: Duration,
    pub kill_timeout: Duration,
    pub recovery_limit: u32,
    pub recovery_backoff: Duration,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            selection_timeout: Duration::from_secs(10),
            selection_first_pass: Duration::from_secs(3),
            selection_settle: Duration::from_secs(30),
            selection_interval: Duration::from_secs(1),
            validation_timeout: Duration::from_secs(5),
            script_timeout: Duration::from_secs(5),
            readiness_attempts: 30,
            probe_timeout: Duration::from_millis(400),
            probe_interval: Duration::from_millis(100),
            stop_timeout: Duration::from_secs(5),
            kill_timeout: Duration::from_secs(5),
            recovery_limit: 3,
            recovery_backoff: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CoreOptions {
    pub script_worker: Option<PathBuf>,
    pub binary: PathBuf,
    pub data_dir: PathBuf,
    pub config: PathBuf,
    pub policy: LifecyclePolicy,
    pub resources: Option<crate::resources::Resources>,
    pub core_dir: Option<PathBuf>,
    /// Shared multi-user installation: rewrite ports/TUN routing into this
    /// user's slot; the managed core stays private and upgradable per user.
    pub isolation: Option<headless_core::enhance::isolation::Isolation>,
    /// Whether this user may create TUN devices; known only in multi-user mode.
    pub(crate) tun_capable: Option<bool>,
    /// The installation's capability launcher the core is started through.
    pub(crate) tun_exec: Option<PathBuf>,
}

impl CoreOptions {
    pub fn new(binary: PathBuf, data_dir: PathBuf, config: PathBuf) -> Self {
        Self {
            script_worker: None,
            binary,
            data_dir,
            config,
            policy: LifecyclePolicy::default(),
            resources: None,
            core_dir: None,
            isolation: None,
            tun_capable: None,
            tun_exec: None,
        }
    }

    /// The core lives in the data directory and can be upgraded from the Web.
    pub(crate) fn managed_core(&self) -> bool {
        self.resources.is_some()
    }

    fn prepare(&mut self) -> Result<(File, String)> {
        ensure!(
            self.policy.selection_timeout > Duration::ZERO
                && self.policy.selection_first_pass > Duration::ZERO
                && self.policy.selection_settle > Duration::ZERO
                && self.policy.selection_interval > Duration::ZERO,
            "selection timeouts and retry interval must be positive"
        );
        ensure!(
            self.policy.readiness_attempts > 0,
            "readiness attempts must be positive"
        );
        ensure!(self.policy.recovery_limit <= 10, "recovery limit must not exceed 10");
        ensure!(
            self.policy.script_timeout > Duration::ZERO && self.policy.script_timeout <= Duration::from_secs(5),
            "script timeout must be positive and at most 5 seconds"
        );
        let cwd = std::env::current_dir()?;
        if self.config.is_relative() {
            self.config = cwd.join(&self.config);
        }
        if self.binary.is_relative() && self.binary.components().count() > 1 {
            self.binary = cwd.join(&self.binary);
        }
        std::fs::create_dir_all(&self.data_dir)?;
        self.data_dir = self.data_dir.canonicalize()?;
        let mut open = OpenOptions::new();
        open.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            open.mode(0o600);
        }
        let lock = open.open(self.data_dir.join(".mihomo-server.lock"))?;
        lock.try_lock().context("another service owns this data directory")?;
        #[cfg(unix)]
        crate::geo::live::recover(&self.data_dir).context("live Geo rollback recovery failed")?;
        if let Some(resources) = &self.resources {
            let core_directory = self.core_dir.clone().unwrap_or_else(|| self.data_dir.join("core"));
            crate::core_upgrade::recover(&core_directory).context("managed core upgrade recovery failed")?;
            self.binary = resources.initialize_core(&core_directory)?;
            if self.isolation.is_some() {
                #[cfg(target_os = "linux")]
                {
                    // Only the system TUN owner may create a TUN once one exists.
                    let reserved = self.isolation.is_some_and(|isolation| {
                        matches!(
                            isolation.tun_scope(),
                            headless_core::enhance::isolation::TunScope::Reserved(_)
                        )
                    });
                    self.tun_exec = crate::tun_exec::available().filter(|_| !reserved);
                    self.tun_capable = Some(self.tun_exec.is_some());
                }
                #[cfg(not(target_os = "linux"))]
                bail!("multi-user mode requires Linux");
            }
            resources.initialize_geo(&self.data_dir)?;
        } else {
            ensure!(self.isolation.is_none(), "multi-user mode requires bundle resources");
            ensure!(self.core_dir.is_none(), "core directory requires bundle resources");
        }
        let run = self.data_dir.join("run");
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&run) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            let metadata = run.metadata()?;
            ensure!(
                metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0,
                "runtime directory must be private: {}",
                run.display()
            );
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(&run)?;
        #[cfg(unix)]
        let socket = run
            .join("core.sock")
            .to_str()
            .context("socket path must be UTF-8")?
            .to_owned();
        #[cfg(windows)]
        let socket = {
            use std::hash::{Hash as _, Hasher as _};
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            self.data_dir.hash(&mut hash);
            format!(r"\\.\pipe\mihomo-server-{:x}", hash.finish())
        };
        Ok((lock, socket))
    }
}

/// This user's share of a multi-user installation, for the Web and logs.
#[derive(Debug, Clone, Serialize)]
pub struct MultiUser {
    pub uid: u32,
    pub slot: u16,
    pub mixed_port: u16,
    pub dns_listen: String,
    pub tun_device: String,
    /// The core runs through the capability launcher (the user is in the TUN group).
    pub tun_capable: bool,
    /// The installing user, whose TUN captures the whole host and its resolver.
    pub tun_owner: Option<u32>,
}

#[derive(Clone)]
pub struct CoreManager {
    multi_user: Option<MultiUser>,
    backup_admission: Arc<tokio::sync::Semaphore>,
    core_release_admission: Arc<tokio::sync::Semaphore>,
    core_downloads: Option<Arc<crate::core_release::CoreDownloads>>,
    remote_admission: Arc<tokio::sync::Semaphore>,
    commands: mpsc::Sender<CommandMessage>,
    state: watch::Receiver<CoreStatus>,
    shutdown: watch::Sender<bool>,
    completion: watch::Receiver<Option<std::result::Result<(), String>>>,
    scheduler_completion: watch::Receiver<bool>,
    logs: Logs,
    client: Arc<Mihomo>,
    profiles: watch::Receiver<IProfiles>,
}

impl CoreManager {
    pub fn multi_user(&self) -> Option<&MultiUser> {
        self.multi_user.as_ref()
    }

    /// Send one command to the actor and await its reply.
    async fn call<T>(
        &self,
        message: impl FnOnce(oneshot::Sender<Result<T>>) -> CommandMessage,
        cancelled: &'static str,
    ) -> Result<T> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(message(reply))
            .await
            .context("core manager stopped")?;
        result.await.context(cancelled)?
    }

    pub fn spawn(mut options: CoreOptions) -> Result<Self> {
        let (lock, socket) = options.prepare()?;
        let multi_user = options.isolation.map(|isolation| MultiUser {
            uid: isolation.uid(),
            slot: isolation.slot(),
            mixed_port: isolation.mixed_port(),
            dns_listen: isolation.dns_listen(),
            tun_device: isolation.tun_device(),
            tun_capable: options.tun_capable == Some(true),
            tun_owner: match isolation.tun_scope() {
                headless_core::enhance::isolation::TunScope::Own => None,
                headless_core::enhance::isolation::TunScope::System => Some(isolation.uid()),
                headless_core::enhance::isolation::TunScope::Reserved(owner) => Some(owner),
            },
        });
        let core_downloads = if options.managed_core() {
            Some(Arc::new(crate::core_release::CoreDownloads::new(
                options.binary.parent().context("managed core directory missing")?,
            )?))
        } else {
            None
        };
        #[cfg(unix)]
        crate::backup::candidates::cleanup(&options.data_dir)?;
        #[cfg(target_os = "linux")]
        crate::backup::storage::recover(&options.data_dir)?;
        let store = RuntimeStore::open(&options.data_dir)?;
        let mut settings_store = SettingsStore::open(&options.data_dir)?;
        let mut profile_store = ProfileStore::open(&options.data_dir)?;
        #[cfg(unix)]
        profile_store.recover_restore(&mut settings_store, &store)?;
        settings_store.recover(store.state().current.as_ref())?;
        profile_store.recover_import()?;
        profile_store.recover_refresh(store.state().current.as_ref())?;
        profile_store.recover_enhancement(store.state().current.as_ref())?;
        profile_store.recover_delete_with_settings(&mut settings_store)?;
        settings_store.prune_profile_dns(&profile_store.snapshot())?;
        let settings = settings_store.snapshot();
        profile_store.set_current(store.state().active_profile.as_deref())?;
        profile_store.ensure_global_defaults()?;
        let (profile_state, profiles) = watch::channel(profile_store.snapshot());
        if let Some(path) = store.current_path()? {
            options.config = path;
        }
        let client = Arc::new(
            Builder::new()
                .protocol(Protocol::LocalSocket)
                .socket_path(&socket)
                .build()?,
        );
        let (commands, receiver) = mpsc::channel(32);
        let initial = CoreStatus {
            config_revision: store.state().current.map(|revision| revision.file),
            active_profile: store.state().active_profile,
            ..CoreStatus::default()
        };
        let (status, state) = watch::channel(initial);
        let (shutdown, shutdown_rx) = watch::channel(false);
        let (finished, completion) = watch::channel(None);
        let (events, _) = broadcast::channel(200);
        let logs = Logs {
            tail: Arc::new(Mutex::new(VecDeque::new())),
            events,
        };
        let mut actor = Actor {
            dns_confirmations: HashMap::new(),
            settings_store,
            settings,
            profile_store,
            profile_state,
            store,
            options,
            socket,
            _lock: lock,
            client: Arc::clone(&client),
            logs: logs.clone(),
            status,
            shutdown: shutdown_rx,
            receiver,
            process: None,
            retry_at: None,
            restoration: None,
        };
        tokio::spawn(async move {
            let result = actor.run().await.map_err(|error| format!("{error:#}"));
            // Release directory ownership only after the child has been reaped.
            drop(actor);
            finished.send_replace(Some(result));
        });
        let (scheduler_finished, scheduler_completion) = watch::channel(false);
        let manager = Self {
            multi_user,
            backup_admission: Arc::new(tokio::sync::Semaphore::new(1)),
            core_release_admission: Arc::new(tokio::sync::Semaphore::new(1)),
            core_downloads,
            scheduler_completion,
            remote_admission: Arc::new(tokio::sync::Semaphore::new(4)),
            commands,
            state,
            shutdown,
            completion,
            logs,
            client,
            profiles,
        };
        let access = scheduler::Access::new(&manager);
        tokio::spawn(async move {
            scheduler::run(access).await;
            scheduler_finished.send_replace(true);
        });
        Ok(manager)
    }

    pub fn status(&self) -> CoreStatus {
        self.state.borrow().clone()
    }

    pub fn subscribe_status(&self) -> watch::Receiver<CoreStatus> {
        self.state.clone()
    }

    pub fn subscribe_logs(&self) -> broadcast::Receiver<CoreLog> {
        self.logs.events.subscribe()
    }

    pub fn logs(&self) -> Vec<CoreLog> {
        self.logs.tail.lock().iter().cloned().collect()
    }

    pub fn client(&self) -> Arc<Mihomo> {
        Arc::clone(&self.client)
    }

    pub async fn rules(&self) -> Result<mihomo_client::models::Rules> {
        ensure!(self.status().phase == CorePhase::Running, "core is not running");
        tokio::time::timeout(std::time::Duration::from_secs(10), self.client.get_rules())
            .await
            .map_err(|_| anyhow::anyhow!("rules query timed out"))?
            .context("failed to query rules from core")
    }

    pub async fn rule_providers(&self) -> Result<mihomo_client::models::RuleProviders> {
        ensure!(self.status().phase == CorePhase::Running, "core is not running");
        tokio::time::timeout(std::time::Duration::from_secs(10), self.client.get_rule_providers())
            .await
            .map_err(|_| anyhow::anyhow!("rule providers query timed out"))?
            .context("failed to query rule providers from core")
    }

    pub async fn update_rule_provider(&self, name: &str) -> Result<()> {
        ensure!(self.status().phase == CorePhase::Running, "core is not running");
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.client.update_rule_provider(name),
        )
        .await
        .map_err(|_| anyhow::anyhow!("update rule provider timed out"))?
        .with_context(|| format!("failed to update rule provider '{name}'"))
    }

    pub async fn proxy_providers(&self) -> Result<mihomo_client::models::ProxyProviders> {
        ensure!(self.status().phase == CorePhase::Running, "core is not running");
        tokio::time::timeout(std::time::Duration::from_secs(10), self.client.get_proxy_providers())
            .await
            .map_err(|_| anyhow::anyhow!("proxy providers query timed out"))?
            .context("failed to query proxy providers from core")
    }

    pub async fn update_proxy_provider(&self, name: &str) -> Result<()> {
        ensure!(self.status().phase == CorePhase::Running, "core is not running");
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.client.update_proxy_provider(name),
        )
        .await
        .map_err(|_| anyhow::anyhow!("update proxy provider timed out"))?
        .with_context(|| format!("failed to update proxy provider '{name}'"))
    }

    pub async fn healthcheck_proxy_provider(&self, name: &str) -> Result<()> {
        ensure!(self.status().phase == CorePhase::Running, "core is not running");
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            self.client.healthcheck_proxy_provider(name),
        )
        .await
        .map_err(|_| anyhow::anyhow!("healthcheck proxy provider timed out"))?
        .with_context(|| format!("failed to healthcheck proxy provider '{name}'"))
    }

    pub async fn delay_proxy(
        &self,
        name: &str,
        test_url: Option<&str>,
        timeout_ms: Option<u32>,
    ) -> Result<mihomo_client::models::ProxyDelay> {
        ensure!(self.status().phase == CorePhase::Running, "core is not running");
        let url = test_url
            .filter(|u| !u.trim().is_empty())
            .unwrap_or("http://www.gstatic.com/generate_204");
        let timeout = timeout_ms.unwrap_or(5000).max(100);
        let req_timeout = std::time::Duration::from_millis(timeout as u64) + std::time::Duration::from_secs(5);
        tokio::time::timeout(req_timeout, self.client.delay_proxy_by_name(name, url, timeout))
            .await
            .map_err(|_| anyhow::anyhow!("delay proxy query timed out"))?
            .with_context(|| format!("failed to test delay for proxy '{name}'"))
    }

    pub async fn delay_group(
        &self,
        group: &str,
        test_url: Option<&str>,
        timeout_ms: Option<u32>,
    ) -> Result<std::collections::HashMap<String, u32>> {
        ensure!(self.status().phase == CorePhase::Running, "core is not running");
        let url = test_url
            .filter(|u| !u.trim().is_empty())
            .unwrap_or("http://www.gstatic.com/generate_204");
        let timeout = timeout_ms.unwrap_or(5000).max(100);
        let req_timeout = std::time::Duration::from_millis(timeout as u64) + std::time::Duration::from_secs(10);
        tokio::time::timeout(req_timeout, self.client.delay_group(group, url, timeout))
            .await
            .map_err(|_| anyhow::anyhow!("delay group query timed out"))?
            .with_context(|| format!("failed to test delay for group '{group}'"))
    }
}

struct Actor {
    dns_confirmations: HashMap<String, String>,
    settings_store: SettingsStore,
    settings: ServiceSettings,
    profile_store: ProfileStore,
    profile_state: watch::Sender<IProfiles>,
    store: RuntimeStore,
    options: CoreOptions,
    socket: String,
    _lock: File,
    client: Arc<Mihomo>,
    logs: Logs,
    status: watch::Sender<CoreStatus>,
    shutdown: watch::Receiver<bool>,
    receiver: mpsc::Receiver<CommandMessage>,
    process: Option<ManagedProcess>,
    retry_at: Option<Instant>,
    restoration: Option<Restoration>,
}

async fn closing(receiver: &mut watch::Receiver<bool>) {
    if !*receiver.borrow() {
        let _ = receiver.changed().await;
    }
}

fn drain<R: AsyncRead + Unpin + Send + 'static>(reader: R, logs: Logs, stream: &'static str) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => logs.append(stream, line),
                Ok(None) => break,
                Err(error) => {
                    logs.append(stream, format!("log reader failed: {error}"));
                    break;
                }
            }
        }
    })
}

impl Actor {
    fn publish(&self, phase: CorePhase, error: Option<String>) {
        self.status.send_modify(|state| {
            state.phase = phase;
            state.error = error;
            state.pid = self.process.as_ref().and_then(|process| process.child.id());
            if phase != CorePhase::Running {
                state.version = None;
            }
        });
    }

    async fn run(&mut self) -> Result<()> {
        let operation = self.run_loop().await;
        self.retry_at = None;
        self.cancel_restoration();
        self.receiver.close();
        self.publish(CorePhase::Stopping, None);
        let cleanup = self.stop_process().await;
        match operation.and(cleanup) {
            Ok(()) => {
                self.publish(CorePhase::Shutdown, None);
                Ok(())
            }
            Err(error) => {
                self.publish(CorePhase::Failed, Some(format!("{error:#}")));
                Err(error)
            }
        }
    }

    async fn run_loop(&mut self) -> Result<()> {
        let mut monitor = tokio::time::interval(Duration::from_millis(100));
        loop {
            let retry = self
                .retry_at
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400));
            let restore = self
                .restoration
                .as_ref()
                .map(|restore| restore.next_at)
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400));
            tokio::select! {
                biased;
                _ = closing(&mut self.shutdown) => break,
                request = self.receiver.recv() => {
                    let Some(request) = request else { break; };
                    self.handle(request).await;
                }
                _ = monitor.tick() => self.observe_exit().await?,
                _ = sleep_until(retry), if self.retry_at.is_some() => {
                    self.retry_at = None;
                    if self.start_core().await.is_err() { self.schedule_recovery(); }
                }
                _ = sleep_until(restore), if self.restoration.is_some() => {
                    let deadline = self.restoration.as_ref().expect("restoration scheduled").deadline;
                    self.restore_step(deadline).await;
                }
            }
        }
        Ok(())
    }
}

async fn check_config(path: &Path) -> Result<()> {
    read_config(path).await?;
    Ok(())
}

async fn read_config(path: &Path) -> Result<Mapping> {
    ensure!(
        tokio::fs::metadata(path).await?.len() <= runtime::MAX_CONFIG_BYTES as u64,
        "configuration exceeds 8 MiB"
    );
    let yaml = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("cannot read configuration {}", path.display()))?;
    runtime::parse(&yaml)
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod upgrade_tests;
