#[path = "scheduler.rs"]
mod scheduler;

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
        }
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
        if let Some(resources) = &self.resources {
            let core_directory = self.core_dir.clone().unwrap_or_else(|| self.data_dir.join("core"));
            crate::core_upgrade::recover(&core_directory).context("managed core upgrade recovery failed")?;
            self.binary = resources.initialize_core(&core_directory)?;
        } else {
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

enum Operation {
    Start,
    Stop,
    Restart,
    Reload(PathBuf),
    Import(PathBuf),
    Apply(Box<Mapping>),
    Edit(Box<Mapping>),
    Merge(Box<Mapping>),
    SelectProfile(String),
    SelectNode { group: String, node: String },
    UnfixNode(String),
}

struct Request {
    operation: Operation,
    reply: oneshot::Sender<Result<CoreStatus>>,
}

enum ProfileChange {
    Settings(Box<ServiceSettings>),
    RawEdit { revision: String, yaml: String },
    Refresh(Box<headless_core::config::remote::RemoteProfile>),
    Enhancement(Box<EnhancementPlan>),
}

/// Preserve upstream phase order: enhanced candidates must not derive TUN/DNS twice.
enum ConfigCandidate {
    Raw(Mapping),
    Enhanced {
        config: Mapping,
        runtime: Box<RuntimeSettings>,
        dns: Box<DnsOverrideState>,
    },
}

impl From<Mapping> for ConfigCandidate {
    fn from(config: Mapping) -> Self {
        Self::Raw(config)
    }
}

#[derive(Clone, Copy)]
enum ProfileEnhancement {
    Global(GlobalEnhancementKind),
    Merge,
    Sequence(SequenceKind),
    Script,
}

enum CommandMessage {
    InstalledCoreVersion(oneshot::Sender<Result<String>>),
    CheckCoreUpgrade {
        version: String,
        force: bool,
        reply: oneshot::Sender<Result<Option<CoreUpgradeReport>>>,
    },
    UpgradePreparedCore {
        _permit: tokio::sync::OwnedSemaphorePermit,
        prepared: crate::core_release::PreparedCore,
        force: bool,
        downloads: Arc<crate::core_release::CoreDownloads>,
        reply: oneshot::Sender<Result<CoreUpgradeReport>>,
    },
    ActivateCoreUpgrade {
        _permit: tokio::sync::OwnedSemaphorePermit,
        id: String,
        downloads: Arc<crate::core_release::CoreDownloads>,
        reply: oneshot::Sender<Result<CoreActivation>>,
    },
    CoreInstallation(oneshot::Sender<Result<Option<crate::core_upgrade::CoreInstallation>>>),
    StageCoreUpgrade {
        _permit: tokio::sync::OwnedSemaphorePermit,
        id: String,
        downloads: Arc<crate::core_release::CoreDownloads>,
        reply: oneshot::Sender<Result<crate::core_release::StagedCore>>,
    },
    ReadProfileRaw {
        uid: String,
        reply: oneshot::Sender<Result<RawContent>>,
    },
    SetProfileRaw {
        uid: String,
        revision: String,
        yaml: String,
        reply: oneshot::Sender<Result<RawContent>>,
    },
    ReadProfileDns {
        uid: String,
        reply: oneshot::Sender<Result<DnsOverrideState>>,
    },
    SetProfileDns {
        uid: String,
        enabled: bool,
        confirmation: Option<String>,
        reply: oneshot::Sender<Result<DnsOverrideOutcome>>,
    },
    ReadSettings(oneshot::Sender<Result<ServiceSettings>>),
    SetSettings {
        runtime: RuntimeSettings,
        reply: oneshot::Sender<Result<ServiceSettings>>,
    },
    ReadEnhancement {
        uid: String,
        kind: ProfileEnhancement,
        reply: oneshot::Sender<Result<EnhancementContent>>,
    },
    SetEnhancement {
        uid: String,
        kind: ProfileEnhancement,
        yaml: Option<String>,
        reply: oneshot::Sender<Result<PrfItem>>,
    },
    EditProfile {
        uid: String,
        patch: ProfilePatch,
        reply: oneshot::Sender<Result<PrfItem>>,
    },
    DeleteProfile {
        uid: String,
        reply: oneshot::Sender<Result<IProfiles>>,
    },
    RefreshRemote {
        automatic: bool,
        source: Box<PrfItem>,
        profile: Box<headless_core::config::remote::RemoteProfile>,
        reply: oneshot::Sender<Result<PrfItem>>,
    },
    ImportRemote {
        profile: Box<headless_core::config::remote::RemoteProfile>,
        reply: oneshot::Sender<Result<PrfItem>>,
    },
    Control(Request),
    ImportProfileYaml {
        yaml: String,
        name: String,
        reply: oneshot::Sender<Result<PrfItem>>,
    },
    ReadConfig(oneshot::Sender<Result<Mapping>>),
    ImportProfile {
        path: PathBuf,
        name: Option<String>,
        reply: oneshot::Sender<Result<PrfItem>>,
    },
}

#[derive(Clone)]
pub struct CoreManager {
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
    pub fn spawn(mut options: CoreOptions) -> Result<Self> {
        let (lock, socket) = options.prepare()?;
        let core_downloads = if options.resources.is_some() {
            Some(Arc::new(crate::core_release::CoreDownloads::new(
                options.binary.parent().context("managed core directory missing")?,
            )?))
        } else {
            None
        };
        let store = RuntimeStore::open(&options.data_dir)?;
        let mut settings_store = SettingsStore::open(&options.data_dir)?;
        settings_store.recover(store.state().current.as_ref())?;
        let mut profile_store = ProfileStore::open(&options.data_dir)?;
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

    /// Core release network work is independent of the lifecycle actor and subscriptions.
    pub async fn core_release(&self, version: Option<String>) -> Result<crate::core_release::CoreRelease> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        let mut shutdown = self.shutdown.subscribe();
        tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("core release check cancelled during shutdown"),
            result = async { Ok(self.discover_core_release(version.as_deref()).await?.release) } => result,
        }
    }

    pub async fn prepare_core_upgrade(&self, version: Option<String>) -> Result<crate::core_release::PreparedCore> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let downloads = self
            .core_downloads
            .as_ref()
            .context("core preparation requires bundle-managed resources")?;
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        let mut shutdown = self.shutdown.subscribe();
        let cancellation = self.shutdown.subscribe();
        tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("core preparation cancelled during shutdown"),
            result = async { let resolved=self.discover_core_release(version.as_deref()).await?; downloads.prepare_selected(resolved,&cancellation).await } => result,
        }
    }

    pub fn prepared_core_upgrade(&self, id: &str) -> Result<crate::core_release::PreparedCore> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        self.core_downloads
            .as_ref()
            .context("core preparation requires bundle-managed resources")?
            .inspect(id)
    }

    pub async fn stage_core_upgrade(&self, id: String) -> Result<crate::core_release::StagedCore> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let downloads = self
            .core_downloads
            .clone()
            .context("core staging requires bundle-managed resources")?;
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        let (reply, response) = oneshot::channel();
        self.commands
            .send(CommandMessage::StageCoreUpgrade {
                id,
                downloads,
                reply,
                _permit,
            })
            .await
            .context("core manager stopped")?;
        response.await.context("core manager stopped")?
    }

    pub fn staged_core_upgrade(&self, id: &str) -> Result<crate::core_release::StagedCore> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        self.core_downloads
            .as_ref()
            .context("core staging requires bundle-managed resources")?
            .inspect_stage(id)
    }

    pub async fn activate_core_upgrade(&self, id: String) -> Result<CoreActivation> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let downloads = self
            .core_downloads
            .clone()
            .context("core activation requires bundle-managed resources")?;
        let _permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        let (reply, response) = oneshot::channel();
        self.commands
            .send(CommandMessage::ActivateCoreUpgrade {
                id,
                downloads,
                reply,
                _permit,
            })
            .await
            .context("core manager stopped")?;
        response.await.context("core manager stopped")?
    }

    pub async fn core_installation(&self) -> Result<Option<crate::core_upgrade::CoreInstallation>> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(
            self.core_downloads.is_some(),
            "core installation requires bundle-managed resources"
        );
        let (reply, response) = oneshot::channel();
        self.commands
            .send(CommandMessage::CoreInstallation(reply))
            .await
            .context("core manager stopped")?;
        response.await.context("core manager stopped")?
    }

    pub async fn installed_core_version(&self) -> Result<String> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(
            self.core_downloads.is_some(),
            "core upgrade requires bundle-managed resources"
        );
        let (reply, response) = oneshot::channel();
        self.commands
            .send(CommandMessage::InstalledCoreVersion(reply))
            .await
            .context("core manager stopped")?;
        response.await.context("core manager stopped")?
    }

    pub async fn upgrade_clash_core(&self, force: bool) -> Result<CoreUpgradeReport> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let downloads = self
            .core_downloads
            .clone()
            .context("core upgrade requires bundle-managed resources")?;
        let permit = Arc::clone(&self.core_release_admission)
            .try_acquire_owned()
            .context("core release request already in progress")?;
        let mut shutdown = self.shutdown.subscribe();
        let release = tokio::select! {biased;
            _ = closing(&mut shutdown) => bail!("core upgrade cancelled during shutdown"),
            result = self.discover_core_release(None) => result?,
        };
        let (reply, response) = oneshot::channel();
        self.commands
            .send(CommandMessage::CheckCoreUpgrade {
                version: release.release.version.clone(),
                force,
                reply,
            })
            .await
            .context("core manager stopped")?;
        if let Some(report) = response.await.context("core manager stopped")?? {
            return Ok(report);
        }
        let cancellation = self.shutdown.subscribe();
        let prepared = tokio::select! {biased;
            _ = closing(&mut shutdown) => bail!("core upgrade cancelled during shutdown"),
            result = downloads.prepare_selected(release, &cancellation) => result?,
        };
        let (reply, response) = oneshot::channel();
        self.commands
            .send(CommandMessage::UpgradePreparedCore {
                _permit: permit,
                prepared,
                force,
                downloads,
                reply,
            })
            .await
            .context("core manager stopped")?;
        response.await.context("core manager stopped")?
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

    pub fn profiles(&self) -> IProfiles {
        self.profiles.borrow().clone()
    }

    pub fn subscribe_profiles(&self) -> watch::Receiver<IProfiles> {
        self.profiles.clone()
    }

    pub async fn import_profile(&self, path: PathBuf, name: Option<String>) -> Result<PrfItem> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::ImportProfile { path, name, reply })
            .await
            .context("core manager stopped")?;
        result.await.context("profile import cancelled during shutdown")?
    }

    pub async fn profile_raw(&self, uid: String) -> Result<RawContent> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::ReadProfileRaw { uid, reply })
            .await
            .context("core manager stopped")?;
        result.await.context("profile raw read cancelled during shutdown")?
    }

    pub async fn set_profile_raw(&self, uid: String, revision: String, yaml: String) -> Result<RawContent> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(
            uid.len() <= 256 && revision.len() <= 256,
            "profile UID/revision exceeds 256 bytes"
        );
        ensure!(yaml.len() <= runtime::MAX_CONFIG_BYTES, "profile exceeds 8 MiB");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::SetProfileRaw {
                uid,
                revision,
                yaml,
                reply,
            })
            .await
            .context("core manager stopped")?;
        result.await.context("profile raw update cancelled during shutdown")?
    }

    pub async fn profile_merge(&self, uid: String) -> Result<EnhancementContent> {
        self.read_enhancement(uid, ProfileEnhancement::Merge).await
    }

    pub async fn global_merge(&self) -> Result<EnhancementContent> {
        self.read_enhancement("Merge".into(), ProfileEnhancement::Global(GlobalEnhancementKind::Merge))
            .await
    }

    pub async fn global_script(&self) -> Result<ScriptContent> {
        let content = self
            .read_enhancement(
                "Script".into(),
                ProfileEnhancement::Global(GlobalEnhancementKind::Script),
            )
            .await?;
        Ok(ScriptContent {
            uid: content.uid,
            source: content.yaml,
        })
    }

    pub async fn set_global_merge(&self, yaml: Option<String>) -> Result<PrfItem> {
        self.update_enhancement(
            "Merge".into(),
            ProfileEnhancement::Global(GlobalEnhancementKind::Merge),
            yaml,
        )
        .await
    }

    pub async fn set_global_script(&self, source: Option<String>) -> Result<PrfItem> {
        if let Some(source) = &source {
            headless_core::enhance::script::validate_source(source)?;
        }
        self.update_enhancement(
            "Script".into(),
            ProfileEnhancement::Global(GlobalEnhancementKind::Script),
            source,
        )
        .await
    }

    pub async fn profile_sequence(&self, uid: String, kind: SequenceKind) -> Result<EnhancementContent> {
        self.read_enhancement(uid, ProfileEnhancement::Sequence(kind)).await
    }

    pub async fn profile_script(&self, uid: String) -> Result<ScriptContent> {
        let content = self.read_enhancement(uid, ProfileEnhancement::Script).await?;
        Ok(ScriptContent {
            uid: content.uid,
            source: content.yaml,
        })
    }

    pub async fn set_profile_script(&self, uid: String, source: Option<String>) -> Result<PrfItem> {
        if let Some(source) = &source {
            headless_core::enhance::script::validate_source(source)?;
        }
        self.update_enhancement(uid, ProfileEnhancement::Script, source).await
    }

    async fn read_enhancement(&self, uid: String, kind: ProfileEnhancement) -> Result<EnhancementContent> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::ReadEnhancement { uid, kind, reply })
            .await
            .context("core manager stopped")?;
        result.await.context("enhancement read cancelled during shutdown")?
    }

    pub async fn set_profile_merge(&self, uid: String, yaml: Option<String>) -> Result<PrfItem> {
        self.update_enhancement(uid, ProfileEnhancement::Merge, yaml).await
    }

    pub async fn set_profile_sequence(&self, uid: String, kind: SequenceKind, yaml: Option<String>) -> Result<PrfItem> {
        self.update_enhancement(uid, ProfileEnhancement::Sequence(kind), yaml)
            .await
    }

    async fn update_enhancement(&self, uid: String, kind: ProfileEnhancement, yaml: Option<String>) -> Result<PrfItem> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        ensure!(
            yaml.as_ref().is_none_or(|yaml| yaml.len() <= runtime::MAX_CONFIG_BYTES),
            "enhancement exceeds 8 MiB"
        );
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::SetEnhancement { uid, kind, yaml, reply })
            .await
            .context("core manager stopped")?;
        result.await.context("enhancement update cancelled during shutdown")?
    }

    pub async fn edit_profile(&self, uid: String, patch: ProfilePatch) -> Result<PrfItem> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::EditProfile { uid, patch, reply })
            .await
            .context("core manager stopped")?;
        result.await.context("profile edit cancelled during shutdown")?
    }

    pub async fn delete_profile(&self, uid: String) -> Result<IProfiles> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::DeleteProfile { uid, reply })
            .await
            .context("core manager stopped")?;
        result.await.context("profile deletion cancelled during shutdown")?
    }

    pub async fn select_profile(&self, uid: String) -> Result<CoreStatus> {
        self.request(Operation::SelectProfile(uid)).await
    }

    /// Upload content without exposing server filesystem paths to clients.
    pub async fn import_profile_yaml(&self, yaml: String, name: String) -> Result<PrfItem> {
        ensure!(yaml.len() <= runtime::MAX_CONFIG_BYTES, "profile exceeds 8 MiB");
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::ImportProfileYaml { yaml, name, reply })
            .await
            .context("core manager stopped")?;
        result.await.context("profile import cancelled during shutdown")?
    }

    /// Network work cannot block child supervision; only the completed import mutates storage.
    pub async fn import_remote_profile(
        &self,
        url: String,
        name: Option<String>,
        options: crate::remote::RemoteOptions,
    ) -> Result<PrfItem> {
        ensure!(url.len() <= 8192, "subscription URL exceeds 8 KiB");
        ensure!(
            name.as_ref().is_none_or(|name| name.len() <= 256),
            "profile name exceeds 256 bytes"
        );
        ensure!(
            options.user_agent.as_ref().is_none_or(|agent| agent.len() <= 1024),
            "subscription user agent exceeds 1 KiB"
        );
        let mut shutdown = self.shutdown.subscribe();
        let _permit = tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("service is shutting down"),
            permit = Arc::clone(&self.remote_admission).acquire_owned() => permit?,
        };
        let profile = tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("remote import cancelled during shutdown"),
            result = self.download_remote(&url, name.as_deref(), options) => result?,
        };
        let (reply, result) = oneshot::channel();
        tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("remote import cancelled during shutdown"),
            sent = self.commands.send(CommandMessage::ImportRemote { profile: Box::new(profile), reply }) => sent.context("core manager stopped")?,
        }
        result.await.context("remote import cancelled during shutdown")?
    }

    /// The source file guards against a second download overwriting a newer refresh.
    pub async fn refresh_profile(&self, uid: String) -> Result<PrfItem> {
        self.refresh_profile_mode(uid, false).await
    }

    async fn refresh_profile_mode(&self, uid: String, automatic: bool) -> Result<PrfItem> {
        ensure!(uid.len() <= 256, "profile UID exceeds 256 bytes");
        let source = self
            .profiles
            .borrow()
            .items
            .iter()
            .flatten()
            .find(|item| item.uid.as_deref() == Some(uid.as_str()))
            .cloned()
            .context("remote profile not found")?;
        ensure!(
            source.itype.as_deref() == Some("remote"),
            "only remote profiles can be refreshed"
        );
        if automatic {
            ensure!(scheduler::eligible(&source), "automatic subscription update disabled");
        }
        let options = crate::remote::RemoteOptions::from_profile(source.option.as_ref())?;
        let url = source.url.as_deref().context("remote URL missing")?;
        let mut shutdown = self.shutdown.subscribe();
        let _permit = tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("service is shutting down"),
            permit = Arc::clone(&self.remote_admission).acquire_owned() => permit?,
        };
        if automatic {
            let snapshot = self.profiles();
            let current = snapshot
                .items
                .iter()
                .flatten()
                .find(|item| item.uid.as_deref() == Some(uid.as_str()))
                .context("scheduled profile removed while waiting")?;
            ensure!(
                scheduler::eligible(current)
                    && current.file == source.file
                    && current.url == source.url
                    && current.option == source.option,
                "scheduled profile changed while waiting for admission"
            );
        }
        let profile = tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("remote refresh cancelled during shutdown"),
            result = self.download_remote(url, None, options) => result?,
        };
        let (reply, result) = oneshot::channel();
        tokio::select! {
            biased;
            _ = closing(&mut shutdown) => bail!("remote refresh cancelled during shutdown"),
            sent = self.commands.send(CommandMessage::RefreshRemote {
                automatic, source: Box::new(source), profile: Box::new(profile), reply,
            }) => sent.context("core manager stopped")?,
        }
        result.await.context("remote refresh cancelled during shutdown")?
    }

    async fn core_download_routes(&self) -> Vec<crate::core_release::Route> {
        let mut routes = Vec::new();
        if self.status().phase == CorePhase::Running
            && let Ok(route) = self.managed_download_route().await
        {
            routes.push(route);
        }
        routes.extend([crate::core_release::Route::System, crate::core_release::Route::Direct]);
        routes
    }

    async fn discover_core_release(&self, version: Option<&str>) -> Result<crate::core_release::ResolvedRelease> {
        let resolved = crate::core_release::discover_via(version, self.core_download_routes().await).await?;
        self.logs.append(
            "core-upgrade",
            format!("release metadata resolved via {} route", resolved.route.name()),
        );
        Ok(resolved)
    }

    /// Resolve only from the current child's reported ports and committed authentication.
    async fn managed_download_route(&self) -> Result<crate::core_release::Route> {
        let mut state = self.state.clone();
        let snapshot = state.borrow_and_update().clone();
        ensure!(
            snapshot.phase == CorePhase::Running,
            "self_proxy requires a running managed core"
        );
        let route = timeout(Duration::from_secs(3), async {
            let runtime = self.runtime_config().await?;
            let core = self.client.get_base_config().await?;
            ensure!(
                same_proxy_snapshot(&snapshot, &self.status()),
                "managed proxy changed during route resolution; retry"
            );
            crate::proxy_access::verify_ports(&runtime, &core)?;
            crate::remote::ManagedProxy::from_core(&core, &runtime)
        })
        .await
        .context("managed proxy query timed out")??;
        Ok(crate::core_release::Route::Managed {
            proxy: route,
            snapshot: Box::new(snapshot),
            state,
        })
    }

    /// Resolve after admission, then cancel if the child or committed configuration changes.
    async fn download_remote(
        &self,
        url: &str,
        name: Option<&str>,
        options: crate::remote::RemoteOptions,
    ) -> Result<headless_core::config::remote::RemoteProfile> {
        if options.self_proxy != Some(true) {
            return crate::remote::download(url, name, options).await;
        }
        let route = self.managed_download_route().await?;
        let crate::core_release::Route::Managed { proxy, .. } = &route else {
            unreachable!()
        };
        route
            .run(crate::remote::download_via(url, name, options, Some(proxy.clone())))
            .await
    }

    pub async fn runtime_config(&self) -> Result<Mapping> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::ReadConfig(reply))
            .await
            .context("core manager stopped")?;
        result.await.context("configuration read cancelled during shutdown")?
    }

    pub async fn settings(&self) -> Result<ServiceSettings> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::ReadSettings(reply))
            .await
            .context("core manager stopped")?;
        result.await.context("settings read cancelled during shutdown")?
    }

    pub async fn profile_dns(&self, uid: String) -> Result<DnsOverrideState> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::ReadProfileDns { uid, reply })
            .await
            .context("core manager stopped")?;
        result.await.context("DNS settings read cancelled during shutdown")?
    }

    pub async fn set_profile_dns(
        &self,
        uid: String,
        enabled: bool,
        confirmation: Option<String>,
    ) -> Result<DnsOverrideOutcome> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::SetProfileDns {
                uid,
                enabled,
                confirmation,
                reply,
            })
            .await
            .context("core manager stopped")?;
        result.await.context("DNS settings update cancelled during shutdown")?
    }

    pub async fn set_settings(&self, runtime: RuntimeSettings) -> Result<ServiceSettings> {
        runtime.validate()?;
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::SetSettings { runtime, reply })
            .await
            .context("core manager stopped")?;
        result.await.context("settings update cancelled during shutdown")?
    }

    pub async fn select_node(&self, group: String, node: String) -> Result<CoreStatus> {
        self.request(Operation::SelectNode { group, node }).await
    }

    pub async fn unfix_node(&self, group: String) -> Result<CoreStatus> {
        self.request(Operation::UnfixNode(group)).await
    }

    async fn request(&self, operation: Operation) -> Result<CoreStatus> {
        ensure!(!*self.shutdown.borrow(), "service is shutting down");
        let (reply, result) = oneshot::channel();
        self.commands
            .send(CommandMessage::Control(Request { operation, reply }))
            .await
            .context("core manager stopped")?;
        result.await.context("core operation cancelled during shutdown")?
    }

    pub async fn start(&self) -> Result<CoreStatus> {
        self.request(Operation::Start).await
    }
    pub async fn stop(&self) -> Result<CoreStatus> {
        self.request(Operation::Stop).await
    }
    pub async fn restart(&self) -> Result<CoreStatus> {
        self.request(Operation::Restart).await
    }
    pub async fn reload_config(&self, path: PathBuf) -> Result<CoreStatus> {
        self.request(Operation::Reload(path)).await
    }

    pub async fn apply_config(&self, config: Mapping) -> Result<CoreStatus> {
        self.request(Operation::Apply(Box::new(config))).await
    }

    /// Replace edited runtime YAML while preserving the active profile association.
    pub async fn edit_config(&self, config: Mapping) -> Result<CoreStatus> {
        self.request(Operation::Edit(Box::new(config))).await
    }

    pub async fn import_config(&self, path: PathBuf) -> Result<CoreStatus> {
        self.request(Operation::Import(path)).await
    }

    /// Merge settings into the committed config using the extracted upstream helper.
    pub async fn apply_overlay(&self, overlay: Mapping) -> Result<CoreStatus> {
        self.request(Operation::Merge(Box::new(overlay))).await
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown.send_replace(true);
        let mut scheduler = self.scheduler_completion.clone();
        while !*scheduler.borrow_and_update() {
            scheduler
                .changed()
                .await
                .context("subscription scheduler ended without completion")?;
        }
        let mut done = self.completion.clone();
        loop {
            if let Some(result) = done.borrow().clone() {
                return result.map_err(anyhow::Error::msg);
            }
            done.changed()
                .await
                .context("core supervisor ended without a shutdown result")?;
        }
    }
}

