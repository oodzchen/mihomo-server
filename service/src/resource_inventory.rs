//! Metadata-only inventory of resources used by the committed runtime.
//! Provider URLs, headers, inline content and file contents never enter the report.
use anyhow::{Result, ensure};
use serde::Serialize;
use serde_yaml_ng::{Mapping, Value};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

// Same discovery names as upstream core/runtime_bundle.rs (Linux is case-sensitive).
pub const GEO_ASSETS: &[&str] = &[
    "Country.mmdb",
    "ASN.mmdb",
    "geoip.dat",
    "geosite.dat",
    "geoip.metadb",
    "GeoSite.dat",
];
const MAX_PROVIDERS: usize = 512;

#[derive(Debug, Serialize)]
pub struct Inventory {
    pub data_dir: PathBuf,
    pub bundle_dir: Option<PathBuf>,
    pub config_revision: Option<String>,
    pub geo: Vec<Resource>,
    pub providers: Vec<Resource>,
}

#[derive(Debug, Serialize)]
pub struct Resource {
    pub section: String,
    pub name: String,
    pub provider_type: Option<String>,
    /// Normalized path relative to the Mihomo data directory; absent for unsafe paths.
    pub path: Option<String>,
    pub state: FileState,
    pub bytes: Option<u64>,
    pub conflict: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
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

pub(crate) fn inspect(
    data_dir: PathBuf,
    bundle_dir: Option<PathBuf>,
    config_revision: Option<String>,
    config: Mapping,
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
            let mut resource = Resource {
                section: section.into(),
                name: name.into(),
                provider_type: kind
                    .filter(|kind| ["http", "file", "inline"].contains(kind))
                    .map(str::to_owned),
                path: None,
                state: FileState::InvalidDeclaration,
                bytes: None,
                conflict: false,
            };
            if value.as_mapping().is_some() {
                match kind {
                    Some("inline") => resource.state = FileState::Inline,
                    Some("http" | "file") => match value.get("path") {
                        Some(Value::String(path)) => inspect_path(&data_dir, path, &mut resource),
                        None if kind == Some("http") => resource.state = FileState::CoreManaged,
                        _ => {}
                    },
                    _ => {}
                }
            }
            providers.push(resource);
        }
    }
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
                conflict: false,
            };
            inspect_path(&data_dir, name, &mut resource);
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
        geo,
        providers,
    })
}

fn inspect_path(root: &Path, raw: &str, resource: &mut Resource) {
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
            resource.state = if metadata.len() == 0 {
                FileState::Empty
            } else {
                FileState::Available
            };
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

fn relative_path(root: &Path, raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || raw.len() > 4096 || raw.contains('\0') {
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

/// Walk pinned directory descriptors without following links, including parent links.
/// O_PATH permits metadata inspection without reading files, blocking on FIFOs or
/// triggering downloads. A swapped directory cannot redirect this walk outside root.
#[cfg(target_os = "linux")]
fn metadata_below(root: &Path, relative: &Path) -> std::io::Result<std::fs::Metadata> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    let components: Vec<_> = relative.components().collect();
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
fn metadata_below(_root: &Path, _relative: &Path) -> std::io::Result<std::fs::Metadata> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "resource inventory requires Linux",
    ))
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
            inspect(
                self.0.clone(),
                None,
                Some("committed.yaml".into()),
                serde_yaml_ng::from_str(yaml)?,
            )
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
        assert_eq!(inventory.geo[1].state, FileState::Empty);
        assert_eq!(inventory.geo[2].state, FileState::UnsafePath);
        assert_eq!(inventory.geo[3].state, FileState::NotFile);
        assert_eq!(inventory.geo[4].state, FileState::Missing);
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
}
