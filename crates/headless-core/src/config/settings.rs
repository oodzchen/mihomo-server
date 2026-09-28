//! Versioned, service-owned settings. The caller must own the data-directory lock.
//! Absent fields inherit source configuration; owned fields win over enhancements.
//! DNS page ownership additionally follows upstream non-empty/true selection.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Mapping;

use super::runtime::{Revision, sync_directory, unique_id, write_new};

mod geo;
pub use geo::{GeoUrls, GeodataLoader, GeositeMatcher};
mod dns_policy;
pub use dns_policy::{FallbackFilter, ResolverPolicy, ResolverPolicyValue};
mod hosts;
pub use hosts::{HostValue, Hosts};
mod network;
pub use network::{DnsMode, DnsSettings, FakeIpFilterMode, TunSettings, TunStack};

pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Rule,
    Global,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Silent,
    Error,
    Warning,
    Info,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindProcessMode {
    Strict,
    Always,
    Off,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RuntimeSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mixed_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socks_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redir_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tproxy_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_lan: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified_delay: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_concurrent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub find_process_mode: Option<FindProcessMode>,
    // Signed seconds preserve Mihomo/Go zero and negative duration semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive_interval: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive_idle: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_keep_alive: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_string"
    )]
    pub interface_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_mark: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_string"
    )]
    pub global_ua: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag_support: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<LogLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<DnsSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosts: Option<Hosts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tun: Option<TunSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geodata_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geodata_loader: Option<GeodataLoader>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geosite_matcher: Option<GeositeMatcher>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_auto_update: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo_update_interval: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geox_url: Option<GeoUrls>,
}

// YAML strings otherwise coerce numbers/booleans; owned text fields are strict.
fn deserialize_optional_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    match Option::<serde_yaml_ng::Value>::deserialize(deserializer)? {
        None => Ok(None),
        Some(serde_yaml_ng::Value::String(name)) => Ok(Some(name)),
        Some(_) => Err(serde::de::Error::custom("setting must be a string or null")),
    }
}

fn deserialize_optional_string_list<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Vec<String>>, D::Error> {
    match Option::<serde_yaml_ng::Value>::deserialize(deserializer)? {
        None => Ok(None),
        Some(serde_yaml_ng::Value::Sequence(values)) => values
            .into_iter()
            .map(|value| match value {
                serde_yaml_ng::Value::String(value) => Ok(value),
                _ => Err(serde::de::Error::custom("list must contain only strings")),
            })
            .collect::<std::result::Result<Vec<_>, D::Error>>()
            .map(Some),
        Some(_) => Err(serde::de::Error::custom("setting must be a string list or null")),
    }
}

