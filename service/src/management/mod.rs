//! Authenticated commands and their HTTP transport.
mod assets;
pub mod auth;
pub mod http;
pub mod preferences;
pub mod update_checks;
mod websocket;

use crate::core_manager::{CoreManager, CorePhase};
use anyhow::{Result, ensure};
use auth::Authentication;
use headless_core::config::{ProviderAction, ProviderOperationReceipt, profile_store::SequenceKind, runtime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagementCommand {
    Status {},
    ServiceVersion {},
    /// This program: version, release, systemd unit and update unit state.
    ServiceInfo {},
    /// Latest published mihomo-server release tag (recorded as a check).
    ServiceRelease {},
    /// The latest recorded core and service update checks.
    UpdateChecks {},
    /// Stop or restart this service (not the core) after answering.
    StopService {},
    RestartService {},
    /// Start the shared installation's update unit.
    UpgradeService {},
    /// Enable or disable this service's systemd unit (start at login).
    SetServiceAutostart {
        enabled: bool,
    },
    CoreRelease {
        version: Option<String>,
    },
    PrepareCoreUpgrade {
        version: Option<String>,
    },
    AlphaCoreRelease {
        version: Option<String>,
    },
    PrepareAlphaCoreUpgrade {
        version: Option<String>,
    },
    PreparedCoreUpgrade {
        id: String,
    },
    StageCoreUpgrade {
        id: String,
    },
    StagedCoreUpgrade {
        id: String,
    },
    ActivateCoreUpgrade {
        id: String,
    },
    CoreInstallation {},
    InstalledCoreVersion {},
    MultiUser {},
    UpgradeClashCore {
        force: bool,
    },
    UpgradeAlphaCore {
        force: bool,
    },
    Logs {},
    Profiles {},
    Config {},
    Settings {},
    Resources {},
    Rules {},
    RuleProviders {},
    UpdateRuleProvider {
        name: String,
    },
    ProxyProviders {},
    UpdateProxyProvider {
        name: String,
    },
    HealthcheckProxyProvider {
        name: String,
    },
    DelayProxy {
        name: String,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        timeout: Option<u32>,
    },
    DelayGroup {
        group: String,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        timeout: Option<u32>,
    },
    GeoSettings {},
    UpdateGeo {},
    ConnectionSettings {},
    #[cfg(unix)]
    GeoSeed {
        name: String,
    },
    #[cfg(unix)]
    InstallGeoSeed {
        name: String,
        expected_current_sha256: Option<String>,
        expected_seed_sha256: String,
        #[serde(default)]
        accept_metadata_only: bool,
    },
    #[cfg(unix)]
    GeoOnlineInfo {
        name: String,
    },
    #[cfg(unix)]
    UpdateGeoOnline {
        name: String,
        expected_current_sha256: Option<String>,
        expected_source_sha256: String,
        #[serde(default)]
        expected_download_sha256: Option<String>,
        #[serde(default)]
        accept_metadata_only: bool,
        #[serde(default)]
        route: crate::geo::online::RouteChoice,
        #[serde(default)]
        danger_accept_invalid_certs: bool,
    },
    ValidateGeo {
        name: String,
    },
    ProxyAccess {},
    SetProxyMode {
        mode: headless_core::config::settings::Mode,
    },
    SetTunEnabled {
        enabled: bool,
    },
    ProfileDns {
        uid: String,
    },
    SetProfileDns {
        uid: String,
        enabled: bool,
        confirmation: Option<String>,
    },
    SetSettings {
        runtime: Box<headless_core::config::settings::RuntimeSettings>,
    },
    Proxies {},
    Start {},
    Stop {},
    Restart {},
    ApplyConfig {
        yaml: String,
    },
    EditConfig {
        yaml: String,
    },
    ApplyOverlay {
        yaml: String,
    },
    ImportProfile {
        name: String,
        yaml: String,
    },
    ImportRemoteProfile {
        url: String,
        name: Option<String>,
        #[serde(default)]
        options: crate::remote::RemoteOptions,
    },
    ProfileRaw {
        uid: String,
    },
    SetProfileRaw {
        uid: String,
        revision: String,
        yaml: String,
    },
    ProfileMerge {
        uid: String,
    },
    GlobalMerge {},
    SetGlobalMerge {
        yaml: String,
    },
    ResetGlobalMerge {},
    GlobalScript {},
    SetGlobalScript {
        source: String,
    },
    ResetGlobalScript {},
    SetProfileMerge {
        uid: String,
        yaml: String,
    },
    ClearProfileMerge {
        uid: String,
    },
    ProfileSequence {
        uid: String,
        kind: SequenceKind,
    },
    SetProfileSequence {
        uid: String,
        kind: SequenceKind,
        yaml: String,
    },
    ClearProfileSequence {
        uid: String,
        kind: SequenceKind,
    },
    ProfileScript {
        uid: String,
    },
    SetProfileScript {
        uid: String,
        source: String,
    },
    ClearProfileScript {
        uid: String,
    },
    EditProfile {
        uid: String,
        patch: headless_core::config::profile_store::ProfilePatch,
    },
    DeleteProfile {
        uid: String,
    },
    SelectProfile {
        uid: String,
    },
    RefreshProfile {
        uid: String,
    },
    SelectNode {
        group: String,
        node: String,
    },
    UnfixNode {
        group: String,
    },
    /// Interface preferences shared by this instance's clients.
    Preferences {},
    /// `null` clears it: each client then uses its own default.
    SetLanguage {
        language: Option<preferences::Language>,
    },
}

#[derive(Clone, Copy)]
pub struct RequestCredentials<'a> {
    pub host: &'a str,
    pub origin: Option<&'a str>,
    pub authorization: Option<&'a str>,
}