struct ManagedProcess {
    child: Child,
    readers: Vec<JoinHandle<()>>,
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        for reader in &self.readers {
            reader.abort();
        }
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

struct Restoration {
    uid: String,
    selected: Vec<PrfSelected>,
    previous: Option<Proxies>,
    completed: HashMap<smartstring::alias::String, smartstring::alias::String>,
    next_at: Instant,
    deadline: Instant,
    prune: bool,
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
    fn cancel_restoration(&mut self) {
        self.restoration = None;
        self.status.send_modify(|state| {
            state.selection_pending.clear();
            state.selection_error = None;
        });
    }

    async fn selection_call<T>(&mut self, deadline: Instant, task: impl Future<Output = Result<T>>) -> Result<T> {
        let deadline = deadline.min(Instant::now() + self.options.policy.selection_timeout);
        tokio::select! {
            biased;
            _ = closing(&mut self.shutdown) => bail!("node operation cancelled during shutdown"),
            result = tokio::time::timeout_at(deadline, task) => result.context("node operation timeout")?,
        }
    }

    async fn proxy_snapshot(&mut self, deadline: Instant) -> Result<Proxies> {
        let client = self.client.clone();
        self.selection_call(deadline, async move { Ok(client.get_proxies().await?) })
            .await
    }

    async fn set_runtime_node(&mut self, group: &str, node: Option<&str>, deadline: Instant) -> Result<()> {
        let client = self.client.clone();
        self.selection_call(deadline, async move {
            match node {
                Some(node) => client.select_node_for_group(group, node).await?,
                None => client.unfixed_proxy(group).await?,
            }
            Ok(())
        })
        .await
    }

