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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    target: String,
    core: Core,
    #[cfg(unix)]
    #[serde(default)]
    geo: BTreeMap<String, crate::geo_resources::Seed>,
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
    hash: String,
    #[cfg(unix)]
    geo: BTreeMap<String, crate::geo_resources::Seed>,
}
impl Resources {
    pub fn directory(&self) -> &Path {
        &self.root
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
        crate::geo_resources::validate_manifest(&manifest.geo)?;
        Ok(Self {
            root,
            hash: manifest.core.sha256.to_ascii_lowercase(),
            #[cfg(unix)]
            geo: manifest.geo,
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
            crate::geo_resources::initialize(&self.root.join("geo"), data, &self.geo)
        }
        #[cfg(not(unix))]
        {
            let _ = data;
            Ok(Vec::new())
        }
    }

    #[cfg(unix)]
    pub(crate) fn geo_seed_info(&self, data: &Path, name: &str) -> Result<crate::geo_update::SeedInfo> {
        ensure!(
            crate::geo_validation::MMDB_FILES.contains(&name) || crate::dat_validation::DAT_FILES.contains(&name),
            "unsupported bundled Geo update name"
        );
        let seed = self
            .geo
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("no pinned bundle seed for this Geo file"))?;
        crate::geo_update::info(data, name, seed)
    }

    #[cfg(unix)]
    pub(crate) fn install_geo_seed(
        &self,
        data: &Path,
        request: &crate::geo_update::InstallRequest,
    ) -> Result<crate::geo_update::Receipt> {
        ensure!(
            crate::geo_validation::MMDB_FILES.contains(&request.name.as_str()),
            "only MMDB bundle updates are supported"
        );
        let seed = self
            .geo
            .get(&request.name)
            .ok_or_else(|| anyhow::anyhow!("no pinned bundle seed for this Geo file"))?;
        crate::geo_update::install(&self.root.join("geo"), data, seed, request)
    }

    #[cfg(unix)]
    pub(crate) fn prepare_dat_seed(
        &self,
        data: &Path,
        request: &crate::geo_update::InstallRequest,
    ) -> Result<crate::geo_update::Prepared> {
        ensure!(
            crate::dat_validation::DAT_FILES.contains(&request.name.as_str()),
            "only DAT bundle updates require a core probe"
        );
        let seed = self
            .geo
            .get(&request.name)
            .ok_or_else(|| anyhow::anyhow!("no pinned bundle seed for this Geo file"))?;
        crate::geo_update::prepare(&self.root.join("geo"), data, seed, request)
    }

    /// Called only while the manager owns its data-directory lock.
    /// Existing managed cores are authoritative, including independently upgraded ones.
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
            return Ok(destination);
        }
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
            // Atomic publication without replacing a concurrent initializer's core.
            match fs::hard_link(&temporary, &destination) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            #[cfg(unix)]
            File::open(&directory)?.sync_all()?;
            check_executable(&destination)?;
            Ok(destination)
        })();
        let _ = fs::remove_file(&temporary);
        result
    }
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