pub struct Management {
    manager: CoreManager,
    authentication: Authentication,
    preferences: preferences::PreferenceStore,
    update_checks: update_checks::UpdateCheckStore,
}

impl Management {
    /// Preferences stay in memory unless [`Self::with_preferences`] persists them.
    pub fn new(manager: CoreManager, authentication: Authentication) -> Self {
        Self {
            manager,
            authentication,
            preferences: preferences::PreferenceStore::in_memory(),
            update_checks: update_checks::UpdateCheckStore::in_memory(),
        }
    }

    pub fn with_preferences(mut self, preferences: preferences::PreferenceStore) -> Self {
        self.preferences = preferences;
        self
    }

    pub fn with_update_checks(mut self, update_checks: update_checks::UpdateCheckStore) -> Self {
        self.update_checks = update_checks;
        self
    }

    /// Records a check for the latest release against the installed core.
    /// The answer stands even when the record cannot be kept.
    async fn checked_core_release(
        &self,
        version: Option<String>,
        channel: update_checks::CoreChannel,
    ) -> Result<crate::core_release::CoreRelease> {
        let latest = version.is_none();
        let installed = if latest {
            self.manager.installed_core_version().await.ok()
        } else {
            None
        };
        let release = match channel {
            update_checks::CoreChannel::Stable => self.manager.core_release(version).await?,
            update_checks::CoreChannel::Alpha => self.manager.alpha_core_release(version).await?,
        };
        if let Some(installed) = installed {
            let check = update_checks::CoreCheck {
                channel,
                installed,
                latest: release.version.clone(),
            };
            if let Err(error) = self.update_checks.record_core(check).await {
                eprintln!("cannot record core update check: {error:#}");
            }
        }
        Ok(release)
    }

    pub fn authorize(&self, credentials: RequestCredentials<'_>) -> Result<()> {
        self.authentication
            .authorize(credentials.host, credentials.origin, credentials.authorization)
    }