    async fn change_node(&mut self, group: &str, node: Option<&str>) -> Result<()> {
        let result = self.change_node_inner(group, node).await;
        if let Err(error) = &result {
            self.status.send_modify(|state| {
                state.error = Some(format!("{error:#}"));
                state.selection_error = Some(format!("{error:#}"));
            });
        }
        result
    }

    async fn change_node_inner(&mut self, group: &str, node: Option<&str>) -> Result<()> {
        ensure!(self.status.borrow().phase == CorePhase::Running, "core is not running");
        let uid = self
            .store
            .state()
            .active_profile
            .context("select an active profile before choosing a node")?;
        let recorded = self.profile_store.selections(&uid)?;
        let snapshot = self
            .proxy_snapshot(Instant::now() + self.options.policy.selection_timeout)
            .await?;
        let proxy = snapshot.proxies.get(group).context("proxy group does not exist")?;
        let automatic = matches!(
            proxy.proxy_type,
            ProxyType::URLTest | ProxyType::Fallback | ProxyType::LoadBalance
        );
        ensure!(
            automatic || proxy.proxy_type == ProxyType::Selector,
            "proxy is not a selectable group"
        );
        if let Some(node) = node {
            ensure!(
                proxy
                    .all
                    .as_ref()
                    .is_some_and(|all| all.iter().any(|member| member == node)),
                "node is not a member of this group"
            );
        } else {
            ensure!(automatic, "only automatic groups support removing a fixed node");
        }
        let previous = if automatic {
            proxy.fixed.clone()
        } else {
            Some(proxy.now.clone().context("previous group selection is unavailable")?)
        };
        self.cancel_restoration();
        let result = async {
            self.set_runtime_node(group, node, Instant::now() + self.options.policy.selection_timeout)
                .await?;
            let snapshot = self
                .proxy_snapshot(Instant::now() + self.options.policy.selection_timeout)
                .await?;
            let actual = snapshot
                .proxies
                .get(group)
                .context("group disappeared after node operation")?;
            match node {
                Some(node) => ensure!(
                    actual.now.as_deref() == Some(node) || (automatic && actual.fixed.as_deref() == Some(node)),
                    "core did not confirm requested node"
                ),
                None => ensure!(
                    actual.fixed.as_deref().is_none_or(str::is_empty),
                    "core did not remove the fixed node"
                ),
            }
            ensure!(!*self.shutdown.borrow(), "node persistence cancelled during shutdown");
            match node {
                Some(node) => self.profile_store.record_selection(&uid, group, node)?,
                None => self.profile_store.forget_selection(&uid, group)?,
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = result {
            let disk = self.profile_store.save_selections(&uid, recorded);
            let runtime = if *self.shutdown.borrow() {
                Ok(())
            } else {
                self.set_runtime_node(
                    group,
                    previous.as_deref(),
                    Instant::now() + self.options.policy.selection_timeout,
                )
                .await
            };
            self.profile_state.send_replace(self.profile_store.snapshot());
            let mut message = format!("node operation failed: {error:#}");
            if let Err(error) = disk {
                message.push_str(&format!("; selection record rollback failed: {error:#}"));
            }
            if let Err(error) = runtime {
                message.push_str(&format!("; runtime node rollback failed: {error:#}"));
            }
            bail!(message);
        }
        self.profile_state.send_replace(self.profile_store.snapshot());
        self.status.send_modify(|state| {
            state.error = None;
            state.selection_error = None;
        });
        Ok(())
    }

    async fn begin_restoration(&mut self, prune: bool) {
        self.cancel_restoration();
        if *self.shutdown.borrow() || self.status.borrow().phase != CorePhase::Running {
            return;
        }
        let Some(uid) = self.store.state().active_profile else {
            return;
        };
        let selected = match self.profile_store.selections(&uid) {
            Ok(selected) => selected,
            Err(error) => {
                self.status.send_modify(|state| {
                    state.selection_error = Some(format!("cannot read saved selections: {error:#}"))
                });
                return;
            }
        };
        if selected.is_empty() {
            return;
        }
        self.status.send_modify(|state| {
            state.selection_pending = selected
                .iter()
                .filter_map(|record| record.name.as_ref().map(ToString::to_string))
                .collect();
        });
        let now = Instant::now();
        self.restoration = Some(Restoration {
            uid,
            selected,
            previous: None,
            completed: HashMap::new(),
            next_at: now,
            deadline: now + self.options.policy.selection_settle,
            prune,
        });
        self.restore_step(now + self.options.policy.selection_first_pass).await;
    }

    async fn restore_step(&mut self, budget: Instant) {
        let Some(mut restore) = self.restoration.take() else {
            return;
        };
        if self.status.borrow().phase != CorePhase::Running
            || self.store.state().active_profile.as_deref() != Some(&restore.uid)
            || *self.shutdown.borrow()
        {
            return;
        }
        let deadline = budget.min(restore.deadline);
        let result = async {
            let mut snapshot = self.proxy_snapshot(deadline).await?;
            // Retry a previously accepted selection if the core subsequently moved away.
            restore.completed.retain(|group, node| {
                snapshot
                    .proxies
                    .get(group.as_str())
                    .is_some_and(|proxy| proxy.now.as_deref() == Some(node.as_str()))
            });
            let plan =
                crate::selections::reconcile_selected_nodes(&restore.selected, restore.previous.as_ref(), &snapshot);
            let confirm = crate::selections::selected_nodes_need_confirmation(&restore.selected, &snapshot);
            let mut attempted = false;
            for (group, node) in crate::selections::remaining_activations(&plan.activations, &restore.completed) {
                self.set_runtime_node(&group, Some(&node), deadline).await?;
                restore.completed.insert(group, node);
                attempted = true;
            }
            if attempted {
                snapshot = self.proxy_snapshot(deadline).await?;
            }
            if restore.prune && (!confirm || restore.previous.is_some()) && plan.repaired_count > 0 {
                self.profile_store
                    .save_selections(&restore.uid, plan.selected.clone())?;
                self.profile_state.send_replace(self.profile_store.snapshot());
                restore.selected = plan.selected;
            }
            let pending = crate::selections::unsettled_selections(&restore.selected, &snapshot)
                .into_iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>();
            restore.previous = Some(snapshot);
            Ok::<_, anyhow::Error>(pending)
        }
        .await;
        if *self.shutdown.borrow() {
            return;
        }
        let (pending, error) = match result {
            Ok(pending) => (pending, None),
            Err(error) => (
                restore
                    .selected
                    .iter()
                    .filter_map(|record| record.name.as_ref().map(ToString::to_string))
                    .collect(),
                Some(format!("node restoration failed: {error:#}")),
            ),
        };
        if pending.is_empty() && error.is_none() {
            self.status.send_modify(|state| {
                state.selection_pending.clear();
                state.selection_error = None;
            });
            return;
        }
        let expired = Instant::now() >= restore.deadline;
        let error = if expired {
            Some(format!(
                "node restoration deadline reached for [{}]{}",
                pending.join(", "),
                error.map(|error| format!(": {error}")).unwrap_or_default()
            ))
        } else {
            error
        };
        self.status.send_modify(|state| {
            state.selection_pending = if expired { Vec::new() } else { pending };
            state.selection_error = error.clone();
        });
        if expired {
            self.logs.append("manager", error.unwrap_or_default());
        } else {
            restore.next_at = (Instant::now() + self.options.policy.selection_interval).min(restore.deadline);
            self.restoration = Some(restore);
        }
    }

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
                    if let Err(error) = self.recover_core_upgrade().await.and_then(|()| self.profile_store.recover_import())
                        .and_then(|()| self.profile_store.recover_refresh(self.store.state().current.as_ref()))
                        .and_then(|()| self.profile_store.recover_enhancement(self.store.state().current.as_ref()))
                        .and_then(|()| self.settings_store.recover(self.store.state().current.as_ref()))
                        .and_then(|()| self.profile_store.recover_delete_with_settings(&mut self.settings_store)) {
                        let message = format!("configuration recovery failed: {error:#}");
                        self.status.send_modify(|state| state.error = Some(message.clone()));
                        match request {
                            CommandMessage::InstalledCoreVersion(reply) => {let _ = reply.send(Err(anyhow::anyhow!(message)));}
                            CommandMessage::CheckCoreUpgrade {reply, ..} => {let _ = reply.send(Err(anyhow::anyhow!(message)));}
                            CommandMessage::UpgradePreparedCore {reply, ..} => {let _ = reply.send(Err(anyhow::anyhow!(message)));}
                            CommandMessage::ActivateCoreUpgrade {reply, ..} => {let _ = reply.send(Err(anyhow::anyhow!(message)));}
                            CommandMessage::CoreInstallation(reply) => {let _ = reply.send(Err(anyhow::anyhow!(message)));}
                            CommandMessage::StageCoreUpgrade { reply, .. } => { let _ = reply.send(Err(anyhow::anyhow!(message))); }
                            CommandMessage::ReadProfileRaw { reply, .. } | CommandMessage::SetProfileRaw { reply, .. } => { let _ = reply.send(Err(anyhow::anyhow!(message))); }
                            CommandMessage::ReadProfileDns { reply, .. } => { let _ = reply.send(Err(anyhow::anyhow!(message))); }
                            CommandMessage::SetProfileDns { reply, .. } => { let _ = reply.send(Err(anyhow::anyhow!(message))); }
                            CommandMessage::RefreshRemote { reply, .. }
                            | CommandMessage::SetEnhancement { reply, .. }
                            | CommandMessage::EditProfile { reply, .. }
                            | CommandMessage::ImportRemote { reply, .. }
                            | CommandMessage::ImportProfileYaml { reply, .. }
                            | CommandMessage::ImportProfile { reply, .. } => { let _ = reply.send(Err(anyhow::anyhow!(message))); }
                            CommandMessage::ReadEnhancement { reply, .. } => { let _ = reply.send(Err(anyhow::anyhow!(message))); }
                            CommandMessage::DeleteProfile { reply, .. } => { let _ = reply.send(Err(anyhow::anyhow!(message))); }
                            CommandMessage::ReadConfig(reply) => { let _ = reply.send(Err(anyhow::anyhow!(message))); }
                            CommandMessage::ReadSettings(reply) | CommandMessage::SetSettings { reply, .. } => { let _ = reply.send(Err(anyhow::anyhow!(message))); }
                            CommandMessage::Control(request) => { let _ = request.reply.send(Err(anyhow::anyhow!(message))); }
                        }
                        continue;
                    }
                    self.settings = self.settings_store.snapshot();
                    self.dns_confirmations.retain(|uid, _| self.profile_store.get_item(uid).is_ok());
                    match request {
                        CommandMessage::InstalledCoreVersion(reply) => {
                            if !reply.is_closed() {let result = self.installed_version().await;let _ = reply.send(result);}
                        }
                        CommandMessage::CheckCoreUpgrade {version,force,reply} => {
                            if !reply.is_closed() {let result = self.check_upgrade(&version,force).await;let _ = reply.send(result);}
                        }
                        CommandMessage::UpgradePreparedCore {prepared,force,downloads,reply,_permit:permit} => {
                            if !reply.is_closed() {
                                let result = self.upgrade_prepared(prepared,force,downloads).await;
                                drop(permit);let _ = reply.send(result);
                            }
                        }
                        CommandMessage::ActivateCoreUpgrade {id, downloads, reply, _permit: permit} => {
                            if !reply.is_closed() {let result = self.activate_core(&id, downloads).await;drop(permit);let _ = reply.send(result);}
                        }
                        CommandMessage::CoreInstallation(reply) => {
                            let result = crate::core_upgrade::installation(self.options.binary.parent().expect("managed core directory"));
                            let _ = reply.send(result);
                        }
                        CommandMessage::StageCoreUpgrade {id, downloads, reply, _permit: permit} => {
                            if !reply.is_closed() {
                                let result = async {
                                    let yaml = serde_yaml_ng::to_string(&read_config(&self.options.config).await?)?;
                                    let revision = self.store.state().current.map(|revision| revision.file);
                                    downloads.stage(&id, yaml, revision, &self.options.data_dir, &mut self.shutdown).await
                                }.await;
                                drop(permit);
                                let _ = reply.send(result);
                            }
                        }
                        CommandMessage::ReadProfileRaw { uid, reply } => { let _ = reply.send(self.profile_store.read_raw(&uid)); }
                        CommandMessage::SetProfileRaw { uid, revision, yaml, reply } => {
                            if !reply.is_closed() {
                                let result = self.update_profile_raw(&uid, revision, yaml).await;
                                if let Err(error) = &result { self.status.send_modify(|state| state.error = Some(format!("{error:#}"))); }
                                self.profile_state.send_replace(self.profile_store.snapshot());
                                let _ = reply.send(result);
                            }
                        }
                        CommandMessage::ReadProfileDns { uid, reply } => {
                            let result = self.profile_store.dns_source(&uid).map(|source| self.dns_decision(&uid, source));
                            let _ = reply.send(result);
                        }
                        CommandMessage::SetProfileDns { uid, enabled, confirmation, reply } => {
                            let result = self.update_profile_dns(&uid, enabled, confirmation).await;
                            let _ = reply.send(result);
                        }
                        CommandMessage::ReadSettings(reply) => { let _ = reply.send(Ok(self.settings.clone())); }
                        CommandMessage::SetSettings { runtime, reply } => {
                            if !reply.is_closed() {
                                let result = self.update_settings(runtime).await;
                                if let Err(error) = &result { self.status.send_modify(|state| state.error = Some(format!("{error:#}"))); }
                                let _ = reply.send(result);
                            }
                        }
                        CommandMessage::ReadEnhancement { uid, kind, reply } => {
                            let result = match kind {
                                ProfileEnhancement::Global(kind) => self.profile_store.read_global(kind),
                                ProfileEnhancement::Sequence(kind) => self.profile_store.read_sequence(&uid, kind),
                                ProfileEnhancement::Merge => self.profile_store.read_merge(&uid),
                                ProfileEnhancement::Script => self.profile_store.read_script(&uid).map(|content| EnhancementContent { uid: content.uid, yaml: content.source }),
                            };
                            let _ = reply.send(result);
                        }
                        CommandMessage::SetEnhancement { uid, kind, yaml, reply } => {
                            if !reply.is_closed() {
                                let result = self.set_enhancement(&uid, kind, yaml).await;
                                if let Err(error) = &result {
                                    self.status.send_modify(|state| state.error = Some(format!("{error:#}")));
                                }
                                self.profile_state.send_replace(self.profile_store.snapshot());
                                let _ = reply.send(result);
                            }
                        }
                        CommandMessage::EditProfile { uid, patch, reply } => {
                            if !reply.is_closed() {
                                let result = self.profile_store.edit_profile(&uid, patch);
                                self.profile_state.send_replace(self.profile_store.snapshot());
                                let _ = reply.send(result);
                            }
                        }
                        CommandMessage::DeleteProfile { uid, reply } => {
                            if !reply.is_closed() {
                                let result = self.profile_store.delete_profile_with_settings(&uid, self.store.state().active_profile.as_deref(), &mut self.settings_store);
                                self.settings = self.settings_store.snapshot();
                                if self.profile_store.get_item(&uid).is_err() { self.dns_confirmations.remove(&uid); }
                                self.profile_state.send_replace(self.profile_store.snapshot());
                                let _ = reply.send(result.map(|()| self.profile_store.snapshot()));
                            }
                        }
                        CommandMessage::RefreshRemote { automatic, source, profile, reply } => {
                            if !reply.is_closed() {
                                let result = if automatic && !self.profile_store.get_item(source.uid.as_deref().unwrap_or_default()).is_ok_and(scheduler::eligible) {
                                    Err(anyhow::anyhow!("automatic subscription update disabled"))
                                } else { self.refresh_remote(*source, *profile).await };
                                if let Err(error) = &result {
                                    self.status.send_modify(|state| state.error = Some(format!("{error:#}")));
                                }
                                self.profile_state.send_replace(self.profile_store.snapshot());
                                let _ = reply.send(result);
                            }
                        }
                        CommandMessage::ImportRemote { profile, reply } => {
                            if !reply.is_closed() {
                                let result = self.profile_store.import_remote_with_defaults(*profile);
                                self.profile_state.send_replace(self.profile_store.snapshot());
                                let _ = reply.send(result);
                            }
                        }
                        CommandMessage::ReadConfig(reply) => {
                            let _ = reply.send(self.store.read_current());
                        }
                        CommandMessage::ImportProfileYaml { yaml, name, reply } => {
                            let result = self.profile_store.import_local_with_defaults(&name, &yaml);
                            if let Err(error) = &result {
                                self.status.send_modify(|state| state.error = Some(format!("{error:#}")));
                            }
                            self.profile_state.send_replace(self.profile_store.snapshot());
                            let _ = reply.send(result);
                        }
                        CommandMessage::Control(request) => {
                            let result = self.execute(request.operation).await;
                            let _ = request.reply.send(result.map(|()| self.status.borrow().clone()));
                        }
                        CommandMessage::ImportProfile { path, name, reply } => {
                            let result = self.import_profile(&path, name).await;
                            if let Err(error) = &result {
                                self.status.send_modify(|state| state.error = Some(format!("{error:#}")));
                            }
                            self.profile_state.send_replace(self.profile_store.snapshot());
                            let _ = reply.send(result);
                        }
                    }
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

    async fn execute(&mut self, operation: Operation) -> Result<()> {
        let edit = matches!(operation, Operation::Edit(_));
        self.observe_exit().await?;
        let merge = matches!(&operation, Operation::Merge(_));
        let reload = matches!(&operation, Operation::Reload(_));
        match operation {
            Operation::Start => {
                self.retry_at = None;
                if self.process.is_some() && self.status.borrow().phase == CorePhase::Running {
                    return Ok(());
                }
                self.status.send_modify(|state| state.recovery_attempt = 0);
                self.start_core().await
            }
            Operation::Stop | Operation::Restart => {
                self.retry_at = None;
                let restart = matches!(operation, Operation::Restart);
                self.status.send_modify(|state| state.recovery_attempt = 0);
                self.publish(CorePhase::Stopping, None);
                if let Err(error) = self.stop_process().await {
                    self.publish(CorePhase::Failed, Some(format!("{error:#}")));
                    return Err(error);
                }
                self.publish(CorePhase::Stopped, None);
                if restart { self.start_core().await } else { Ok(()) }
            }
            Operation::Reload(path) | Operation::Import(path) => {
                if reload {
                    ensure!(self.status.borrow().phase == CorePhase::Running, "core is not running");
                }
                let result = async {
                    let config = read_config(&path).await?;
                    self.apply(config, None).await
                }
                .await;
                if let Err(error) = &result {
                    self.status
                        .send_modify(|state| state.error = Some(format!("{error:#}")));
                }
                result
            }
            Operation::Apply(config) | Operation::Merge(config) | Operation::Edit(config) => {
                let result = async {
                    let config = if merge {
                        runtime::generate(self.store.read_current()?, &config)?
                    } else {
                        *config
                    };
                    let active_profile = if merge || edit {
                        self.store.state().active_profile
                    } else {
                        None
                    };
                    self.apply(config, active_profile).await
                }
                .await;
                if let Err(error) = &result {
                    self.status
                        .send_modify(|state| state.error = Some(format!("{error:#}")));
                }
                result
            }
            Operation::SelectProfile(uid) => {
                let result = async {
                    let generation = self.profile_store.read_generation(&uid)?;
                    let config = self.finish_generation(generation).await?;
                    self.apply(config, Some(uid)).await
                }
                .await;
                if let Err(error) = &result {
                    self.status
                        .send_modify(|state| state.error = Some(format!("{error:#}")));
                }
                result
            }
            Operation::SelectNode { group, node } => self.change_node(&group, Some(&node)).await,
            Operation::UnfixNode(group) => self.change_node(&group, None).await,
        }
    }

    async fn import_profile(&mut self, path: &Path, name: Option<String>) -> Result<PrfItem> {
        ensure!(!*self.shutdown.borrow(), "profile import cancelled during shutdown");
        ensure!(
            tokio::fs::metadata(path).await?.len() <= runtime::MAX_CONFIG_BYTES as u64,
            "profile exceeds 8 MiB"
        );
        let yaml = tokio::fs::read_to_string(path).await?;
        ensure!(!*self.shutdown.borrow(), "profile import cancelled during shutdown");
        let name = name.unwrap_or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Local File")
                .to_owned()
        });
        self.profile_store.import_local_with_defaults(&name, &yaml)
    }