impl RuntimeSettings {
    /// Initial stage only: service fields/TUN, pure TUN derivation, then DNS page.
    /// Final enforcement must not rerun derivation after manual enhancements.
    pub fn prepare(&self, config: Mapping) -> Result<Mapping> {
        let mut initial = self.clone();
        initial.dns = None;
        initial.hosts = None;
        let mut config = initial.enforce(config)?;
        if let Some(enable) = self.tun.as_ref().and_then(|tun| tun.enable) {
            config = crate::enhance::tun::use_tun(config, enable);
        }
        config = self.enforce(config)?;
        if self.dns.is_some() {
            crate::enhance::tun::ensure_fake_ip_range6(&mut config);
        }
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            cfg!(not(target_os = "windows")) || self.redir_port.is_none(),
            "redir-port is unsupported on Windows"
        );
        ensure!(
            cfg!(target_os = "linux") || self.tproxy_port.is_none(),
            "tproxy-port is supported only on Linux"
        );
        ensure!(
            self.geo_update_interval.is_none_or(|hours| (1..=8760).contains(&hours)),
            "geo-update-interval must be 1–8760 hours"
        );
        ensure!(
            cfg!(target_os = "linux") || self.routing_mark.is_none(),
            "routing-mark is supported only on Linux"
        );
        if let Some(name) = &self.interface_name {
            ensure!(
                name.len() <= 15
                    && name != "."
                    && name != ".."
                    && !name
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | ':')),
                "interface-name must be empty or a valid Linux interface name of at most 15 UTF-8 bytes"
            );
        }
        if let Some(agent) = &self.global_ua {
            ensure!(
                agent.len() <= 1024 && agent.bytes().all(|b| (0x20..=0x7e).contains(&b)),
                "global-ua must contain at most 1024 printable ASCII bytes"
            );
        }
        if let Some(urls) = &self.geox_url {
            urls.validate()?;
        }
        if let Some(hosts) = &self.hosts {
            hosts.validate()?;
        }
        if let Some(tun) = &self.tun {
            tun.validate()?;
        }
        if let Some(dns) = &self.dns {
            dns.validate()?;
        }
        Ok(())
    }

    pub fn enforce(&self, config: Mapping) -> Result<Mapping> {
        self.validate()?;
        let mut config = super::runtime::generate(config, &Mapping::new())?;
        let mut values = self.owned_fields()?;
        // Nested network/Geo ownership is per key, preserving subscription fields.
        for section in ["dns", "tun", "geox-url"] {
            if let Some(value) = values.remove(section) {
                let mut nested = config
                    .remove(section)
                    .and_then(|v| v.as_mapping().cloned())
                    .unwrap_or_default();
                let mut owned = value.as_mapping().context("invalid nested settings mapping")?.clone();
                if section == "dns"
                    && let Some(filter) = owned.remove("fallback-filter")
                {
                    let mut merged = nested
                        .remove("fallback-filter")
                        .and_then(|value| value.as_mapping().cloned())
                        .unwrap_or_default();
                    merged.extend(filter.as_mapping().context("invalid fallback filter mapping")?.clone());
                    nested.insert("fallback-filter".into(), merged.into());
                }
                nested.extend(owned);
                config.insert(section.into(), nested.into());
            }
        }
        config.extend(values);
        Ok(config)
    }

    /// DNS host-use flags own both booleans; other page fields follow upstream is_set.
    /// TUN false and empty lists are explicit values; absent fields inherit.
    fn owned_fields(&self) -> Result<Mapping> {
        let mut values = serde_yaml_ng::to_value(self)?
            .as_mapping()
            .context("invalid runtime settings mapping")?
            .clone();
        for section in ["dns", "tun", "geox-url"] {
            if let Some(nested) = values.get_mut(section).and_then(|v| v.as_mapping_mut()) {
                if section == "dns" {
                    if let Some(filter) = nested
                        .get_mut("fallback-filter")
                        .and_then(|value| value.as_mapping_mut())
                    {
                        filter.retain(|_, value| match value {
                            serde_yaml_ng::Value::Null => false,
                            serde_yaml_ng::Value::String(value) => !value.trim().is_empty(),
                            serde_yaml_ng::Value::Sequence(value) => !value.is_empty(),
                            _ => true,
                        });
                    }
                    nested.retain(|key, value| match value {
                        serde_yaml_ng::Value::Null => false,
                        serde_yaml_ng::Value::Bool(on) => {
                            *on || matches!(key.as_str(), Some("use-hosts" | "use-system-hosts"))
                        }
                        serde_yaml_ng::Value::String(s) => !s.trim().is_empty(),
                        serde_yaml_ng::Value::Sequence(s) => !s.is_empty(),
                        serde_yaml_ng::Value::Mapping(m) => !m.is_empty(),
                        _ => true,
                    });
                }
                if nested.is_empty() {
                    values.remove(section);
                }
            }
        }
        Ok(values)
    }

    /// Report changed owned leaves rather than attributing an entire nested map.
    pub fn overridden_fields(&self, before: &Mapping, after: &Mapping) -> Result<Vec<String>> {
        let mut changed = Vec::new();
        for (key, value) in self.owned_fields()? {
            let name = key.as_str().context("invalid settings key")?;
            if name == "hosts" {
                if before.get(&key) != after.get(&key) {
                    changed.push(name.to_owned());
                }
            } else if let Some(nested) = value.as_mapping() {
                for (subkey, owned) in nested {
                    if name == "dns" && subkey.as_str() == Some("fallback-filter") {
                        let before_filter = before.get(&key).and_then(|value| value.get(subkey));
                        let after_filter = after.get(&key).and_then(|value| value.get(subkey));
                        for field in owned.as_mapping().context("invalid fallback filter mapping")?.keys() {
                            if before_filter.and_then(|value| value.get(field))
                                != after_filter.and_then(|value| value.get(field))
                            {
                                changed.push(format!(
                                    "dns.fallback-filter.{}",
                                    field.as_str().context("invalid fallback filter key")?
                                ));
                            }
                        }
                        continue;
                    }
                    let get = |config: &Mapping| {
                        config
                            .get(&key)
                            .and_then(|v| v.as_mapping())
                            .and_then(|m| m.get(subkey))
                            .cloned()
                    };
                    if get(before) != get(after) {
                        changed.push(format!(
                            "{name}.{}",
                            subkey.as_str().context("invalid nested settings key")?
                        ));
                    }
                }
            } else if before.get(&key) != after.get(&key) {
                changed.push(name.to_owned());
            }
        }
        Ok(changed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceSettings {
    pub schema_version: u32,
    #[serde(default)]
    pub runtime: RuntimeSettings,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profile_dns: BTreeMap<String, super::dns::ProfileDnsSettings>,
}

impl Default for ServiceSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            runtime: RuntimeSettings::default(),
            profile_dns: BTreeMap::new(),
        }
    }
}