    /// Every read and mutation passes the same authentication boundary.
    /// Config/profile input is content, never a path supplied by the browser.
    pub async fn execute(&self, credentials: RequestCredentials<'_>, command: ManagementCommand) -> Result<Value> {
        self.authorize(credentials)?;
        ensure!(
            self.manager.status().phase != CorePhase::Shutdown,
            "service is shutting down"
        );
        let value = match command {
            ManagementCommand::ProfileRaw { uid } => serde_json::to_value(self.manager.profile_raw(uid).await?)?,
            ManagementCommand::SetProfileRaw { uid, revision, yaml } => {
                serde_json::to_value(self.manager.set_profile_raw(uid, revision, yaml).await?)?
            }
            ManagementCommand::ProfileDns { uid } => serde_json::to_value(self.manager.profile_dns(uid).await?)?,
            ManagementCommand::SetProfileDns {
                uid,
                enabled,
                confirmation,
            } => serde_json::to_value(self.manager.set_profile_dns(uid, enabled, confirmation).await?)?,
            ManagementCommand::CoreRelease { version } => serde_json::to_value(
                self.checked_core_release(version, update_checks::CoreChannel::Stable)
                    .await?,
            )?,
            ManagementCommand::PrepareCoreUpgrade { version } => {
                serde_json::to_value(self.manager.prepare_core_upgrade(version).await?)?
            }
            ManagementCommand::AlphaCoreRelease { version } => serde_json::to_value(
                self.checked_core_release(version, update_checks::CoreChannel::Alpha)
                    .await?,
            )?,
            ManagementCommand::PrepareAlphaCoreUpgrade { version } => {
                serde_json::to_value(self.manager.prepare_alpha_core_upgrade(version).await?)?
            }
            ManagementCommand::PreparedCoreUpgrade { id } => {
                serde_json::to_value(self.manager.prepared_core_upgrade(&id)?)?
            }
            ManagementCommand::StageCoreUpgrade { id } => {
                serde_json::to_value(self.manager.stage_core_upgrade(id).await?)?
            }
            ManagementCommand::StagedCoreUpgrade { id } => {
                serde_json::to_value(self.manager.staged_core_upgrade(&id)?)?
            }
            ManagementCommand::ActivateCoreUpgrade { id } => {
                serde_json::to_value(self.manager.activate_core_upgrade(id).await?)?
            }
            ManagementCommand::CoreInstallation {} => serde_json::to_value(self.manager.core_installation().await?)?,
            ManagementCommand::MultiUser {} => {
                let mut value = serde_json::to_value(self.manager.multi_user())?;
                if let Some(user) = value.as_object_mut() {
                    user.insert("tun_holder".into(), crate::proxy_access::tun_holder(&self.manager));
                }
                value
            }
            ManagementCommand::InstalledCoreVersion {} => {
                serde_json::to_value(self.manager.installed_core_version().await?)?
            }
            ManagementCommand::UpgradeClashCore { force } => {
                serde_json::to_value(self.manager.upgrade_clash_core(force).await?)?
            }
            ManagementCommand::UpgradeAlphaCore { force } => {
                serde_json::to_value(self.manager.upgrade_alpha_core(force).await?)?
            }
            ManagementCommand::Status {} => serde_json::to_value(self.manager.status())?,
            ManagementCommand::ServiceVersion {} => serde_json::to_value(env!("CARGO_PKG_VERSION"))?,
            // systemctl runs off the async workers.
            ManagementCommand::ServiceInfo {} => {
                serde_json::to_value(tokio::task::spawn_blocking(crate::service_control::info).await?)?
            }
            ManagementCommand::ServiceRelease {} => {
                let latest = self.manager.latest_service_release().await?;
                let check = update_checks::ServiceCheck {
                    installed: crate::cli::system::installed_release(),
                    latest: latest.clone(),
                };
                if let Err(error) = self.update_checks.record_service(check).await {
                    eprintln!("cannot record service update check: {error:#}");
                }
                serde_json::to_value(latest)?
            }
            ManagementCommand::UpdateChecks {} => serde_json::to_value(self.update_checks.get().await)?,
            ManagementCommand::StopService {} => {
                tokio::task::spawn_blocking(|| crate::service_control::schedule(crate::service_control::Action::Stop))
                    .await??;
                serde_json::json!({ "action": "stop" })
            }
            ManagementCommand::RestartService {} => {
                tokio::task::spawn_blocking(|| {
                    crate::service_control::schedule(crate::service_control::Action::Restart)
                })
                .await??;
                serde_json::json!({ "action": "restart" })
            }
            ManagementCommand::SetServiceAutostart { enabled } => serde_json::to_value(
                tokio::task::spawn_blocking(move || crate::service_control::set_autostart(enabled)).await??,
            )?,
            ManagementCommand::UpgradeService {} => {
                tokio::task::spawn_blocking(crate::service_control::upgrade).await??;
                serde_json::json!({ "unit": crate::service_control::UPDATE_UNIT })
            }
            ManagementCommand::Logs {} => serde_json::to_value(self.manager.logs())?,
            ManagementCommand::Profiles {} => serde_json::to_value(self.manager.profiles())?,
            ManagementCommand::Settings {} => serde_json::to_value(self.manager.settings().await?)?,
            #[cfg(unix)]
            ManagementCommand::GeoSeed { name } => serde_json::to_value(self.manager.geo_seed_info(name).await?)?,
            #[cfg(unix)]
            ManagementCommand::InstallGeoSeed {
                name,
                expected_current_sha256,
                expected_seed_sha256,
                accept_metadata_only,
            } => serde_json::to_value(
                self.manager
                    .install_geo_seed(crate::geo::update::InstallRequest {
                        name,
                        expected_current_sha256,
                        expected_seed_sha256,
                        accept_metadata_only,
                    })
                    .await?,
            )?,
            #[cfg(unix)]
            ManagementCommand::GeoOnlineInfo { name } => {
                serde_json::to_value(self.manager.geo_online_info(name).await?)?
            }
            #[cfg(unix)]
            ManagementCommand::UpdateGeoOnline {
                name,
                expected_current_sha256,
                expected_source_sha256,
                expected_download_sha256,
                accept_metadata_only,
                route,
                danger_accept_invalid_certs,
            } => serde_json::to_value(
                self.manager
                    .update_geo_online(crate::geo::online::Request {
                        name,
                        expected_current_sha256,
                        expected_source_sha256,
                        expected_download_sha256,
                        accept_metadata_only,
                        route,
                        danger_accept_invalid_certs,
                    })
                    .await?,
            )?,
            ManagementCommand::ValidateGeo { name } => serde_json::to_value(self.manager.validate_geo(name).await?)?,
            ManagementCommand::ConnectionSettings {} => {
                serde_json::to_value(self.manager.connection_settings().await?)?
            }
            ManagementCommand::GeoSettings {} => serde_json::to_value(self.manager.geo_settings().await?)?,
            ManagementCommand::UpdateGeo {} => {
                self.manager.update_geo().await?;
                serde_json::json!({})
            }
            ManagementCommand::Resources {} => serde_json::to_value(self.manager.resource_inventory().await?)?,
            ManagementCommand::Rules {} => serde_json::to_value(self.manager.rules().await?)?,
            ManagementCommand::RuleProviders {} => serde_json::to_value(self.manager.rule_providers().await?)?,
            ManagementCommand::UpdateRuleProvider { name } => {
                self.manager.update_rule_provider(&name).await?;
                serde_json::json!({
                    "name": name,
                    "action": "update",
                    "success": true,
                    "updated": true,
                })
            }
            ManagementCommand::ProxyProviders {} => serde_json::to_value(self.manager.proxy_providers().await?)?,
            ManagementCommand::UpdateProxyProvider { name } => {
                self.manager.update_proxy_provider(&name).await?;
                serde_json::to_value(ProviderOperationReceipt {
                    name,
                    action: ProviderAction::Update,
                    success: true,
                })?
            }
            ManagementCommand::HealthcheckProxyProvider { name } => {
                self.manager.healthcheck_proxy_provider(&name).await?;
                serde_json::to_value(ProviderOperationReceipt {
                    name,
                    action: ProviderAction::Healthcheck,
                    success: true,
                })?
            }
            ManagementCommand::DelayProxy { name, url, timeout } => {
                serde_json::to_value(self.manager.delay_proxy(&name, url.as_deref(), timeout).await?)?
            }
            ManagementCommand::DelayGroup { group, url, timeout } => {
                serde_json::to_value(self.manager.delay_group(&group, url.as_deref(), timeout).await?)?
            }
            ManagementCommand::ProxyAccess {} => crate::proxy_access::inspect(&self.manager).await?,
            ManagementCommand::SetProxyMode { mode } => serde_json::to_value(self.manager.set_proxy_mode(mode).await?)?,
            ManagementCommand::Preferences {} => serde_json::to_value(self.preferences.get())?,
            ManagementCommand::SetLanguage { language } => {
                serde_json::to_value(self.preferences.set_language(language).await?)?
            }
            ManagementCommand::SetTunEnabled { enabled } => {
                serde_json::to_value(self.manager.set_tun_enabled(enabled).await?)?
            }
            ManagementCommand::SetSettings { runtime } => {
                serde_json::to_value(self.manager.set_settings(*runtime).await?)?
            }
            ManagementCommand::Config {} => {
                serde_json::json!({"yaml": serde_yaml_ng::to_string(&self.manager.runtime_config().await?)?})
            }
            ManagementCommand::Proxies {} => {
                ensure!(self.manager.status().phase == CorePhase::Running, "core is not running");
                serde_json::to_value(
                    tokio::time::timeout(std::time::Duration::from_secs(10), self.manager.client().get_proxies())
                        .await
                        .map_err(|_| anyhow::anyhow!("core query timed out"))??,
                )?
            }
            ManagementCommand::Start {} => serde_json::to_value(self.manager.start().await?)?,
            ManagementCommand::Stop {} => serde_json::to_value(self.manager.stop().await?)?,
            ManagementCommand::Restart {} => serde_json::to_value(self.manager.restart().await?)?,
            ManagementCommand::ApplyConfig { yaml } => {
                serde_json::to_value(self.manager.apply_config(runtime::parse(&yaml)?).await?)?
            }
            ManagementCommand::EditConfig { yaml } => {
                serde_json::to_value(self.manager.edit_config(runtime::parse(&yaml)?).await?)?
            }
            ManagementCommand::ApplyOverlay { yaml } => {
                serde_json::to_value(self.manager.apply_overlay(runtime::parse(&yaml)?).await?)?
            }
            ManagementCommand::ImportProfile { name, yaml } => {
                serde_json::to_value(self.manager.import_profile_yaml(yaml, name).await?)?
            }
            ManagementCommand::ImportRemoteProfile { url, name, options } => {
                serde_json::to_value(self.manager.import_remote_profile(url, name, options).await?)?
            }
            ManagementCommand::ProfileMerge { uid } => serde_json::to_value(self.manager.profile_merge(uid).await?)?,
            ManagementCommand::GlobalMerge {} => serde_json::to_value(self.manager.global_merge().await?)?,
            ManagementCommand::SetGlobalMerge { yaml } => {
                serde_json::to_value(self.manager.set_global_merge(Some(yaml)).await?)?
            }
            ManagementCommand::ResetGlobalMerge {} => serde_json::to_value(self.manager.set_global_merge(None).await?)?,
            ManagementCommand::GlobalScript {} => serde_json::to_value(self.manager.global_script().await?)?,
            ManagementCommand::SetGlobalScript { source } => {
                serde_json::to_value(self.manager.set_global_script(Some(source)).await?)?
            }
            ManagementCommand::ResetGlobalScript {} => {
                serde_json::to_value(self.manager.set_global_script(None).await?)?
            }
            ManagementCommand::SetProfileMerge { uid, yaml } => {
                serde_json::to_value(self.manager.set_profile_merge(uid, Some(yaml)).await?)?
            }
            ManagementCommand::ClearProfileMerge { uid } => {
                serde_json::to_value(self.manager.set_profile_merge(uid, None).await?)?
            }
            ManagementCommand::ProfileSequence { uid, kind } => {
                serde_json::to_value(self.manager.profile_sequence(uid, kind).await?)?
            }
            ManagementCommand::SetProfileSequence { uid, kind, yaml } => {
                serde_json::to_value(self.manager.set_profile_sequence(uid, kind, Some(yaml)).await?)?
            }
            ManagementCommand::ClearProfileSequence { uid, kind } => {
                serde_json::to_value(self.manager.set_profile_sequence(uid, kind, None).await?)?
            }
            ManagementCommand::ProfileScript { uid } => serde_json::to_value(self.manager.profile_script(uid).await?)?,
            ManagementCommand::SetProfileScript { uid, source } => {
                serde_json::to_value(self.manager.set_profile_script(uid, Some(source)).await?)?
            }
            ManagementCommand::ClearProfileScript { uid } => {
                serde_json::to_value(self.manager.set_profile_script(uid, None).await?)?
            }
            ManagementCommand::EditProfile { uid, patch } => {
                serde_json::to_value(self.manager.edit_profile(uid, patch).await?)?
            }
            ManagementCommand::DeleteProfile { uid } => serde_json::to_value(self.manager.delete_profile(uid).await?)?,
            ManagementCommand::SelectProfile { uid } => serde_json::to_value(self.manager.select_profile(uid).await?)?,
            ManagementCommand::RefreshProfile { uid } => {
                serde_json::to_value(self.manager.refresh_profile(uid).await?)?
            }
            ManagementCommand::SelectNode { group, node } => {
                serde_json::to_value(self.manager.select_node(group, node).await?)?
            }
            ManagementCommand::UnfixNode { group } => serde_json::to_value(self.manager.unfix_node(group).await?)?,
        };
        Ok(value)
    }
}