    async fn set_enhancement(&mut self, uid: &str, kind: ProfileEnhancement, yaml: Option<String>) -> Result<PrfItem> {
        self.observe_exit().await?;
        if let ProfileEnhancement::Global(kind) = kind {
            return self.set_global_enhancement(kind, yaml).await;
        }
        if yaml.is_none() {
            let content = match kind {
                ProfileEnhancement::Global(_) => unreachable!(),
                ProfileEnhancement::Sequence(kind) => self.profile_store.read_sequence(uid, kind)?,
                ProfileEnhancement::Merge => self.profile_store.read_merge(uid)?,
                ProfileEnhancement::Script => {
                    let content = self.profile_store.read_script(uid)?;
                    EnhancementContent {
                        uid: content.uid,
                        yaml: content.source,
                    }
                }
            };
            if content.uid.is_none() {
                return Ok(self.profile_store.get_item(uid)?.clone());
            }
        }
        let mut plan = match kind {
            ProfileEnhancement::Global(_) => unreachable!(),
            ProfileEnhancement::Sequence(kind) => self.profile_store.prepare_sequence(uid, kind, yaml)?,
            ProfileEnhancement::Merge => self.profile_store.prepare_merge(uid, yaml)?,
            ProfileEnhancement::Script => self.profile_store.prepare_script(uid, yaml)?,
        };
        let config = self
            .finish_generation(plan.generation.take().context("linked generation missing")?)
            .await?;
        if self.store.state().active_profile.as_deref() == Some(uid) {
            self.apply_with_change(
                config,
                Some(uid.into()),
                Some(ProfileChange::Enhancement(Box::new(plan))),
            )
            .await?;
        } else {
            let result = (|| {
                self.profile_store.begin_enhancement(plan, None)?;
                self.profile_store.publish_enhancement()
            })();
            let recovery = self
                .profile_store
                .recover_enhancement(self.store.state().current.as_ref());
            result.map_err(|error| error.context(format!("enhancement recovery: {recovery:?}")))?;
            recovery?;
        }
        Ok(self.profile_store.get_item(uid)?.clone())
    }

