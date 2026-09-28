//! Provider path authority for a service-owned Mihomo data directory.
//! Conflict allocation adapts upstream core/runtime_bundle.rs; source YAML is unchanged.
use anyhow::{Result, ensure};
use ring::digest::{Context, SHA256};
use serde_yaml_ng::{Mapping, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Component, Path, PathBuf},
};

pub const GEO_ASSETS: &[&str] = &[
    "Country.mmdb",
    "ASN.mmdb",
    "geoip.dat",
    "geosite.dat",
    "geoip.metadb",
    "GeoSite.dat",
];

/// Check if a filename matches any recognized Geo asset (case-insensitively).
pub fn is_geo_asset(name: &str) -> bool {
    GEO_ASSETS.iter().any(|asset| asset.eq_ignore_ascii_case(name))
}
pub const MAX_PROVIDERS: usize = 512;
/// Reserved for source-addressed HTTP caches; never used by local providers.
pub const HTTP_CACHE_ROOT: &str = "provider-cache";

/// Service policy beyond upstream's simultaneous path-conflict allocation.
/// Ownership is encoded in immutable source-addressed names, so no mutable ledger
/// or cache rollback is needed when a probe downloads before runtime publication.
pub fn prepare_owned(config: Mapping, root: &Path, protected: &[PathBuf]) -> Result<Mapping> {
    let mut config = prepare(config, root, protected)?;
    owned_paths(&mut config, root, protected, true)?;
    Ok(config)
}

/// Legacy HTTP revisions must be reapplied explicitly; never rewrite a commit on start.
pub fn validate_owned(config: &Mapping, root: &Path, protected: &[PathBuf]) -> Result<()> {
    validate(config, root, protected)?;
    owned_paths(&mut config.clone(), root, protected, false)
}

fn owned_paths(config: &mut Mapping, root: &Path, protected: &[PathBuf], rewrite: bool) -> Result<()> {
    for section in ["proxy-providers", "rule-providers"] {
        let Some(providers) = config.get_mut(section).and_then(Value::as_mapping_mut) else {
            continue;
        };
        for provider in providers.values_mut() {
            let remote = provider.get("type").and_then(Value::as_str) == Some("http");
            let inline = provider.get("type").and_then(Value::as_str) == Some("inline");
            if !remote {
                if !inline {
                    ensure!(
                        !provider
                            .get("path")
                            .and_then(Value::as_str)
                            .and_then(|raw| relative_path(root, raw))
                            .is_some_and(|path| path.starts_with(HTTP_CACHE_ROOT)),
                        "local provider cannot use the service HTTP cache namespace"
                    );
                }
                continue;
            }
            let destination = owned_destination(section, provider)?;
            if !rewrite {
                ensure!(
                    provider.get("path").and_then(Value::as_str) == Some(destination.as_str()),
                    "HTTP provider cache ownership changed or is legacy; reapply the configuration"
                );
            }
            check_destination(root, Path::new(&destination), protected)?;
            check_file(root, Path::new(&destination), true)?;
            if rewrite {
                provider
                    .as_mapping_mut()
                    .ok_or_else(|| anyhow::anyhow!("provider must be a mapping"))?
                    .insert("path".into(), destination.into());
            }
        }
    }
    Ok(())
}

fn owned_destination(section: &str, provider: &Value) -> Result<String> {
    let url = provider
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty() && url.len() <= 8192)
        .ok_or_else(|| anyhow::anyhow!("HTTP provider requires a URL of at most 8192 bytes"))?;
    // These affect retrieval/content interpretation. Names, interval, health
    // checks, filters and overrides do not identify the downloaded raw file.
    let mut identity = BTreeMap::new();
    identity.insert("section", serde_json::Value::String(section.into()));
    identity.insert("url", serde_json::Value::String(url.into()));
    for key in ["header", "proxy", "format", "behavior"] {
        let value = provider.get(key).unwrap_or(&Value::Null);
        identity.insert(
            key,
            serde_json::to_value(value).map_err(|_| anyhow::anyhow!("invalid HTTP provider source identity"))?,
        );
    }
    // JSON object keys sort without depending on YAML declaration order.
    let bytes = serde_json::to_vec(&identity)?;
    let mut hash = Context::new(&SHA256);
    hash.update(b"mihomo-server-http-cache-v1\0");
    hash.update(&bytes);
    let hash: String = hash
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(format!("{HTTP_CACHE_ROOT}/v1/{hash}.cache"))
}

