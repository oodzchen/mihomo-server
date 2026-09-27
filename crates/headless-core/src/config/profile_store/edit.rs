//! Restricted metadata patches: ownership fields and raw content are never user-writable.
use super::*;
use crate::config::PrfOption;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteOptionsPatch {
    pub danger_accept_invalid_certs: Option<bool>,
    pub with_proxy: Option<bool>,
    pub self_proxy: Option<bool>,
    pub user_agent: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub update_interval: Option<u64>,
    pub allow_auto_update: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePatch {
    pub name: Option<String>,
    pub desc: Option<String>,
    pub url: Option<String>,
    pub options: Option<RemoteOptionsPatch>,
}

impl ProfileStore {
    /// Adapt upstream patch_item with a strict field allowlist and option merge.
    pub fn edit_profile(&mut self, uid: &str, patch: ProfilePatch) -> Result<PrfItem> {
        ensure!(
            !self.data_dir.join("profile-refresh.yaml").try_exists()?
                && !self.data_dir.join("backup-restore.yaml").try_exists()?
                && !self.data_dir.join("profile-import.yaml").try_exists()?
                && !self.data_dir.join("profile-delete.yaml").try_exists()?
                && !self.data_dir.join("profile-merge.yaml").try_exists()?,
            "profile recovery is pending"
        );
        let previous = self.get_item(uid)?;
        ensure!(
            matches!(previous.itype.as_deref(), Some("local" | "remote")),
            "only local or remote profiles can be edited"
        );
        let mut item = previous.clone();
        if let Some(name) = patch.name {
            ensure!(
                !name.trim().is_empty() && name.len() <= 256,
                "profile name must be 1..256 bytes"
            );
            item.name = Some(name.into());
        }
        if let Some(desc) = patch.desc {
            ensure!(desc.len() <= 4096, "profile description exceeds 4 KiB");
            item.desc = Some(desc.into());
        }
        if patch.url.is_some() || patch.options.is_some() {
            ensure!(
                item.itype.as_deref() == Some("remote"),
                "download settings require a remote profile"
            );
        }
        if let Some(url) = patch.url {
            item.url = Some(subscription_url(&url)?.to_string().into());
        }
        if let Some(option) = patch.options {
            if let Some(agent) = &option.user_agent {
                ensure!(
                    agent.len() <= 1024 && !agent.chars().any(char::is_control),
                    "invalid subscription user agent"
                );
            }
            if let Some(seconds) = option.timeout_seconds {
                ensure!(
                    (1..=120).contains(&seconds),
                    "subscription timeout must be 1..120 seconds"
                );
            }
            item.option = PrfOption::merge(
                item.option.as_ref(),
                Some(&PrfOption {
                    danger_accept_invalid_certs: option.danger_accept_invalid_certs,
                    with_proxy: option.with_proxy,
                    self_proxy: option.self_proxy,
                    user_agent: option.user_agent.map(Into::into),
                    timeout_seconds: option.timeout_seconds,
                    update_interval: option.update_interval,
                    allow_auto_update: option.allow_auto_update,
                    ..Default::default()
                }),
            );
        }
        let mut profiles = self.profiles.clone();
        *profiles
            .items
            .as_mut()
            .and_then(|items| items.iter_mut().find(|item| item.uid.as_deref() == Some(uid)))
            .context("profile missing while editing")? = item.clone();
        self.save(profiles)?;
        Ok(item)
    }
}