    async fn set_global_enhancement(&mut self, kind: GlobalEnhancementKind, source: Option<String>) -> Result<PrfItem> {
        let source = source.unwrap_or_else(|| kind.default_source().into());
        let active = self.store.state().active_profile;
        let mut plan = self
            .profile_store
            .prepare_global(kind, source.clone(), active.as_deref())?;
        if let Some(generation) = plan.generation.take() {
            let config = self.finish_generation(generation).await?;
            self.apply_with_change(config, active, Some(ProfileChange::Enhancement(Box::new(plan))))
                .await?;
        } else {
            if kind == GlobalEnhancementKind::Script && source != DEFAULT_GLOBAL_SCRIPT {
                self.check_script(source).await?;
            }
            ensure!(!*self.shutdown.borrow(), "global edit cancelled during shutdown");
            let result = (|| {
                self.profile_store.begin_enhancement(plan, None)?;
                self.profile_store.publish_enhancement()
            })();
            let recovery = self
                .profile_store
                .recover_enhancement(self.store.state().current.as_ref());
            result.map_err(|error| error.context(format!("global enhancement recovery: {recovery:?}")))?;
            recovery?;
        }
        Ok(self.profile_store.get_item(kind.uid())?.clone())
    }

    async fn check_script(&mut self, source: String) -> Result<()> {
        let binary = self
            .options
            .script_worker
            .clone()
            .map_or_else(std::env::current_exe, Ok)?;
        let response = crate::script::execute(
            &binary,
            headless_core::enhance::script::ScriptRequest {
                source,
                config: Mapping::new(),
                name: String::new(),
                check_only: true,
            },
            &mut self.shutdown,
            self.options.policy.script_timeout,
        )
        .await?;
        if let Some(error) = response.error {
            bail!("{error}");
        }
        ensure!(response.config.is_some(), "script syntax check returned no result");
        Ok(())
    }

    async fn finish_generation(&mut self, generation: GenerationPlan) -> Result<ConfigCandidate> {
        let dns = self.dns_decision(&generation.profile_uid, generation.dns_source);
        let runtime = self.dns_runtime(&dns);
        let config = runtime.prepare(generation.config)?;
        let config = runtime::generate(config, &generation.global_merge)?;
        let config = self
            .finish_script(config, generation.global_script, generation.name.clone())
            .await?;
        let config = runtime::generate(config, &generation.profile_merge)?;
        let config = self.finish_script(config, generation.script, generation.name).await?;
        Ok(ConfigCandidate::Enhanced {
            config: self.enforce_runtime_settings(config, &runtime)?,
            runtime: Box::new(runtime),
            dns: Box::new(dns),
        })
    }