impl ServiceSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "unsupported settings schema version");
        ensure!(
            self.profile_dns
                .keys()
                .all(|uid| !uid.trim().is_empty() && uid.len() <= 256 && !uid.chars().any(char::is_control)),
            "invalid profile DNS UID"
        );
        self.runtime.validate()?;
        ensure!(
            serde_yaml_ng::to_string(self)?.len() <= MAX_SETTINGS_BYTES,
            "settings exceed 64 KiB"
        );
        Ok(())
    }
}

pub struct SettingsStore {
    root: PathBuf,
    settings: ServiceSettings,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsJournal {
    schema_version: u32,
    previous: ServiceSettings,
    candidate: ServiceSettings,
    runtime_revision: Revision,
}

impl SettingsStore {
    #[cfg(unix)]
    pub(crate) fn data_dir(&self) -> &Path {
        &self.root
    }

    pub fn open(data_dir: &Path) -> Result<Self> {
        fs::create_dir_all(data_dir)?;
        let path = data_dir.join("settings.yaml");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let settings = if let Some(metadata) = &metadata {
            ensure!(
                metadata.is_file() && metadata.len() <= MAX_SETTINGS_BYTES as u64,
                "unsafe or oversized settings file"
            );
            serde_yaml_ng::from_str::<ServiceSettings>(&fs::read_to_string(&path)?).context("invalid settings.yaml")?
        } else {
            ServiceSettings::default()
        };
        settings.validate()?;
        let mut store = Self {
            root: data_dir.to_owned(),
            settings,
        };
        if metadata.is_none() {
            store.replace(store.snapshot())?;
        }
        Ok(store)
    }

    pub fn snapshot(&self) -> ServiceSettings {
        self.settings.clone()
    }

    /// Catalog-committed deletion does not change runtime settings or its revision.
    pub fn remove_profile_dns(&mut self, uid: &str) -> Result<()> {
        ensure!(
            !self.root.join("settings-transaction.yaml").try_exists()?,
            "settings recovery is pending"
        );
        let mut candidate = self.snapshot();
        if candidate.profile_dns.remove(uid).is_some() {
            self.replace(candidate)?;
        }
        Ok(())
    }