#[derive(Default)]
struct Owners {
    local: bool,
    remote: BTreeMap<String, Vec<(&'static str, Value)>>,
}

/// Normalize explicit paths and allocate every HTTP source in a conflicting group.
/// Protected paths include caller-owned files/directories inside the data root.
pub fn prepare(config: Mapping, root: &Path, protected: &[PathBuf]) -> Result<Mapping> {
    process(config, root, protected, true)
}

/// Check again before a probe/start, without rewriting an immutable committed revision.
/// Legacy normalized aliases remain valid; unresolved different-source conflicts fail.
pub fn validate(config: &Mapping, root: &Path, protected: &[PathBuf]) -> Result<()> {
    process(config.clone(), root, protected, false).map(|_| ())
}

fn process(mut config: Mapping, root: &Path, protected: &[PathBuf], allocate: bool) -> Result<Mapping> {
    let mut paths = BTreeMap::<String, Owners>::new();
    let mut reserved: HashSet<String> = GEO_ASSETS.iter().map(|name| (*name).into()).collect();
    let mut count = 0;
    for section in ["proxy-providers", "rule-providers"] {
        let Some(declarations) = config.get_mut(section) else {
            continue;
        };
        let declarations = declarations
            .as_mapping_mut()
            .ok_or_else(|| anyhow::anyhow!("provider section must be a mapping"))?;
        count += declarations.len();
        ensure!(count <= MAX_PROVIDERS, "resource configuration exceeds 512 providers");
        for (name, provider) in declarations {
            ensure!(
                name.as_str().is_some_and(|name| name.len() <= 512),
                "invalid provider name"
            );
            let Some(raw_path) = provider.get("path") else { continue };
            // Inline providers do not read/write a cache file.
            if provider.get("type").and_then(Value::as_str) == Some("inline") {
                continue;
            }
            let raw_path = raw_path
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("provider path must be a string"))?;
            let relative = relative_path(root, raw_path)
                .ok_or_else(|| anyhow::anyhow!("provider path must stay inside the Mihomo data directory"))?;
            check_destination(root, &relative, protected)?;
            let remote = provider.get("type").and_then(Value::as_str) == Some("http");
            check_file(root, &relative, remote)?;
            let destination = relative.to_string_lossy().into_owned();
            reserved.insert(destination.clone());
            let owners = paths.entry(destination.clone()).or_default();
            match (remote, provider.get("url").and_then(Value::as_str)) {
                (true, Some(url)) => owners
                    .remote
                    .entry(url.into())
                    .or_default()
                    .push((section, name.clone())),
                _ => owners.local = true,
            }
            provider
                .as_mapping_mut()
                .ok_or_else(|| anyhow::anyhow!("provider must be a mapping"))?
                .insert(Value::String("path".into()), Value::String(destination));
        }
    }
    for (destination, owners) in paths {
        ensure!(
            !owners.local || owners.remote.is_empty(),
            "HTTP provider cache conflicts with a local resource"
        );
        if owners.remote.len() < 2 {
            continue;
        }
        ensure!(
            allocate,
            "provider cache has multiple HTTP sources; reapply the configuration"
        );
        // Sorted destinations/URLs ensure declaration order never chooses a cache owner.
        for (url, providers) in owners.remote {
            let replacement = allocate_destination(&destination, &url, &mut reserved);
            let relative = Path::new(&replacement);
            check_destination(root, relative, protected)?;
            check_file(root, relative, true)?;
            for (section, name) in providers {
                config[section][&name]["path"] = Value::String(replacement.clone());
            }
        }
    }
    Ok(config)
}

fn allocate_destination(destination: &str, url: &str, reserved: &mut HashSet<String>) -> String {
    let mut hash = Context::new(&SHA256);
    hash.update(destination.as_bytes());
    hash.update(&[0]);
    hash.update(url.as_bytes());
    let hash: String = hash
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let path = Path::new(destination);
    let extension = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    let mut sequence = 0;
    loop {
        let suffix = if sequence == 0 {
            String::new()
        } else {
            format!("-{sequence}")
        };
        let candidate = path
            .with_file_name(format!("cvr-{hash}{suffix}{extension}"))
            .to_string_lossy()
            .into_owned();
        if reserved.insert(candidate.clone()) {
            return candidate;
        }
        sequence += 1;
    }
}

fn check_destination(root: &Path, path: &Path, protected: &[PathBuf]) -> Result<()> {
    let first = path.components().next().unwrap().as_os_str().to_string_lossy();
    ensure!(
        ![
            "config",
            "profiles",
            "core",
            "run",
            "backups",
            "restore-candidates",
            "cache.db",
            "management-token",
            "settings.yaml",
            "profiles.yaml",
            "backup-restore.yaml",
            "settings-transaction.yaml"
        ]
        .contains(&first.as_ref())
            && !first.starts_with("profile-")
            && !is_geo_asset(&first)
            && !path
                .components()
                .any(|part| part.as_os_str().to_string_lossy().starts_with('.')),
        "provider path targets a service-owned or Geo resource"
    );
    ensure!(
        !protected.iter().any(|file| root.join(path).starts_with(file)),
        "provider path targets a protected service resource"
    );
    Ok(())
}

fn check_file(root: &Path, path: &Path, remote: bool) -> Result<()> {
    match metadata_below(root, path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "provider path must be a regular file without symlinks"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt as _;
                ensure!(
                    !remote || metadata.nlink() == 1,
                    "HTTP provider cache must not be a hard-linked file"
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {} // The core decides whether a missing local file is valid.
        Err(_) => anyhow::bail!("provider path cannot be inspected without following links"),
    }
    Ok(())
}

pub fn relative_path(root: &Path, raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(|c| c.is_control() || c == '\\' || c == ':') {
        return None;
    }
    let path = Path::new(raw);
    let relative = if path.is_absolute() {
        path.strip_prefix(root).ok()?
    } else {
        path
    };
    let mut result = PathBuf::new();
    let mut count = 0;
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                result.push(part);
                count += 1;
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (count > 0 && count <= 64).then_some(result)
}

/// Metadata walk pinned to descriptors; never opens provider contents or follows links.
#[cfg(target_os = "linux")]
pub fn metadata_below(root: &Path, relative: &Path) -> std::io::Result<std::fs::Metadata> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() || components.iter().any(|part| !matches!(part, Component::Normal(_))) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "metadata path must be relative and normalized",
        ));
    }
    let mut directory = std::fs::File::open(root)?;
    for (index, component) in components.iter().enumerate() {
        let name = CString::new(component.as_os_str().as_bytes())?;
        let flags = libc::O_PATH
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if index + 1 < components.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        directory = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    directory.metadata()
}

#[cfg(not(target_os = "linux"))]
pub fn metadata_below(_root: &Path, _relative: &Path) -> std::io::Result<std::fs::Metadata> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "resource path validation requires Linux",
    ))
}