    async fn update_settings(&mut self, runtime: RuntimeSettings) -> Result<ServiceSettings> {
        let candidate = ServiceSettings {
            schema_version: 1,
            runtime,
            profile_dns: self.settings.profile_dns.clone(),
        };
        candidate.validate()?;
        if candidate == self.settings {
            return Ok(self.settings.clone());
        }
        self.observe_exit().await?;
        let result = async {
            if self.store.state().current.is_none() {
                ensure!(!*self.shutdown.borrow(), "settings update cancelled during shutdown");
                self.settings_store.replace(candidate.clone())?;
                return Ok(());
            }
            self.settings = candidate.clone();
            let active = self.store.state().active_profile;
            let config = if let Some(uid) = &active {
                let plan = self.profile_store.read_generation(uid)?;
                self.finish_generation(plan).await?
            } else {
                self.store.read_current()?.into()
            };
            self.apply_with_change(config, active, Some(ProfileChange::Settings(Box::new(candidate))))
                .await
        }
        .await;
        self.settings = self.settings_store.snapshot();
        result?;
        Ok(self.settings.clone())
    }

    fn dns_decision(&self, uid: &str, source: Option<String>) -> DnsOverrideState {
        let requested = self
            .settings
            .profile_dns
            .get(uid)
            .map_or(self.settings.runtime.dns.is_some(), |settings| settings.enabled);
        DnsOverrideState::new(
            uid,
            source,
            requested,
            self.dns_confirmations.get(uid).map(String::as_str),
        )
    }

    fn dns_runtime(&self, state: &DnsOverrideState) -> RuntimeSettings {
        let mut runtime = self.settings.runtime.clone();
        if !state.enabled {
            runtime.dns = None;
        }
        runtime
    }

    async fn update_profile_dns(
        &mut self,
        uid: &str,
        enabled: bool,
        confirmation: Option<String>,
    ) -> Result<DnsOverrideOutcome> {
        ensure!(
            self.store.state().active_profile.as_deref() == Some(uid),
            "active profile changed; retry DNS settings"
        );
        let source = self.profile_store.dns_source(uid)?;
        if enabled && let Some(source) = source.as_ref().filter(|s| Some(s.as_str()) != confirmation.as_deref()) {
            return Ok(DnsOverrideOutcome::ConfirmationRequired { source: source.clone() });
        }
        ensure!(
            !enabled || self.settings.runtime.dns.is_some(),
            "save DNS settings before enabling the profile override"
        );
        self.observe_exit().await?;
        let old_revision = self.store.state().current;
        let old_confirmation = self.dns_confirmations.remove(uid);
        if enabled && let Some(source) = source {
            self.dns_confirmations.insert(uid.into(), source);
        }
        self.settings
            .profile_dns
            .insert(uid.into(), ProfileDnsSettings { enabled });
        let result = async {
            let generation = self.profile_store.read_generation(uid)?;
            let config = self.finish_generation(generation).await?;
            self.apply_with_change(
                config,
                Some(uid.into()),
                Some(ProfileChange::Settings(Box::new(self.settings.clone()))),
            )
            .await
        }
        .await;
        self.settings = self.settings_store.snapshot();
        if self.store.state().current == old_revision {
            self.dns_confirmations.remove(uid);
            if let Some(source) = old_confirmation {
                self.dns_confirmations.insert(uid.into(), source);
            }
        }
        result?;
        Ok(DnsOverrideOutcome::Applied {
            state: self.dns_decision(uid, self.profile_store.dns_source(uid)?),
        })
    }

    fn enforce_runtime_settings(&self, config: Mapping, runtime: &RuntimeSettings) -> Result<Mapping> {
        let enforced = runtime.enforce(config.clone())?;
        for field in runtime.overridden_fields(&config, &enforced)? {
            self.logs.append(
                "settings",
                format!("{field} is managed by settings; override discarded"),
            );
        }
        Ok(enforced)
    }

    async fn finish_script(&mut self, config: Mapping, source: Option<String>, name: String) -> Result<Mapping> {
        let Some(source) = source else {
            return Ok(config);
        };
        // The exact upstream identity template has no side effects or changes.
        // Avoid spawning workers for untouched defaults, including on non-Linux.
        if source == DEFAULT_GLOBAL_SCRIPT {
            return runtime::generate(config, &Mapping::new());
        }
        let binary = match &self.options.script_worker {
            Some(binary) => binary.clone(),
            None => std::env::current_exe()?,
        };
        let response = crate::script::execute(
            &binary,
            headless_core::enhance::script::ScriptRequest {
                source,
                config,
                name,
                check_only: false,
            },
            &mut self.shutdown,
            self.options.policy.script_timeout,
        )
        .await?;
        for (level, text) in response.logs {
            self.logs.append("script", format!("[{level}] {text}"));
        }
        if let Some(error) = response.error {
            bail!("{error}");
        }
        runtime::generate(
            response.config.context("script returned no configuration")?,
            &Mapping::new(),
        )
    }

    async fn update_profile_raw(&mut self, uid: &str, revision: String, yaml: String) -> Result<RawContent> {
        self.observe_exit().await?;
        let previous = self.profile_store.read_raw(uid)?;
        ensure!(
            previous.revision == revision,
            "profile raw revision changed; reload before saving"
        );
        let raw = runtime::parse_profile(&yaml)?;
        // Upstream first validates original YAML even for an inactive profile.
        // An immutable validation revision changes no runtime manifest or catalog.
        let validation = self.store.stage(raw.clone())?;
        crate::validation::validate(
            &self.options.binary,
            &self.options.data_dir,
            &self.store.path(&validation)?,
            &mut self.shutdown,
            self.options.policy.validation_timeout,
        )
        .await?;
        if self.store.state().active_profile.as_deref() == Some(uid) {
            let generation = self.profile_store.generation(uid, raw)?;
            let config = self.finish_generation(generation).await?;
            self.apply_with_change(
                config,
                Some(uid.into()),
                Some(ProfileChange::RawEdit { revision, yaml }),
            )
            .await?;
        } else {
            let result = (|| {
                self.profile_store.begin_raw_edit(uid, &revision, &yaml, None)?;
                self.profile_store.publish_refresh()
            })();
            let recovery = self.profile_store.recover_refresh(self.store.state().current.as_ref());
            result.map_err(|error| error.context(format!("raw edit recovery: {recovery:?}")))?;
            recovery?;
        }
        self.profile_store.read_raw(uid)
    }

    async fn refresh_remote(
        &mut self,
        source: PrfItem,
        remote: headless_core::config::remote::RemoteProfile,
    ) -> Result<PrfItem> {
        let uid = source.uid.as_deref().context("remote UID missing")?.to_string();
        let current = self.profile_store.get_item(&uid)?;
        ensure!(
            current.itype == source.itype
                && current.file == source.file
                && current.url == source.url
                && current.option == source.option,
            "remote profile changed during download; retry refresh"
        );
        self.observe_exit().await?;
        if self.store.state().active_profile.as_deref() == Some(uid.as_str()) {
            let generation = self
                .profile_store
                .generation(&uid, runtime::parse_profile(&remote.yaml)?)?;
            let config = self.finish_generation(generation).await?;
            self.apply_with_change(
                config,
                Some(uid.clone()),
                Some(ProfileChange::Refresh(Box::new(remote))),
            )
            .await?;
        } else {
            let result = (|| {
                self.profile_store.begin_refresh(&uid, remote, None)?;
                self.profile_store.publish_refresh()
            })();
            let recovery = self.profile_store.recover_refresh(self.store.state().current.as_ref());
            if let Err(error) = result {
                return Err(error.context(format!("refresh recovery: {recovery:?}")));
            }
            recovery?;
        }
        Ok(self.profile_store.get_item(&uid)?.clone())
    }

    async fn stage(
        &mut self,
        config: ConfigCandidate,
        active_profile: Option<String>,
    ) -> Result<(PathBuf, Option<DnsOverrideState>)> {
        let (config, runtime, dns) = match config {
            ConfigCandidate::Raw(config) => {
                let dns = active_profile
                    .as_deref()
                    .map(|uid| {
                        self.profile_store
                            .dns_source(uid)
                            .map(|source| self.dns_decision(uid, source))
                    })
                    .transpose()?;
                let runtime = dns
                    .as_ref()
                    .map_or_else(|| self.settings.runtime.clone(), |state| self.dns_runtime(state));
                (runtime.prepare(config)?, runtime, dns)
            }
            ConfigCandidate::Enhanced { config, runtime, dns } => (config, *runtime, Some(*dns)),
        };
        let config = self.enforce_runtime_settings(config, &runtime)?;
        let config = headless_core::enhance::finalize::finalize(config);
        let revision = self.store.stage(config)?;
        let path = self.store.path(&revision)?;
        crate::validation::validate(
            &self.options.binary,
            &self.options.data_dir,
            &path,
            &mut self.shutdown,
            self.options.policy.validation_timeout,
        )
        .await?;
        ensure!(
            !*self.shutdown.borrow(),
            "configuration application cancelled during shutdown"
        );
        let previous = self.store.state();
        if let Err(error) = self.store.begin_profile(revision, active_profile) {
            let rollback = self.store.restore(previous);
            return Err(error.context(format!("candidate journal failed; recovery: {rollback:?}")));
        }
        Ok((path, dns))
    }

    async fn apply(&mut self, config: impl Into<ConfigCandidate>, active_profile: Option<String>) -> Result<()> {
        self.apply_with_change(config, active_profile, None).await
    }

