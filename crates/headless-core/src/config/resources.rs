//! Typed resource inventory, provider settings, and lifecycle policy models.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};
use std::path::PathBuf;

pub use super::resource_paths::{GEO_ASSETS, HTTP_CACHE_ROOT, MAX_PROVIDERS, is_geo_asset};

/// Full resource inventory reported for the committed runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    pub data_dir: PathBuf,
    pub bundle_dir: Option<PathBuf>,
    pub config_revision: Option<String>,
    pub geo_update: GeoUpdatePolicy,
    pub geo: Vec<Resource>,
    pub providers: Vec<Resource>,
}

/// Committed policy and running core reported policy for Geo databases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeoUpdatePolicy {
    pub core_running: bool,
    pub readback_error: bool,
    pub configured_enabled: Option<bool>,
    pub configured_interval_hours: Option<i64>,
    pub effective_enabled: Option<bool>,
    pub effective_interval_hours: Option<i64>,
    pub auto_update_state: AutoUpdateState,
    pub mismatch: bool,
}

impl GeoUpdatePolicy {
    /// Evaluate the effective update policy from raw configuration and runtime values.
    pub fn evaluate(
        configured_enabled: Option<bool>,
        configured_interval_hours: Option<i64>,
        effective_enabled: Option<bool>,
        effective_interval_hours: Option<i64>,
        core_running: bool,
        readback_error: bool,
    ) -> Self {
        let mismatch = configured_enabled
            .zip(effective_enabled)
            .is_some_and(|(configured, effective)| configured != effective)
            || configured_interval_hours
                .zip(effective_interval_hours)
                .is_some_and(|(configured, effective)| configured != effective);

        let auto_update_state = if !core_running {
            AutoUpdateState::Stopped
        } else if readback_error {
            AutoUpdateState::Indeterminate
        } else if effective_enabled == Some(false) {
            AutoUpdateState::Disabled
        } else if effective_enabled == Some(true) {
            if effective_interval_hours.is_some_and(|h| h <= 0) {
                AutoUpdateState::Disabled
            } else {
                AutoUpdateState::Active
            }
        } else {
            match configured_enabled {
                Some(true) => {
                    if configured_interval_hours.is_some_and(|h| h <= 0) {
                        AutoUpdateState::Disabled
                    } else {
                        AutoUpdateState::Active
                    }
                }
                Some(false) | None => AutoUpdateState::Disabled,
            }
        };

        Self {
            core_running,
            readback_error,
            configured_enabled,
            configured_interval_hours,
            effective_enabled,
            effective_interval_hours,
            auto_update_state,
            mismatch,
        }
    }

    /// Extract configured values from configuration YAML and evaluate with core status.
    pub fn from_config_and_actual(
        config: &Mapping,
        core_running: bool,
        effective_enabled: Option<bool>,
        effective_interval_hours: Option<i64>,
        readback_error: bool,
    ) -> Self {
        let configured_enabled = config.get("geo-auto-update").and_then(Value::as_bool);
        let configured_interval_hours = config.get("geo-update-interval").and_then(Value::as_i64);
        Self::evaluate(
            configured_enabled,
            configured_interval_hours,
            effective_enabled,
            effective_interval_hours,
            core_running,
            readback_error,
        )
    }
}

/// Operational state of automatic resource updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoUpdateState {
    Active,
    Disabled,
    Stopped,
    Indeterminate,
}

/// Age-based freshness classification for an on-disk resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessState {
    Fresh,
    Stale,
    Indeterminate,
}

impl FreshnessState {
    pub fn evaluate_age(age_seconds: u64, interval_seconds: u64) -> Self {
        if interval_seconds > 0 && age_seconds <= interval_seconds {
            Self::Fresh
        } else if interval_seconds > 0 {
            Self::Stale
        } else {
            Self::Indeterminate
        }
    }
}

/// Verification and presence state of a resource on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    Available,
    Missing,
    Empty,
    UnsafePath,
    NotFile,
    Unreadable,
    Inline,
    CoreManaged,
    InvalidDeclaration,
}

