//! Private, bounded Linux executable staging. No live-core replacement occurs here.
use super::*;
use headless_core::config::runtime::MAX_CONFIG_BYTES;
use std::time::Instant;

const MAX_EXECUTABLE: u64 = 128 * 1024 * 1024;
const MAX_RESOURCES: u64 = 256 * 1024 * 1024;
const UNPACK_TIME: Duration = Duration::from_secs(15);
const PROBE_TIME: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StagedCore {
    pub stage_id: String,
    pub prepared: PreparedCore,
    pub executable_bytes: u64,
    pub executable_sha256: String,
    pub config_sha256: String,
    pub config_revision: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StageManifest {
    schema_version: u32,
    core: StagedCore,
}
use crate::secure_fs::sha256_hex as hash;
fn stage_name(id: &str) -> Result<(&str, &str)> {
    let (package_id, config_hash) = id.rsplit_once('-').context("invalid staged core ID")?;
    let (version, package_hash) = package_id.rsplit_once('-').context("invalid staged core ID")?;
    ensure!(
        release_version(version)
            && valid_hash(package_hash)
            && valid_hash(config_hash)
            && package_hash == package_hash.to_ascii_lowercase()
            && config_hash == config_hash.to_ascii_lowercase(),
        "invalid staged core ID"
    );
    Ok((package_id, config_hash))
}
fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    check_file(path, limit)?;
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        !bytes.is_empty() && bytes.len() as u64 <= limit,
        "staged file exceeds size bounds"
    );
    Ok(bytes)
}
fn check_elf(bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() >= 64
            && &bytes[..7] == b"\x7fELF\x02\x01\x01"
            && matches!(u16::from_le_bytes([bytes[16], bytes[17]]), 2 | 3)
            && u16::from_le_bytes([bytes[18], bytes[19]]) == 62
            && u32::from_le_bytes(bytes[20..24].try_into().unwrap()) == 1
            && u16::from_le_bytes([bytes[52], bytes[53]]) == 64,
        "candidate must be a Linux x86_64 ELF executable"
    );
    Ok(())
}
fn unpack(package: &[u8], output: &mut File, shutdown: &watch::Receiver<bool>, limit: u64) -> Result<(u64, String)> {
    let deadline = Instant::now() + UNPACK_TIME;
    let mut decoder = flate2::bufread::GzDecoder::new(package);
    let mut digest = Context::new(&SHA256);
    let mut size = 0;
    let mut header = Vec::new();
    let mut buffer = [0; 65536];
    loop {
        ensure!(!*shutdown.borrow(), "core unpack cancelled during shutdown");
        ensure!(Instant::now() < deadline, "core unpack timed out");
        let count = decoder.read(&mut buffer).context("invalid gzip core package")?;
        if count == 0 {
            break;
        }
        ensure!(count as u64 <= limit - size, "uncompressed core exceeds size bounds");
        header.extend_from_slice(&buffer[..count.min(64 - header.len())]);
        size += count as u64;
        digest.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    ensure!(
        decoder.into_inner().is_empty(),
        "core archive has trailing data or multiple members"
    );
    check_elf(&header)?;
    output.sync_all()?;
    Ok((size, hex(digest.finish().as_ref())))
}
impl CoreDownloads {
    /// Called by the actor, against a serialized snapshot of its current configuration.
    pub(crate) async fn stage(
        &self,
        id: &str,
        yaml: String,
        revision: Option<String>,
        data: &Path,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<StagedCore> {
        ensure!(!*shutdown.borrow(), "core staging cancelled during shutdown");
        ensure!(yaml.len() <= MAX_CONFIG_BYTES, "configuration exceeds 8 MiB");
        headless_core::config::runtime::parse(&yaml)?;
        let prepared = self.inspect(id)?;
        let package = self.root.join(id).join("package.gz");
        let pending = Pending::new(&self.root)?;
        let binary = pending.0.join("verge-mihomo");
        let candidate = pending.0.join("candidate.yaml");
        let config_sha256 = hash(yaml.as_bytes());
        let stage_id = format!("{id}-{config_sha256}");
        let path = binary.clone();
        let stop = shutdown.clone();
        let expected = prepared.release.sha256.clone();
        let extraction = tokio::task::spawn_blocking(move || {
            let package = bounded_read(&package, MAX_PACKAGE)?;
            ensure!(hash(&package) == expected, "core package changed before unpack");
            let mut output = create_private(&path)?;
            let result = unpack(&package, &mut output, &stop, MAX_EXECUTABLE)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                output.set_permissions(fs::Permissions::from_mode(0o700))?;
            }
            output.sync_all()?;
            Ok::<_, anyhow::Error>(result)
        });
        // Await the cooperative worker to completion before removing its files.
        let (executable_bytes, executable_sha256) = extraction.await.context("core unpack worker failed")??;
        ensure!(!*shutdown.borrow(), "core staging cancelled during shutdown");
        let mut file = create_private(&candidate)?;
        file.write_all(yaml.as_bytes())?;
        file.sync_all()?;
        drop(file);
        // Mihomo -t uses an isolated resource directory, never the live core's data directory.
        let resource_data = pending.0.join("validation-data");
        private_directory(&resource_data)?;
        let source_data = data.to_path_buf();
        let isolated = resource_data.clone();
        let stop = shutdown.clone();
        tokio::task::spawn_blocking(move || {
            let data = source_data;
            let resource_data = isolated;
            let mut total = 0;
            for name in ["geoip.metadb", "GeoSite.dat", "Country.mmdb", "GeoIP.dat"] {
                let source = data.join(name);
                if fs::symlink_metadata(&source).is_ok() {
                    let metadata = fs::symlink_metadata(&source)?;
                    ensure!(metadata.is_file(), "validation Geo resource must be a real file");
                    total += metadata.len();
                    ensure!(total <= MAX_RESOURCES, "validation Geo resources exceed size bounds");
                    let mut input = File::open(&source)?.take(metadata.len() + 1);
                    let mut output = create_private(&resource_data.join(name))?;
                    ensure!(
                        std::io::copy(&mut input, &mut output)? == metadata.len(),
                        "validation Geo resource changed"
                    );
                }
                ensure!(!*stop.borrow(), "core staging cancelled during shutdown");
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("validation resource worker failed")??;
        let version = crate::validation::probe_version(&binary, shutdown, PROBE_TIME).await?;
        ensure!(
            version == prepared.release.version,
            "candidate core version differs from release"
        );
        crate::validation::validate(&binary, &resource_data, &candidate, shutdown, PROBE_TIME)
            .await
            .map_err(|_| anyhow::anyhow!("candidate core configuration validation failed"))?;
        let core = StagedCore {
            stage_id: stage_id.clone(),
            prepared,
            executable_bytes,
            executable_sha256,
            config_sha256,
            config_revision: revision,
        };
        // Rehash after execution; a candidate that modifies itself cannot be published.
        ensure!(
            hash(&bounded_read(&binary, MAX_EXECUTABLE)?) == core.executable_sha256,
            "candidate executable changed during validation"
        );
        ensure!(
            hash(&bounded_read(&candidate, MAX_CONFIG_BYTES as u64)?) == core.config_sha256,
            "candidate configuration changed during validation"
        );
        fs::remove_dir_all(&resource_data)?;
        let mut manifest = create_private(&pending.0.join("stage.json"))?;
        manifest.write_all(&serde_json::to_vec(&StageManifest {
            schema_version: 1,
            core: core.clone(),
        })?)?;
        manifest.sync_all()?;
        sync_directory(&pending.0)?;
        ensure!(!*shutdown.borrow(), "core staging cancelled during shutdown");
        let destination = self.root.join(format!(".validated-{stage_id}"));
        if fs::symlink_metadata(&destination).is_ok() {
            let existing = self.inspect_stage(&stage_id)?;
            // Identical YAML may originate from a different runtime revision; retain its original proof.
            ensure!(
                existing.prepared == core.prepared && existing.executable_sha256 == core.executable_sha256,
                "staged core cache conflict"
            );
            return Ok(existing);
        }
        fs::rename(&pending.0, destination)?;
        sync_directory(&self.root)?;
        Ok(core)
    }
    pub(crate) fn staged_binary(&self, id: &str) -> Result<PathBuf> {
        self.inspect_stage(id)?;
        Ok(self.root.join(format!(".validated-{id}")).join("verge-mihomo"))
    }

    pub(crate) fn inspect_stage(&self, id: &str) -> Result<StagedCore> {
        let (package_id, config_hash) = stage_name(id)?;
        let prepared = self.inspect(package_id)?;
        let dir = self.root.join(format!(".validated-{id}"));
        check_directory(&dir)?;
        let manifest: StageManifest = serde_json::from_slice(&bounded_read(&dir.join("stage.json"), MAX_MANIFEST)?)
            .map_err(|_| anyhow::anyhow!("invalid staged core manifest"))?;
        let core = manifest.core;
        ensure!(
            manifest.schema_version == 1
                && core.stage_id == id
                && core.prepared == prepared
                && core.config_sha256 == config_hash
                && core.executable_bytes <= MAX_EXECUTABLE
                && valid_hash(&core.executable_sha256),
            "staged core manifest mismatch"
        );
        let binary = dir.join("verge-mihomo");
        let bytes = bounded_read(&binary, MAX_EXECUTABLE)?;
        check_elf(&bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            ensure!(
                fs::metadata(binary)?.permissions().mode() & 0o7777 == 0o700,
                "staged core is not executable"
            );
        }
        ensure!(
            bytes.len() as u64 == core.executable_bytes && hash(&bytes) == core.executable_sha256,
            "staged executable integrity check failed"
        );
        let yaml = bounded_read(&dir.join("candidate.yaml"), MAX_CONFIG_BYTES as u64)?;
        ensure!(
            hash(&yaml) == config_hash,
            "staged configuration integrity check failed"
        );
        Ok(core)
    }
}

#[cfg(test)]
#[path = "core_stage_tests.rs"]
mod tests;
