//! Read-only bundle resources and first-use initialization of a persistent core.
use anyhow::{Context as _, Result, ensure};
use ring::digest::{Context, SHA256};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

pub const TARGET: &str = env!("MIHOMO_SERVER_TARGET");

#[derive(Debug, Deserialize, Clone, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LicenseInfo {
    #[serde(default)]
    pub primary: Option<String>,
    #[serde(default)]
    pub inventory: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    target: String,
    core: Core,
    #[cfg(unix)]
    #[serde(default)]
    geo: BTreeMap<String, crate::geo::resources::Seed>,
    #[serde(default)]
    licenses: Option<LicenseInfo>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Core {
    version: String,
    sha256: String,
}

#[derive(Debug, Clone)]
pub struct Resources {
    root: PathBuf,
    version: String,
    hash: String,
    #[cfg(unix)]
    geo: BTreeMap<String, crate::geo::resources::Seed>,
    licenses: Option<LicenseInfo>,
}
impl Resources {
    pub fn directory(&self) -> &Path {
        &self.root
    }

    pub fn licenses(&self) -> Option<&LicenseInfo> {
        self.licenses.as_ref()
    }

    pub fn open(directory: &Path) -> Result<Self> {
        ensure!(cfg!(target_os = "linux"), "bundled resources currently require Linux");
        let root = directory.canonicalize().context("open resource directory")?;
        let manifest_path = inside(&root, "manifest.json")?;
        let mut bytes = Vec::new();
        File::open(manifest_path)?.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 64 * 1024, "resource manifest exceeds 64 KiB");
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        ensure!(manifest.schema_version == 1, "unsupported resource manifest schema");
        ensure!(
            manifest.target == TARGET,
            "resource target does not match service target {TARGET}"
        );
        ensure!(
            !manifest.core.version.is_empty()
                && manifest.core.version.len() <= 100
                && manifest
                    .core
                    .version
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c)),
            "invalid pinned core version"
        );
        ensure!(
            manifest.core.sha256.len() == 64 && manifest.core.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid pinned core SHA-256"
        );
        #[cfg(unix)]
        crate::geo::resources::validate_manifest(&manifest.geo)?;
        Ok(Self {
            root,
            hash: manifest.core.sha256.to_ascii_lowercase(),
            version: manifest.core.version,
            #[cfg(unix)]
            geo: manifest.geo,
            licenses: manifest.licenses,
        })
    }

    pub fn web_dir(&self) -> PathBuf {
        self.root.join("web")
    }

    pub fn bootstrap(&self) -> Result<PathBuf> {
        inside(&self.root, "minimal.yaml")
    }

    /// Integrity-pinned, no-overwrite initialization under the manager's data lock.
    pub fn initialize_geo(&self, data: &Path) -> Result<Vec<String>> {
        #[cfg(unix)]
        {
            crate::geo::resources::initialize(&self.root.join("geo"), data, &self.geo)
        }
        #[cfg(not(unix))]
        {
            let _ = data;
            Ok(Vec::new())
        }
    }

    #[cfg(unix)]
    pub(crate) fn geo_seed_info(&self, data: &Path, name: &str) -> Result<crate::geo::update::SeedInfo> {
        ensure!(
            crate::geo::validation::MMDB_FILES.contains(&name) || crate::geo::dat::DAT_FILES.contains(&name),
            "unsupported bundled Geo update name"
        );
        let seed = self
            .geo
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("no pinned bundle seed for this Geo file"))?;
        crate::geo::update::info(data, name, seed)
    }

    #[cfg(unix)]
    pub(crate) fn install_geo_seed(
        &self,
        data: &Path,
        request: &crate::geo::update::InstallRequest,
    ) -> Result<crate::geo::update::Receipt> {
        ensure!(
            crate::geo::validation::MMDB_FILES.contains(&request.name.as_str()),
            "only MMDB bundle updates are supported"
        );
        let seed = self
            .geo
            .get(&request.name)
            .ok_or_else(|| anyhow::anyhow!("no pinned bundle seed for this Geo file"))?;
        crate::geo::update::install(&self.root.join("geo"), data, seed, request)
    }

    #[cfg(unix)]
    pub(crate) fn prepare_dat_seed(
        &self,
        data: &Path,
        request: &crate::geo::update::InstallRequest,
    ) -> Result<crate::geo::update::Prepared> {
        ensure!(
            crate::geo::dat::DAT_FILES.contains(&request.name.as_str()),
            "only DAT bundle updates require a core probe"
        );
        let seed = self
            .geo
            .get(&request.name)
            .ok_or_else(|| anyhow::anyhow!("no pinned bundle seed for this Geo file"))?;
        crate::geo::update::prepare(&self.root.join("geo"), data, seed, request)
    }

    /// Called only while the manager owns its data-directory lock.
    /// Existing managed cores are authoritative, including independently upgraded
    /// ones, unless they are an older stable release than this bundle's pin: an
    /// installer upgrade then brings them up to the pin. Newer and Alpha cores stay.
    pub fn initialize_core(&self, directory: &Path) -> Result<PathBuf> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            builder.create(directory)?;
            let metadata = fs::symlink_metadata(directory)?;
            ensure!(
                metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0,
                "managed core directory must be a private real directory"
            );
        }
        #[cfg(not(unix))]
        fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        let destination = directory.join("verge-mihomo");
        if fs::symlink_metadata(&destination).is_ok() {
            #[cfg(target_os = "linux")]
            crate::core_upgrade::repairable(&destination)?;
            #[cfg(not(target_os = "linux"))]
            check_executable(&destination)?;
            if self.outdated(&destination) {
                // The previous Web installation receipt no longer describes the core.
                crate::core_upgrade::forget_installation(&directory)?;
                self.seed(&directory, &destination, true)?;
            }
            return Ok(destination);
        }
        self.seed(&directory, &destination, false)
    }

    fn outdated(&self, core: &Path) -> bool {
        match (
            stable_version(&self.version),
            core_version(core).as_deref().and_then(stable_version),
        ) {
            (Some(pinned), Some(installed)) => installed < pinned,
            _ => false,
        }
    }

    /// Copies the verified bundle core into place, atomically replacing an existing one.
    fn seed(&self, directory: &Path, destination: &Path, replace: bool) -> Result<PathBuf> {
        let source = inside(&self.root, "core/verge-mihomo")?;
        let mut source = File::open(source)?;
        let temporary = directory.join(format!(
            ".seed-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o700);
            }
            let mut output = options.open(&temporary)?;
            let mut digest = Context::new(&SHA256);
            let mut buffer = [0_u8; 65536];
            loop {
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
                output.write_all(&buffer[..count])?;
            }
            let actual = digest
                .finish()
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            ensure!(actual == self.hash, "bundled core SHA-256 mismatch");
            check_executable(&temporary)?;
            output.sync_all()?;
            if replace {
                fs::rename(&temporary, destination)?;
            } else {
                // Atomic publication without replacing a concurrent initializer's core.
                match fs::hard_link(&temporary, destination) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
            }
            #[cfg(unix)]
            File::open(directory)?.sync_all()?;
            check_executable(destination)?;
            Ok(destination.to_path_buf())
        })();
        let _ = fs::remove_file(&temporary);
        result
    }
}

/// `vMAJOR.MINOR.PATCH` only; Alpha and other builds are never compared.
fn stable_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.strip_prefix('v')?.split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(version)
}

/// The version an installed core reports, or None if it cannot be determined quickly.
fn core_version(core: &Path) -> Option<String> {
    let mut child = std::process::Command::new(core)
        .arg("-v")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut output = String::new();
    child.stdout.take()?.take(4096).read_to_string(&mut output).ok()?;
    let mut words = output.split_whitespace();
    (words.next() == Some("Mihomo") && words.next() == Some("Meta")).then_some(())?;
    words.next().map(str::to_owned)
}

fn inside(root: &Path, relative: &str) -> Result<PathBuf> {
    let file = root
        .join(relative)
        .canonicalize()
        .with_context(|| format!("missing resource {relative}"))?;
    ensure!(file.starts_with(root) && file.is_file(), "unsafe resource {relative}");
    Ok(file)
}
fn check_executable(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() > 0,
        "managed core must be a nonempty regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        ensure!(
            metadata.permissions().mode() & 0o100 != 0,
            "managed core must be executable by its owner"
        );
        ensure!(
            metadata.permissions().mode() & 0o022 == 0,
            "managed core must not be writable by other users"
        );
    }
    Ok(())
}
