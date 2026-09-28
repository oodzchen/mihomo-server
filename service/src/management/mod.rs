//! Authenticated commands and their HTTP transport.
mod assets;
pub mod auth;
pub mod http;
mod websocket;

use crate::core_manager::{CoreManager, CorePhase};
use anyhow::{Result, ensure};
use auth::Authentication;
use headless_core::config::{profile_store::SequenceKind, runtime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagementCommand {
    Status {},
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
    GeoSettings {},
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
        route: crate::geo_online::RouteChoice,
        #[serde(default)]
        danger_accept_invalid_certs: bool,
    },
    ValidateGeo {
        name: String,
    },
    ProxyAccess {},
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
}

impl Management {
    pub fn new(manager: CoreManager, authentication: Authentication) -> Self {
        Self {
            manager,
            authentication,
        }
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
            ManagementCommand::CoreRelease { version } => {
                serde_json::to_value(self.manager.core_release(version).await?)?
            }
            ManagementCommand::PrepareCoreUpgrade { version } => {
                serde_json::to_value(self.manager.prepare_core_upgrade(version).await?)?
            }
            ManagementCommand::AlphaCoreRelease { version } => {
                serde_json::to_value(self.manager.alpha_core_release(version).await?)?
            }
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
                    .install_geo_seed(crate::geo_update::InstallRequest {
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
                    .update_geo_online(crate::geo_online::Request {
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
            ManagementCommand::Resources {} => serde_json::to_value(self.manager.resource_inventory().await?)?,
            ManagementCommand::Rules {} => serde_json::to_value(self.manager.rules().await?)?,
            ManagementCommand::RuleProviders {} => serde_json::to_value(self.manager.rule_providers().await?)?,
            ManagementCommand::UpdateRuleProvider { name } => {
                self.manager.update_rule_provider(&name).await?;
                serde_json::json!({ "name": name, "updated": true })
            }
            ManagementCommand::ProxyAccess {} => crate::proxy_access::inspect(&self.manager).await?,
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