    /// Remove preferences left by deletions predating coordinated cleanup.
    pub fn prune_profile_dns(&mut self, profiles: &super::IProfiles) -> Result<()> {
        ensure!(
            !self.root.join("settings-transaction.yaml").try_exists()?,
            "settings recovery is pending"
        );
        let mut candidate = self.snapshot();
        candidate.profile_dns.retain(|uid, _| {
            profiles.items.iter().flatten().any(|item| {
                item.uid.as_deref() == Some(uid.as_str()) && matches!(item.itype.as_deref(), Some("local" | "remote"))
            })
        });
        if candidate != self.settings {
            self.replace(candidate)?;
        }
        Ok(())
    }

    pub fn begin(&self, candidate: ServiceSettings, runtime_revision: Revision) -> Result<()> {
        ensure!(
            !self.root.join("backup-restore.yaml").try_exists()?,
            "backup restore recovery is pending"
        );
        candidate.validate()?;
        ensure!(
            !self.root.join("settings-transaction.yaml").try_exists()?,
            "settings transaction already pending"
        );
        let journal = SettingsJournal {
            schema_version: 1,
            previous: self.snapshot(),
            candidate,
            runtime_revision,
        };
        let yaml = serde_yaml_ng::to_string(&journal)?;
        self.write_atomic("settings-transaction.yaml", yaml.as_bytes())
    }

    pub fn publish(&mut self) -> Result<()> {
        let journal = self.journal()?.context("no pending settings transaction")?;
        self.check_snapshot(&journal)?;
        self.replace(journal.candidate)
    }

    /// Runtime manifest commit decides both settings publication and rollback.
    pub fn recover(&mut self, committed: Option<&Revision>) -> Result<()> {
        let Some(journal) = self.journal()? else {
            return Ok(());
        };
        self.settings = self.check_snapshot(&journal)?;
        let desired = if committed == Some(&journal.runtime_revision) {
            journal.candidate
        } else {
            journal.previous
        };
        if self.settings != desired {
            self.replace(desired)?;
        }
        fs::remove_file(self.root.join("settings-transaction.yaml"))?;
        sync_directory(&self.root)
    }

    fn check_snapshot(&self, journal: &SettingsJournal) -> Result<ServiceSettings> {
        let path = self.root.join("settings.yaml");
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_SETTINGS_BYTES as u64,
            "unsafe or oversized settings file during recovery"
        );
        let saved: ServiceSettings = serde_yaml_ng::from_str(&fs::read_to_string(path)?)?;
        saved.validate()?;
        ensure!(
            saved == journal.previous || saved == journal.candidate,
            "settings journal conflicts with saved settings"
        );
        Ok(saved)
    }

    fn journal(&self) -> Result<Option<SettingsJournal>> {
        let path = self.root.join("settings-transaction.yaml");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.is_file() && metadata.len() <= (2 * MAX_SETTINGS_BYTES + 4096) as u64,
            "unsafe or oversized settings journal"
        );
        let journal: SettingsJournal = serde_yaml_ng::from_str(&fs::read_to_string(path)?)?;
        ensure!(journal.schema_version == 1, "unsupported settings journal version");
        journal.previous.validate()?;
        journal.candidate.validate()?;
        Ok(Some(journal))
    }

    fn write_atomic(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let temporary = self.root.join(format!("settings-{}.tmp", unique_id()?));
        let result = (|| {
            write_new(&temporary, bytes)?;
            fs::rename(&temporary, self.root.join(name))?;
            sync_directory(&self.root)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    /// Atomic file replacement, not a running-core transaction. Online updates must
    /// coordinate this commit with runtime application in the lifecycle actor.
    pub fn replace(&mut self, settings: ServiceSettings) -> Result<()> {
        settings.validate()?;
        let yaml = serde_yaml_ng::to_string(&settings)?;
        ensure!(yaml.len() <= MAX_SETTINGS_BYTES, "settings exceed 64 KiB");
        let temporary = self.root.join(format!("settings-{}.tmp", unique_id()?));
        let result = (|| {
            write_new(&temporary, yaml.as_bytes())?;
            fs::rename(&temporary, self.root.join("settings.yaml"))?;
            self.settings = settings;
            sync_directory(&self.root)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}
