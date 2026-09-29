//! Metadata-only inventory of resources used by the committed runtime.
//! Provider URLs, headers, inline content and file contents never enter the report.
use anyhow::{Result, ensure};
use mihomo_client::models::GeoConfig;
use serde_yaml_ng::{Mapping, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use headless_core::config::resource_paths::{metadata_below, relative_path};
pub use headless_core::config::resources::{
    AutoUpdateState, FileState, FreshnessState, GEO_ASSETS, GeoUpdatePolicy, HTTP_CACHE_ROOT, Inventory, MAX_PROVIDERS,
    ProviderSettings, Resource, is_geo_asset, validate_resource_declarations,
};

pub trait GeoUpdatePolicyFromCore {
    fn from_config(config: &Mapping, core_running: bool, actual: Option<&GeoConfig>) -> Self;
}

impl GeoUpdatePolicyFromCore for GeoUpdatePolicy {
    fn from_config(config: &Mapping, core_running: bool, actual: Option<&GeoConfig>) -> Self {
        geo_update_policy_from_config(config, core_running, actual)
    }
}

pub fn geo_update_policy_from_config(
    config: &Mapping,
    core_running: bool,
    actual: Option<&GeoConfig>,
) -> GeoUpdatePolicy {
    let configured_enabled = config.get("geo-auto-update").and_then(Value::as_bool);
    let configured_interval_hours = config.get("geo-update-interval").and_then(Value::as_i64);
    let effective_enabled = actual.and_then(|core| core.geo_auto_update);
    let effective_interval_hours = actual.and_then(|core| core.geo_update_interval);
    let readback_error = core_running && actual.is_none();
    GeoUpdatePolicy::evaluate(
        configured_enabled,
        configured_interval_hours,
        effective_enabled,
        effective_interval_hours,
        core_running,
        readback_error,
    )
}

pub(crate) fn inspect(
    data_dir: PathBuf,
    bundle_dir: Option<PathBuf>,
    config_revision: Option<String>,
    config: Mapping,
    geo_update: GeoUpdatePolicy,
) -> Result<Inventory> {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    inspect_with_now(data_dir, bundle_dir, config_revision, config, geo_update, now)
}

pub(crate) fn inspect_with_now(
    data_dir: PathBuf,
    bundle_dir: Option<PathBuf>,
    config_revision: Option<String>,
    config: Mapping,
    geo_update: GeoUpdatePolicy,
    now_unix_seconds: Option<u64>,
) -> Result<Inventory> {
    let mut providers = Vec::new();
    for section in ["proxy-providers", "rule-providers"] {
        let Some(declarations) = config.get(section) else {
            continue;
        };
        let declarations = declarations
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("invalid provider section"))?;
        ensure!(
            providers.len() + declarations.len() <= MAX_PROVIDERS,
            "resource inventory supports at most 512 providers"
        );
        for (name, value) in declarations {
            let name = name.as_str().ok_or_else(|| anyhow::anyhow!("invalid provider name"))?;
            ensure!(name.len() <= 512, "provider name exceeds inventory limit");
            let kind = value.get("type").and_then(Value::as_str);
            let interval_seconds = value.get("interval").and_then(Value::as_u64).filter(|&s| s > 0);
            let mut resource = Resource {
                section: section.into(),
                name: name.into(),
                provider_type: kind
                    .filter(|kind| ["http", "file", "inline"].contains(kind))
                    .map(str::to_owned),
                path: None,
                state: FileState::InvalidDeclaration,
                bytes: None,
                modified_unix_seconds: None,
                age_seconds: None,
                freshness: FreshnessState::Indeterminate,
                conflict: false,
            };
            if value.as_mapping().is_some() {
                match kind {
                    Some("inline") => resource.state = FileState::Inline,
                    Some("http" | "file") => match value.get("path") {
                        Some(Value::String(path)) => {
                            inspect_path(&data_dir, path, &mut resource, interval_seconds, now_unix_seconds)
                        }
                        None if kind == Some("http") => resource.state = FileState::CoreManaged,
                        _ => {}
                    },
                    _ => {}
                }
            }
            providers.push(resource);
        }
    }
    let geo_interval_seconds = geo_update
        .effective_interval_hours
        .or(geo_update.configured_interval_hours)
        .filter(|&h| h > 0)
        .map(|h| (h as u64) * 3600)
        .or_else(|| {
            if geo_update.auto_update_state == AutoUpdateState::Active {
                Some(24 * 3600)
            } else {
                None
            }
        });
    let mut geo: Vec<_> = GEO_ASSETS
        .iter()
        .map(|name| {
            let mut resource = Resource {
                section: "geo".into(),
                name: (*name).into(),
                provider_type: None,
                path: None,
                state: FileState::Missing,
                bytes: None,
                modified_unix_seconds: None,
                age_seconds: None,
                freshness: FreshnessState::Indeterminate,
                conflict: false,
            };
            inspect_path(&data_dir, name, &mut resource, geo_interval_seconds, now_unix_seconds);
            resource
        })
        .collect();
    let mut owners = HashMap::<String, usize>::new();
    for resource in geo.iter().chain(&providers) {
        if let Some(path) = &resource.path {
            *owners.entry(path.clone()).or_default() += 1;
        }
    }
    for resource in geo.iter_mut().chain(&mut providers) {
        resource.conflict = resource.path.as_ref().is_some_and(|path| owners[path] > 1);
    }
    providers.sort_by(|a, b| (&a.section, &a.name).cmp(&(&b.section, &b.name)));
    Ok(Inventory {
        data_dir,
        bundle_dir,
        config_revision,
        geo_update,
        geo,
        providers,
    })
}