    async fn apply_with_change(
        &mut self,
        config: impl Into<ConfigCandidate>,
        active_profile: Option<String>,
        change: Option<ProfileChange>,
    ) -> Result<()> {
        let previous = self.store.state();
        let old_path = self.options.config.clone();
        let was_running = self.status.borrow().phase == CorePhase::Running;
        let (candidate, dns) = self.stage(config.into(), active_profile.clone()).await?;
        let refreshing = matches!(&change, Some(ProfileChange::Refresh(_) | ProfileChange::RawEdit { .. }));
        let merging = matches!(&change, Some(ProfileChange::Enhancement(_)));
        let mut settings_candidate = match &change {
            Some(ProfileChange::Settings(settings)) => (**settings).clone(),
            _ => self.settings.clone(),
        };
        if let Some(state) = &dns
            && (settings_candidate.runtime.dns.is_some() || settings_candidate.profile_dns.contains_key(&state.uid))
        {
            settings_candidate
                .profile_dns
                .insert(state.uid.clone(), ProfileDnsSettings { enabled: state.enabled });
        }
        let setting = settings_candidate != self.settings_store.snapshot();
        // A valid manual apply cancels automatic recovery; it remains stopped if stopped.
        self.retry_at = None;
        self.cancel_restoration();
        let result = async {
            if setting {
                self.settings_store.begin(settings_candidate, self.store.state().pending.context("settings candidate revision missing")?)?;
            }
            match change {
                Some(ProfileChange::Settings(_)) => {},
                Some(ProfileChange::RawEdit { revision, yaml }) => self.profile_store.begin_raw_edit(
                    active_profile.as_deref().context("raw profile UID missing")?, &revision, &yaml, self.store.state().pending,
                )?,
                Some(ProfileChange::Refresh(remote)) => self.profile_store.begin_refresh(
                    active_profile.as_deref().context("refresh profile UID missing")?,
                    *remote,
                    self.store.state().pending,
                )?,
                Some(ProfileChange::Enhancement(plan)) => self
                    .profile_store
                    .begin_enhancement(*plan, self.store.state().pending)?,
                None => {}
            }
            if was_running {
                if let Err(error) = self.reload(&candidate).await {
                    ensure!(
                        !*self.shutdown.borrow(),
                        "application cancelled during shutdown: {error:#}"
                    );
                    self.logs.append(
                        "manager",
                        format!("reload failed; restarting with candidate: {error:#}"),
                    );
                    self.publish(CorePhase::Stopping, None);
                    self.stop_process().await?;
                    self.options.config = candidate.clone();
                    self.start_core().await?;
                }
            } else {
                self.options.config = candidate;
            }
            ensure!(
                !*self.shutdown.borrow(),
                "configuration commit cancelled during shutdown"
            );
            if refreshing {
                self.profile_store.publish_refresh()?;
            }
            if merging {
                self.profile_store.publish_enhancement()?;
            }
            if setting {
                self.settings_store.publish()?;
            }
            self.profile_store.set_current(active_profile.as_deref())?;
            self.store.commit()?;
            self.settings = self.settings_store.snapshot();
            if let Some(state) = &dns {
                if self.dns_confirmations.get(&state.uid).map(String::as_str) != state.source.as_deref() || !state.enabled {
                    self.dns_confirmations.remove(&state.uid);
                }
                if state.requested && !state.enabled {
                    self.logs.append("settings", format!("DNS override auto-disabled for profile {}: provider DNS requires current-session confirmation", state.uid));
                }
            }
            self.profile_state.send_replace(self.profile_store.snapshot());
            self.status.send_modify(|state| {
                state.config_revision = self.store.state().current.map(|revision| revision.file);
                state.active_profile = self.store.state().active_profile;
                state.error = None;
                state.recovery_attempt = 0;
                if !was_running {
                    state.phase = CorePhase::Stopped;
                }
            });
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = result {
            self.options.config = old_path;
            let disk = self.store.restore(previous);
            let settings_rollback = self.settings_store.recover(self.store.state().current.as_ref());
            self.settings = self.settings_store.snapshot();
            let refresh_rollback = self
                .profile_store
                .recover_refresh(self.store.state().current.as_ref())
                .and_then(|()| {
                    self.profile_store
                        .recover_enhancement(self.store.state().current.as_ref())
                });
            let profile_rollback = self
                .profile_store
                .set_current(self.store.state().active_profile.as_deref());
            self.profile_state.send_replace(self.profile_store.snapshot());
            let recovery = if was_running && !*self.shutdown.borrow() {
                self.publish(CorePhase::Stopping, None);
                match self.stop_process().await {
                    Ok(()) => self.start_core().await,
                    Err(error) => Err(error),
                }
            } else {
                Ok(())
            };
            let mut message = format!("configuration application failed: {error:#}");
            if let Err(error) = settings_rollback {
                message.push_str(&format!("; settings rollback failed: {error:#}"));
            }
            if let Err(error) = refresh_rollback {
                message.push_str(&format!("; profile change rollback failed: {error:#}"));
            }
            if let Err(error) = profile_rollback {
                message.push_str(&format!("; profile rollback failed: {error:#}"));
            }
            if let Err(error) = disk {
                message.push_str(&format!("; disk rollback failed: {error:#}"));
            }
            if let Err(error) = recovery {
                message.push_str(&format!("; core recovery failed: {error:#}"));
            }
            bail!(message);
        }
        // Cleanup errors remain observable without undoing an already committed profile change.
        self.profile_store
            .recover_refresh(self.store.state().current.as_ref())?;
        self.profile_store
            .recover_enhancement(self.store.state().current.as_ref())?;
        if setting {
            self.settings_store.recover(self.store.state().current.as_ref())?;
        }
        if was_running {
            self.begin_restoration(true).await;
        }
        Ok(())
    }

    async fn reload(&mut self, path: &Path) -> Result<()> {
        let path = tokio::fs::canonicalize(path).await?;
        check_config(&path).await?;
        tokio::select! {
            biased;
            _ = closing(&mut self.shutdown) => bail!("reload cancelled during shutdown"),
            result = self.client.reload_config(true, path.to_str().context("config path must be UTF-8")?) => result?,
        }
        self.verify_proxy_ports(&path).await?;
        self.options.config = path;
        self.status.send_modify(|state| state.error = None);
        Ok(())
    }

    async fn verify_proxy_ports(&mut self, path: &Path) -> Result<()> {
        let config = read_config(path).await?;
        let core = tokio::select! {
            biased;
            _ = closing(&mut self.shutdown) => bail!("listener verification cancelled during shutdown"),
            result = timeout(self.options.policy.probe_timeout, self.client.get_base_config()) => {
                result.context("listener verification timed out")??
            }
        };
        crate::proxy_access::verify_ports(&config, &core)
    }

    async fn recover_core_upgrade(&mut self) -> Result<()> {
        if self.options.resources.is_none() {
            return Ok(());
        }
        let core = self
            .options
            .binary
            .parent()
            .context("managed core directory missing")?
            .to_path_buf();
        if crate::core_upgrade::pending(&core)? {
            self.retry_at = None;
            self.publish(CorePhase::Stopping, None);
            self.stop_process().await?;
        }
        if let Some(was_running) = crate::core_upgrade::recover(&core)? {
            self.publish(CorePhase::Stopped, None);
            if was_running && !*self.shutdown.borrow() {
                self.publish(CorePhase::Starting, None);
                self.start_inner().await?;
                self.begin_restoration(false).await;
            }
        }
        Ok(())
    }

    async fn installed_version(&mut self) -> Result<String> {
        if self.options.resources.is_some() {
            crate::core_upgrade::repairable(&self.options.binary)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let metadata = std::fs::symlink_metadata(&self.options.binary)?;
                if metadata.len() == 0 || metadata.permissions().mode() & 0o500 != 0o500 {
                    return Ok("unknown".into());
                }
            }
        }
        match crate::validation::probe_version(&self.options.binary, &mut self.shutdown, Duration::from_secs(5)).await {
            Ok(version) => Ok(version),
            Err(_) if self.options.resources.is_some() && !*self.shutdown.borrow() => {
                // An unreadable/broken core must not block the operation that repairs it.
                crate::core_upgrade::repairable(&self.options.binary)?;
                Ok("unknown".into())
            }
            Err(error) => Err(error),
        }
    }

    async fn check_upgrade(&mut self, version: &str, force: bool) -> Result<Option<CoreUpgradeReport>> {
        let from = self.installed_version().await?;
        Ok((!force && from == version).then(|| CoreUpgradeReport {
            upgraded: false,
            from,
            to: version.into(),
        }))
    }

    async fn upgrade_prepared(
        &mut self,
        prepared: crate::core_release::PreparedCore,
        force: bool,
        downloads: Arc<crate::core_release::CoreDownloads>,
    ) -> Result<CoreUpgradeReport> {
        // Lifecycle/configuration commands may have run while the download was in flight.
        if let Some(report) = self.check_upgrade(&prepared.release.version, force).await? {
            return Ok(report);
        }
        let yaml = serde_yaml_ng::to_string(&read_config(&self.options.config).await?)?;
        let revision = self.store.state().current.map(|revision| revision.file);
        let stage = downloads
            .stage(&prepared.id, yaml, revision, &self.options.data_dir, &mut self.shutdown)
            .await?;
        let activated = self.activate_core(&stage.stage_id, downloads).await?;
        Ok(CoreUpgradeReport {
            upgraded: activated.upgraded,
            from: activated.from,
            to: activated.to,
        })
    }

