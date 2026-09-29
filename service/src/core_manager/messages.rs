//! Actor mailbox messages and configuration change descriptors.
use super::*;

pub(super) enum Operation {
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

pub(super) struct Request {
    pub(super) operation: Operation,
    pub(super) reply: oneshot::Sender<Result<CoreStatus>>,
}

pub(super) enum ProfileChange {
    Settings(Box<ServiceSettings>),
    RawEdit { revision: String, yaml: String },
    Refresh(Box<headless_core::config::remote::RemoteProfile>),
    Enhancement(Box<EnhancementPlan>),
}

/// Preserve upstream phase order: enhanced candidates must not derive TUN/DNS twice.
pub(super) enum ConfigCandidate {
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
pub(super) enum ProfileEnhancement {
    Global(GlobalEnhancementKind),
    Merge,
    Sequence(SequenceKind),
    Script,
}

pub(super) enum CommandMessage {
    RestoreBackup {
        bytes: axum::body::Bytes,
        policy: headless_core::backup::BackupRuntimePolicy,
        permit: tokio::sync::OwnedSemaphorePermit,
        closing: watch::Receiver<bool>,
        reply: oneshot::Sender<Result<headless_core::backup::BackupRestoreReceipt>>,
    },
    ValidateBackupRestore {
        bytes: axum::body::Bytes,
        permit: tokio::sync::OwnedSemaphorePermit,
        closing: watch::Receiver<bool>,
        reply: oneshot::Sender<Result<headless_core::backup::BackupRestoreValidation>>,
    },
    RetainedBackup {
        operation: crate::backup::RetainedOperation,
        permit: tokio::sync::OwnedSemaphorePermit,
        closing: watch::Receiver<bool>,
        reply: oneshot::Sender<Result<crate::backup::RetainedOutcome>>,
    },
    ExportBackup {
        permit: tokio::sync::OwnedSemaphorePermit,
        reply: oneshot::Sender<Result<crate::backup::BackupDownload>>,
    },
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
    #[cfg(unix)]
    GeoSeedInfo {
        name: String,
        reply: oneshot::Sender<Result<crate::geo_update::SeedInfo>>,
    },
    #[cfg(unix)]
    InstallGeoSeed {
        request: crate::geo_update::InstallRequest,
        reply: oneshot::Sender<Result<crate::geo_update::Receipt>>,
    },
    #[cfg(unix)]
    GeoOnlineInfo {
        name: String,
        reply: oneshot::Sender<Result<crate::geo_online::Info>>,
    },
    #[cfg(unix)]
    UpdateGeoOnline {
        request: crate::geo_online::Request,
        reply: oneshot::Sender<Result<crate::geo_update::Receipt>>,
    },
    ValidateGeo {
        name: String,
        reply: oneshot::Sender<Result<crate::geo_validation::Validation>>,
    },
    ReadGeoSettings(oneshot::Sender<Result<crate::geo_settings::Snapshot>>),
    ReadConnectionSettings(oneshot::Sender<Result<crate::connection_settings::Snapshot>>),
    ReadSettings(oneshot::Sender<Result<ServiceSettings>>),
    ReadResources(oneshot::Sender<Result<crate::resource_inventory::Inventory>>),
    SetSettings {
        runtime: Box<RuntimeSettings>,
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

fn reject<T>(reply: oneshot::Sender<Result<T>>, message: &str) {
    let _ = reply.send(Err(anyhow::Error::msg(message.to_owned())));
}

impl CommandMessage {
    /// Reply with an error without running the command; admission permits drop with it.
    pub(super) fn fail(self, message: &str) {
        match self {
            Self::RestoreBackup { reply, .. } => reject(reply, message),
            Self::ValidateBackupRestore { reply, .. } => reject(reply, message),
            Self::RetainedBackup { reply, .. } => reject(reply, message),
            Self::ExportBackup { reply, .. } => reject(reply, message),
            Self::InstalledCoreVersion(reply) => reject(reply, message),
            Self::CheckCoreUpgrade { reply, .. } => reject(reply, message),
            Self::UpgradePreparedCore { reply, .. } => reject(reply, message),
            Self::ActivateCoreUpgrade { reply, .. } => reject(reply, message),
            Self::CoreInstallation(reply) => reject(reply, message),
            Self::StageCoreUpgrade { reply, .. } => reject(reply, message),
            Self::ReadProfileRaw { reply, .. } | Self::SetProfileRaw { reply, .. } => reject(reply, message),
            Self::ReadProfileDns { reply, .. } => reject(reply, message),
            Self::SetProfileDns { reply, .. } => reject(reply, message),
            #[cfg(unix)]
            Self::GeoSeedInfo { reply, .. } => reject(reply, message),
            #[cfg(unix)]
            Self::InstallGeoSeed { reply, .. } => reject(reply, message),
            #[cfg(unix)]
            Self::GeoOnlineInfo { reply, .. } => reject(reply, message),
            #[cfg(unix)]
            Self::UpdateGeoOnline { reply, .. } => reject(reply, message),
            Self::ValidateGeo { reply, .. } => reject(reply, message),
            Self::ReadGeoSettings(reply) => reject(reply, message),
            Self::ReadConnectionSettings(reply) => reject(reply, message),
            Self::ReadSettings(reply) | Self::SetSettings { reply, .. } => reject(reply, message),
            Self::ReadResources(reply) => reject(reply, message),
            Self::ReadEnhancement { reply, .. } => reject(reply, message),
            Self::SetEnhancement { reply, .. }
            | Self::EditProfile { reply, .. }
            | Self::RefreshRemote { reply, .. }
            | Self::ImportRemote { reply, .. }
            | Self::ImportProfileYaml { reply, .. }
            | Self::ImportProfile { reply, .. } => reject(reply, message),
            Self::DeleteProfile { reply, .. } => reject(reply, message),
            Self::Control(request) => reject(request.reply, message),
            Self::ReadConfig(reply) => reject(reply, message),
        }
    }
}