fn inspect_path(
    root: &Path,
    raw: &str,
    resource: &mut Resource,
    interval_seconds: Option<u64>,
    now_unix_seconds: Option<u64>,
) {
    let Some(relative) = relative_path(root, raw) else {
        resource.state = FileState::UnsafePath;
        return;
    };
    resource.path = Some(relative.to_string_lossy().into_owned());
    match metadata_below(root, &relative) {
        Ok(metadata) if metadata.file_type().is_symlink() => resource.state = FileState::UnsafePath,
        Ok(metadata) if !metadata.is_file() => resource.state = FileState::NotFile,
        Ok(metadata) => {
            resource.bytes = Some(metadata.len());
            let mtime_sec = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs());
            resource.modified_unix_seconds = mtime_sec;
            if metadata.len() == 0 {
                resource.state = FileState::Empty;
            } else {
                resource.state = FileState::Available;
                if let (Some(mtime), Some(now)) = (mtime_sec, now_unix_seconds) {
                    if mtime > now + 60 {
                        resource.age_seconds = None;
                        resource.freshness = FreshnessState::Indeterminate;
                    } else {
                        let age = now.saturating_sub(mtime);
                        resource.age_seconds = Some(age);
                        if let Some(interval) = interval_seconds
                            && interval > 0
                        {
                            if age <= interval {
                                resource.freshness = FreshnessState::Fresh;
                            } else {
                                resource.freshness = FreshnessState::Stale;
                            }
                        }
                    }
                }
            }
        }
        Err(error) => {
            resource.state = match error.kind() {
                std::io::ErrorKind::NotFound => FileState::Missing,
                std::io::ErrorKind::NotADirectory => FileState::UnsafePath,
                _ => FileState::Unreadable,
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "ms-inventory-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
        fn inspect(&self, yaml: &str) -> Result<Inventory> {
            let config = serde_yaml_ng::from_str(yaml)?;
            let policy = GeoUpdatePolicy::from_config(&config, false, None);
            inspect(self.0.clone(), None, Some("committed.yaml".into()), config, policy)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn paths_are_confined_without_following_links_or_opening_special_files() -> Result<()> {
        let dir = Directory::new()?;
        fs::write(dir.0.join("Country.mmdb"), b"metadata only")?;
        fs::write(dir.0.join("ASN.mmdb"), [])?;
        fs::create_dir(dir.0.join("geosite.dat"))?;
        symlink("/etc/passwd", dir.0.join("geoip.dat"))?;
        symlink("/etc", dir.0.join("outside"))?;
        let pipe = std::ffi::CString::new(dir.0.join("pipe").as_os_str().as_encoded_bytes())?;
        assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
        let inventory = dir.inspect(&format!("proxy-providers:\n  absolute: {{type: file, path: {}/Country.mmdb}}\n  external: {{type: file, path: /etc/passwd}}\n  link: {{type: file, path: outside/passwd}}\n  traversal: {{type: file, path: ../secret}}\n  fifo: {{type: file, path: pipe}}\n", dir.0.display()))?;
        assert_eq!(inventory.geo[0].state, FileState::Available);
        assert_eq!(inventory.geo[0].bytes, Some(13));
        assert!(inventory.geo[0].modified_unix_seconds.is_some());
        assert_eq!(inventory.geo[1].state, FileState::Empty);
        assert!(inventory.geo[1].modified_unix_seconds.is_some());
        assert_eq!(inventory.geo[2].state, FileState::UnsafePath);
        assert!(inventory.geo[2].modified_unix_seconds.is_none());
        assert_eq!(inventory.geo[3].state, FileState::NotFile);
        assert!(inventory.geo[3].modified_unix_seconds.is_none());
        assert_eq!(inventory.geo[4].state, FileState::Missing);
        assert!(inventory.geo[4].modified_unix_seconds.is_none());
        let state = |name| &inventory.providers.iter().find(|p| p.name == name).unwrap().state;
        assert_eq!(state("absolute"), &FileState::Available);
        assert_eq!(state("external"), &FileState::UnsafePath);
        assert_eq!(state("link"), &FileState::UnsafePath);
        assert_eq!(state("traversal"), &FileState::UnsafePath);
        assert_eq!(state("fifo"), &FileState::NotFile);
        assert!(inventory.geo[0].conflict);
        assert!(
            inventory
                .providers
                .iter()
                .find(|p| p.name == "absolute")
                .unwrap()
                .conflict
        );
        Ok(())
    }

    #[test]
    fn missing_explicit_paths_are_distinct_from_inline_and_core_managed_caches() -> Result<()> {
        let dir = Directory::new()?;
        let inventory = dir.inspect("proxy-providers:\n  automatic: {type: http, url: 'https://secret.invalid'}\n  inline: {type: inline, payload: [{password: private}]}\n  local: {type: file, path: cache/missing}\n  invalid: {type: file}\n  unknown: {type: secret}\n  'null': null\n")?;
        let state = |name| &inventory.providers.iter().find(|p| p.name == name).unwrap().state;
        assert_eq!(state("automatic"), &FileState::CoreManaged);
        assert_eq!(state("inline"), &FileState::Inline);
        assert_eq!(state("local"), &FileState::Missing);
        for name in ["invalid", "unknown", "null"] {
            assert_eq!(state(name), &FileState::InvalidDeclaration);
        }
        let encoded = serde_json::to_string(&inventory)?;
        for secret in ["secret.invalid", "password", "private", "payload"] {
            assert!(!encoded.contains(secret));
        }
        Ok(())
    }

    #[test]
    fn malformed_sections_and_oversized_inventories_fail_without_partial_reports() -> Result<()> {
        let dir = Directory::new()?;
        assert!(dir.inspect("proxy-providers: []").is_err());
        assert!(dir.inspect("proxy-providers: {1: {type: file}}").is_err());
        let yaml = format!(
            "proxy-providers:\n{}",
            (0..513)
                .map(|n| format!("  p{n}: {{type: inline}}\n"))
                .collect::<String>()
        );
        assert!(dir.inspect(&yaml).is_err());
        let yaml = format!("proxy-providers:\n  {}: {{type: inline}}", "x".repeat(513));
        assert!(dir.inspect(&yaml).is_err());
        Ok(())
    }

    #[test]
    fn geo_update_policy_distinguishes_inherited_stopped_missing_core_and_explicit_disabled() -> Result<()> {
        let inherited: Mapping = serde_yaml_ng::from_str("mode: rule")?;
        let stopped = GeoUpdatePolicy::from_config(&inherited, false, None);
        assert!(!stopped.core_running && !stopped.readback_error);
        assert!(stopped.configured_enabled.is_none() && stopped.effective_enabled.is_none());
        assert_eq!(stopped.auto_update_state, AutoUpdateState::Stopped);

        let committed: Mapping = serde_yaml_ng::from_str("geo-auto-update: false\ngeo-update-interval: 48")?;
        let unreachable = GeoUpdatePolicy::from_config(&committed, true, None);
        assert!(unreachable.readback_error && unreachable.effective_enabled.is_none());
        assert_eq!(unreachable.auto_update_state, AutoUpdateState::Indeterminate);
        let old_core: GeoConfig = serde_json::from_value(serde_json::json!({}))?;
        let unknown = GeoUpdatePolicy::from_config(&committed, true, Some(&old_core));
        assert!(!unknown.readback_error && !unknown.mismatch);
        assert!(unknown.effective_enabled.is_none() && unknown.effective_interval_hours.is_none());
        assert_eq!(unknown.auto_update_state, AutoUpdateState::Disabled);

        let matching: GeoConfig = serde_json::from_value(serde_json::json!({
            "geo-auto-update": false, "geo-update-interval": 48
        }))?;
        let policy = GeoUpdatePolicy::from_config(&committed, true, Some(&matching));
        assert_eq!(policy.configured_enabled, Some(false));
        assert_eq!(policy.effective_enabled, Some(false));
        assert_eq!(policy.effective_interval_hours, Some(48));
        assert_eq!(policy.auto_update_state, AutoUpdateState::Disabled);
        assert!(!policy.mismatch);

        let drift: GeoConfig = serde_json::from_value(serde_json::json!({
            "geo-auto-update": true, "geo-update-interval": 24
        }))?;
        let drift_policy = GeoUpdatePolicy::from_config(&committed, true, Some(&drift));
        assert!(drift_policy.mismatch);
        assert_eq!(drift_policy.auto_update_state, AutoUpdateState::Active);

        let zero_interval: GeoConfig = serde_json::from_value(serde_json::json!({
            "geo-auto-update": true, "geo-update-interval": 0
        }))?;
        assert_eq!(
            GeoUpdatePolicy::from_config(&committed, true, Some(&zero_interval)).auto_update_state,
            AutoUpdateState::Disabled
        );
        Ok(())
    }

    #[test]
    fn resource_freshness_evaluates_fresh_stale_and_indeterminate_against_intervals() -> Result<()> {
        let dir = Directory::new()?;
        let now = 1_700_000_000_u64;

        fs::write(dir.0.join("Country.mmdb"), b"fresh mmdb content")?;
        fs::write(dir.0.join("geosite.dat"), b"stale dat content")?;
        fs::write(dir.0.join("geoip.dat"), b"future clock skew content")?;
        fs::write(dir.0.join("ASN.mmdb"), [])?;

        let set_mtime = |name: &str, sec: u64| -> Result<()> {
            let path = dir.0.join(name);
            let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
            let times = [
                libc::timespec {
                    tv_sec: sec as libc::time_t,
                    tv_nsec: 0,
                },
                libc::timespec {
                    tv_sec: sec as libc::time_t,
                    tv_nsec: 0,
                },
            ];
            let ret = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
            ensure!(ret == 0, "utimensat failed");
            Ok(())
        };

        set_mtime("Country.mmdb", now - 3600)?; // 1 hour ago
        set_mtime("geosite.dat", now - 100_000)?; // ~27.7 hours ago
        set_mtime("geoip.dat", now + 3600)?; // 1 hour in the future (clock skew)
        set_mtime("ASN.mmdb", now - 3600)?; // empty file

        let yaml = format!(
            "geo-auto-update: true\ngeo-update-interval: 24\nproxy-providers:\n  p_fresh: {{type: file, path: {}/Country.mmdb, interval: 7200}}\n  p_stale: {{type: file, path: {}/Country.mmdb, interval: 1800}}\n  p_no_interval: {{type: file, path: {}/Country.mmdb}}\n",
            dir.0.display(),
            dir.0.display(),
            dir.0.display()
        );
        let config: Mapping = serde_yaml_ng::from_str(&yaml)?;
        let policy = GeoUpdatePolicy::from_config(
            &config,
            true,
            Some(&GeoConfig {
                geo_auto_update: Some(true),
                geo_update_interval: Some(24),
                ..Default::default()
            }),
        );
        assert_eq!(policy.auto_update_state, AutoUpdateState::Active);

        let inventory = inspect_with_now(
            dir.0.clone(),
            None,
            Some("committed.yaml".into()),
            config,
            policy,
            Some(now),
        )?;

        let geo = |name: &str| inventory.geo.iter().find(|r| r.name == name).unwrap();
        let country = geo("Country.mmdb");
        assert_eq!(country.state, FileState::Available);
        assert_eq!(country.age_seconds, Some(3600));
        assert_eq!(country.freshness, FreshnessState::Fresh);

        let geosite = geo("geosite.dat");
        assert_eq!(geosite.state, FileState::Available);
        assert_eq!(geosite.age_seconds, Some(100_000));
        assert_eq!(geosite.freshness, FreshnessState::Stale);

        let geoip = geo("geoip.dat");
        assert_eq!(geoip.state, FileState::Available);
        assert_eq!(geoip.age_seconds, None);
        assert_eq!(geoip.freshness, FreshnessState::Indeterminate);

        let asn = geo("ASN.mmdb");
        assert_eq!(asn.state, FileState::Empty);
        assert_eq!(asn.age_seconds, None);
        assert_eq!(asn.freshness, FreshnessState::Indeterminate);

        let missing = geo("geoip.metadb");
        assert_eq!(missing.state, FileState::Missing);
        assert_eq!(missing.age_seconds, None);
        assert_eq!(missing.freshness, FreshnessState::Indeterminate);

        let provider = |name: &str| inventory.providers.iter().find(|r| r.name == name).unwrap();
        let p_fresh = provider("p_fresh");
        assert_eq!(p_fresh.state, FileState::Available);
        assert_eq!(p_fresh.age_seconds, Some(3600));
        assert_eq!(p_fresh.freshness, FreshnessState::Fresh);

        let p_stale = provider("p_stale");
        assert_eq!(p_stale.state, FileState::Available);
        assert_eq!(p_stale.age_seconds, Some(3600));
        assert_eq!(p_stale.freshness, FreshnessState::Stale);

        let p_none = provider("p_no_interval");
        assert_eq!(p_none.state, FileState::Available);
        assert_eq!(p_none.age_seconds, Some(3600));
        assert_eq!(p_none.freshness, FreshnessState::Indeterminate);

        Ok(())
    }
}