/// Record describing a managed Geo database or proxy/rule provider file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    pub section: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    /// Normalized path relative to the Mihomo data directory; absent for unsafe paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub state: FileState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// Filesystem mtime in seconds since UNIX epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_unix_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<u64>,
    pub freshness: FreshnessState,
    pub conflict: bool,
}

/// Structured settings for an individual proxy or rule provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProviderSettings {
    #[serde(rename = "type")]
    pub provider_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
}

impl ProviderSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            ["http", "file", "inline"].contains(&self.provider_type.as_str()),
            "provider type must be 'http', 'file', or 'inline'"
        );
        if self.provider_type == "http" {
            let url = self
                .url
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("HTTP provider requires a URL"))?;
            ensure!(
                !url.is_empty() && url.len() <= 8192 && url.trim() == url,
                "HTTP provider URL must be 1–8192 non-whitespace bytes"
            );
            let parsed = url::Url::parse(url).map_err(|_| anyhow::anyhow!("invalid provider URL"))?;
            ensure!(
                matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
                "HTTP provider requires an HTTP(S) scheme and valid host"
            );
        }
        if let Some(path) = &self.path {
            ensure!(
                !path.is_empty() && path.len() <= 4096,
                "provider path must be 1–4096 bytes"
            );
        }
        if let Some(format) = &self.format {
            ensure!(
                ["yaml", "json", "text", "mrs"].contains(&format.as_str()),
                "unsupported provider format: {format}"
            );
        }
        if let Some(behavior) = &self.behavior {
            ensure!(
                ["classical", "domain", "ipcidr"].contains(&behavior.as_str()),
                "unsupported rule-provider behavior: {behavior}"
            );
        }
        if let Some(interval) = self.interval {
            ensure!(
                interval <= i32::MAX as u64,
                "provider interval exceeds maximum supported duration"
            );
        }
        Ok(())
    }
}

/// Validate provider declarations in a raw configuration mapping.
/// Enforces structural constraints: mapping section, maximum 512 providers,
/// valid provider name length (1–512 bytes), valid provider type, and bounded interval.
pub fn validate_resource_declarations(config: &Mapping) -> Result<()> {
    let mut count = 0;
    for section in ["proxy-providers", "rule-providers"] {
        let Some(declarations) = config.get(section) else {
            continue;
        };
        let declarations = declarations
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("{section} section must be a mapping"))?;
        count += declarations.len();
        ensure!(
            count <= MAX_PROVIDERS,
            "resource configuration exceeds {MAX_PROVIDERS} providers"
        );
        for (name, provider) in declarations {
            let name_str = name
                .as_str()
                .filter(|n| !n.is_empty() && n.len() <= 512 && !n.chars().any(char::is_control))
                .ok_or_else(|| anyhow::anyhow!("invalid provider name in {section}"))?;
            let Some(provider_map) = provider.as_mapping() else {
                anyhow::bail!("provider declaration '{name_str}' must be a mapping");
            };
            let kind = provider_map
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("provider '{name_str}' missing or invalid 'type'"))?;
            ensure!(
                ["http", "file", "inline"].contains(&kind),
                "unsupported provider type '{kind}' in provider '{name_str}'"
            );
            if let Some(interval) = provider_map.get("interval") {
                ensure!(
                    interval.as_i64().is_some_and(|i| i >= 0 && i <= i32::MAX as i64),
                    "provider '{name_str}' interval must be a non-negative integer up to 2^31-1"
                );
            }
            if let Some(format) = provider_map.get("format").and_then(Value::as_str) {
                ensure!(
                    ["yaml", "json", "text", "mrs"].contains(&format),
                    "unsupported format '{format}' in provider '{name_str}'"
                );
            }
            if let Some(behavior) = provider_map.get("behavior").and_then(Value::as_str) {
                ensure!(
                    ["classical", "domain", "ipcidr"].contains(&behavior),
                    "unsupported behavior '{behavior}' in provider '{name_str}'"
                );
            }
        }
    }
    Ok(())
}