    async fn activate_core(
        &mut self,
        id: &str,
        downloads: Arc<crate::core_release::CoreDownloads>,
    ) -> Result<CoreActivation> {
        self.observe_exit().await?;
        let staged = downloads.inspect_stage(id)?;
        let yaml = serde_yaml_ng::to_string(&read_config(&self.options.config).await?)?;
        ensure!(
            crate::core_upgrade::config_hash(yaml.as_bytes()) == staged.config_sha256,
            "current configuration changed; stage the core again"
        );
        let revision = self.store.state().current.map(|revision| revision.file);
        let checked = downloads
            .stage(
                &staged.prepared.id,
                yaml,
                revision,
                &self.options.data_dir,
                &mut self.shutdown,
            )
            .await?;
        ensure!(checked.stage_id == id, "candidate configuration proof changed");
        let source = downloads.staged_binary(id)?;
        let from = self.installed_version().await?;
        let core = self
            .options
            .binary
            .parent()
            .context("managed core directory missing")?
            .to_path_buf();
        let was_running = self.status.borrow().phase == CorePhase::Running;
        let repair = from == "unknown";
        self.retry_at = None;
        let mut stopped = false;
        let result = async {
            let directory = core.clone();
            let candidate = checked.clone();
            let stop = self.shutdown.clone();
            let receipt = tokio::task::spawn_blocking(move || {
                ensure!(!*stop.borrow(), "core activation cancelled during shutdown");
                crate::core_upgrade::prepare_with_repair(&directory, &source, &candidate, was_running, repair)
            })
            .await
            .context("core replacement worker failed")??;
            ensure!(!*self.shutdown.borrow(), "core activation cancelled during shutdown");
            self.publish(CorePhase::Stopping, None);
            self.stop_process().await?;
            stopped = true;
            ensure!(!*self.shutdown.borrow(), "core activation cancelled during shutdown");
            crate::core_upgrade::publish(&core)?;
            self.publish(CorePhase::Starting, None);
            self.start_inner().await?;
            ensure!(
                self.status.borrow().version.as_deref() == Some(&receipt.version),
                "activated runtime version differs from candidate"
            );
            // A second live probe catches immediate exits after the first readiness response.
            tokio::select! {biased;
                _ = closing(&mut self.shutdown) => bail!("core activation cancelled during shutdown"),
                _ = sleep(self.options.policy.probe_interval) => {},
            }
            self.observe_exit().await?;
            ensure!(
                self.status.borrow().phase == CorePhase::Running,
                "candidate exited during activation health check"
            );
            let version = timeout(self.options.policy.probe_timeout, self.client.get_version())
                .await
                .context("activation health check timed out")??;
            ensure!(
                version.version == receipt.version,
                "activated core failed version health check"
            );
            self.verify_proxy_ports(&self.options.config.clone()).await?;
            if !was_running {
                self.stop_process().await?;
                self.publish(CorePhase::Stopped, None);
            }
            ensure!(!*self.shutdown.borrow(), "core activation cancelled during shutdown");
            crate::core_upgrade::commit(&core)?;
            crate::core_upgrade::recover(&core)?;
            Ok::<_, anyhow::Error>(receipt)
        }
        .await;
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(error) => {
                // A committed marker survives metadata/cleanup errors; never roll it back.
                if !crate::core_upgrade::pending(&core)? {
                    let committed = core.join(".core-upgrade/journal.json").try_exists()?;
                    if committed {
                        crate::core_upgrade::recover(&core).context("activated core metadata recovery required")?;
                        crate::core_upgrade::installation(&core)?.context("activated core receipt missing")?
                    } else {
                        // Preparation failed, or a committed cleanup failed after removing the log.
                        crate::core_upgrade::recover(&core)?;
                        if stopped && crate::core_upgrade::installation(&core)?.is_some_and(|r| r.stage_id == id) {
                            crate::core_upgrade::installation(&core)?.expect("checked receipt")
                        } else {
                            return Err(error);
                        }
                    }
                } else {
                    self.retry_at = None;
                    self.publish(CorePhase::Stopping, None);
                    if let Err(cleanup) = self.stop_process().await {
                        self.publish(
                            CorePhase::Failed,
                            Some("core upgrade rollback requires process cleanup".into()),
                        );
                        return Err(error.context(format!("core rollback cleanup failed: {cleanup:#}")));
                    }
                    if let Err(recovery) = crate::core_upgrade::recover(&core) {
                        self.publish(
                            CorePhase::Failed,
                            Some("core upgrade rollback recovery required".into()),
                        );
                        return Err(error.context(format!("core upgrade rollback recovery required: {recovery:#}")));
                    }
                    self.publish(CorePhase::Stopped, None);
                    if was_running && !*self.shutdown.borrow() {
                        self.publish(CorePhase::Starting, None);
                        if let Err(restart) = self.start_inner().await {
                            let _ = self.stop_process().await;
                            self.publish(
                                CorePhase::Failed,
                                Some("previous core restored but failed to restart".into()),
                            );
                            self.schedule_recovery();
                            return Err(error.context(format!("previous core restart failed: {restart:#}")));
                        }
                        self.begin_restoration(false).await;
                    }
                    let message = "core activation failed; previous core restored";
                    self.status.send_modify(|state| state.error = Some(message.into()));
                    bail!(message);
                }
            }
        };
        self.retry_at = None;
        if was_running {
            self.begin_restoration(false).await;
        }
        Ok(CoreActivation {
            upgraded: true,
            from,
            to: receipt.version.clone(),
            installation: receipt,
            status: self.status.borrow().clone(),
        })
    }

    async fn start_core(&mut self) -> Result<()> {
        if self.options.resources.is_some() && self.process.is_none() {
            crate::core_upgrade::recover(self.options.binary.parent().context("managed core directory missing")?)?;
        }
        self.cancel_restoration();
        self.publish(CorePhase::Starting, None);
        let previous = self.store.state();
        let bootstrap = previous.current.is_none() && previous.pending.is_none();
        let source = self.options.config.clone();
        let result = async {
            if bootstrap {
                let config = read_config(&source).await?;
                self.options.config = self.stage(config.into(), None).await?.0;
            }
            self.start_inner().await?;
            if bootstrap {
                self.store.commit()?;
                self.status.send_modify(|state| {
                    state.config_revision = self.store.state().current.map(|revision| revision.file);
                });
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = result {
            let mut error = error;
            if bootstrap {
                self.options.config = source;
                if let Err(rollback) = self.store.restore(previous) {
                    error = error.context(format!("initial configuration rollback failed: {rollback:#}"));
                }
            }
            let cleanup = self.stop_process().await;
            let message = match cleanup {
                Ok(()) => format!("{error:#}"),
                Err(cleanup) => format!("{error:#}; cleanup failed: {cleanup:#}"),
            };
            self.publish(CorePhase::Failed, Some(message.clone()));
            return Err(anyhow::Error::msg(message));
        }
        if self.store.state().pending.is_none() {
            self.begin_restoration(false).await;
        }
        Ok(())
    }

    async fn start_inner(&mut self) -> Result<()> {
        ensure!(
            self.process.is_none(),
            "previous core must be stopped before another is spawned"
        );
        check_config(&self.options.config).await?;
        if self.store.state().pending.is_none() {
            crate::validation::validate(
                &self.options.binary,
                &self.options.data_dir,
                &self.options.config,
                &mut self.shutdown,
                self.options.policy.validation_timeout,
            )
            .await?;
        }
        #[cfg(unix)]
        if tokio::fs::try_exists(&self.socket).await? {
            use std::os::unix::fs::FileTypeExt as _;
            ensure!(
                tokio::fs::symlink_metadata(&self.socket).await?.file_type().is_socket(),
                "controller path is not a socket"
            );
            match tokio::net::UnixStream::connect(&self.socket).await {
                Ok(_) => bail!("controller socket is already in use"),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    tokio::fs::remove_file(&self.socket).await?;
                }
                Err(error) => return Err(error).context("cannot inspect existing controller socket"),
            }
        }
        let mut command = Command::new(&self.options.binary);
        crate::shutdown::bind_child_lifetime(&mut command);
        let mut child = command
            .arg("-d")
            .arg(&self.options.data_dir)
            .arg("-f")
            .arg(&self.options.config)
            .arg(if cfg!(windows) {
                "-ext-ctl-pipe"
            } else {
                "-ext-ctl-unix"
            })
            .arg(&self.socket)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn Mihomo")?;
        let stdout = child.stdout.take().context("Mihomo stdout unavailable")?;
        let stderr = child.stderr.take().context("Mihomo stderr unavailable")?;
        let readers = vec![
            drain(stdout, self.logs.clone(), "stdout"),
            drain(stderr, self.logs.clone(), "stderr"),
        ];
        self.process = Some(ManagedProcess { child, readers });
        self.publish(CorePhase::Starting, None);
        let mut last_error = String::from("no readiness response");
        for _ in 0..self.options.policy.readiness_attempts {
            if let Some(exit) = self
                .process
                .as_mut()
                .context("core process missing")?
                .child
                .try_wait()?
            {
                self.status.send_modify(|state| state.exit_code = exit.code());
                bail!("Mihomo exited before readiness: {exit}");
            }
            let probe = tokio::select! {
                biased;
                _ = closing(&mut self.shutdown) => bail!("startup cancelled during shutdown"),
                probe = timeout(self.options.policy.probe_timeout, self.client.get_version()) => probe,
            };
            match probe {
                Ok(Ok(version)) => {
                    self.verify_proxy_ports(&self.options.config.clone()).await?;
                    self.status.send_modify(|state| {
                        state.phase = CorePhase::Running;
                        state.pid = self.process.as_ref().and_then(|process| process.child.id());
                        state.error = None;
                        state.version = Some(version.version);
                        state.generation += 1;
                    });
                    return Ok(());
                }
                Ok(Err(error)) => last_error = error.to_string(),
                Err(error) => last_error = error.to_string(),
            }
            tokio::select! {
                biased;
                _ = closing(&mut self.shutdown) => bail!("startup cancelled during shutdown"),
                _ = sleep(self.options.policy.probe_interval) => {}
            }
        }
        bail!("Mihomo API did not become ready: {last_error}")
    }

    async fn stop_process(&mut self) -> Result<()> {
        self.cancel_restoration();
        self.client.clear_all_ws_connections()?;
        let Some(process) = &mut self.process else {
            return Ok(());
        };
        let exit = match process.child.try_wait()? {
            Some(exit) => exit,
            None => {
                #[cfg(unix)]
                if let Some(pid) = process.child.id() {
                    // The actor owns this unreaped child, so the PID cannot be reused here.
                    let sent = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                    if sent != 0 {
                        self.logs.append(
                            "manager",
                            format!("SIGTERM failed: {}", std::io::Error::last_os_error()),
                        );
                    }
                }
                #[cfg(not(unix))]
                process.child.start_kill()?;
                match timeout(self.options.policy.stop_timeout, process.child.wait()).await {
                    Ok(result) => result?,
                    Err(_) => {
                        self.logs
                            .append("manager", "graceful stop timed out; forcing termination".to_owned());
                        process.child.start_kill()?;
                        timeout(self.options.policy.kill_timeout, process.child.wait())
                            .await
                            .context("timed out reaping Mihomo")??
                    }
                }
            }
        };
        self.status.send_modify(|state| state.exit_code = exit.code());
        for reader in &mut process.readers {
            if timeout(Duration::from_secs(1), &mut *reader).await.is_err() {
                reader.abort();
                let _ = reader.await;
            }
        }
        self.process = None;
        Ok(())
    }

    async fn observe_exit(&mut self) -> Result<()> {
        if self.status.borrow().phase != CorePhase::Running {
            return Ok(());
        }
        let exit: Option<ExitStatus> = match &mut self.process {
            Some(process) => process.child.try_wait()?,
            None => None,
        };
        if let Some(exit) = exit {
            self.stop_process().await?;
            self.publish(CorePhase::Failed, Some(format!("Mihomo exited unexpectedly: {exit}")));
            self.schedule_recovery();
        }
        Ok(())
    }

    fn schedule_recovery(&mut self) {
        if *self.shutdown.borrow() || self.process.is_some() {
            return;
        }
        let mut state = self.status.borrow().clone();
        if state.recovery_attempt >= self.options.policy.recovery_limit {
            return;
        }
        state.recovery_attempt += 1;
        state.phase = CorePhase::Recovering;
        let multiplier = 1_u32 << (state.recovery_attempt - 1);
        self.retry_at = Some(Instant::now() + self.options.policy.recovery_backoff * multiplier);
        self.status.send_replace(state);
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
#[path = "core_upgrade_adapter_tests.rs"]
mod upgrade_tests;
