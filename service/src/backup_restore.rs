//! Disposable candidate validation before a future multi-store restore transaction.
use super::hash;
use anyhow::{Context as _, Result, ensure};
use headless_core::{
    backup::BackupRestoreValidation,
    config::{
        dns::DnsOverrideState,
        profile_store::{DEFAULT_GLOBAL_SCRIPT, GenerationPlan, ProfileStore},
        runtime::{self, MAX_CONFIG_BYTES},
        settings::ServiceSettings,
    },
};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::sync::watch;

const MAX_GEO_BYTES: u64 = 256 * 1024 * 1024;
struct Budget {
    deadline: Instant,
    stop: watch::Receiver<bool>,
}
impl Budget {
    fn new(stop: watch::Receiver<bool>) -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(15),
            stop,
        }
    }
    fn check(&self) -> Result<()> {
        ensure!(!*self.stop.borrow(), "backup restore validation cancelled");
        ensure!(
            Instant::now() < self.deadline,
            "backup restore preparation exceeded 15 seconds"
        );
        Ok(())
    }
}
struct PrivateDirectory(PathBuf);
impl PrivateDirectory {
    fn new() -> Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| anyhow::anyhow!("restore candidate ID failed"))?;
        let path = std::env::temp_dir().join(format!("ms-restore-{}", &hash(&random)[..24]));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
    fn cleanup(&self) -> Result<()> {
        fs::remove_dir_all(&self.0).context("restore candidate cleanup failed")
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn private_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?)
}
fn write(path: &Path, bytes: &[u8], budget: &Budget) -> Result<()> {
    let mut output = private_file(path)?;
    for chunk in bytes.chunks(64 * 1024) {
        budget.check()?;
        output.write_all(chunk)?;
    }
    Ok(())
}
fn same(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
fn copy_geo(source: &Path, destination: &Path, budget: &Budget) -> Result<()> {
    let mut total = 0;
    for name in ["geoip.metadb", "GeoSite.dat", "Country.mmdb", "GeoIP.dat"] {
        budget.check()?;
        let path = source.join(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            result => {
                result?;
            }
        }
        let mut input = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)?;
        let before = input.metadata()?;
        ensure!(
            before.is_file()
                && before.nlink() == 1
                && before.uid() == unsafe { libc::geteuid() }
                && before.mode() & 0o7022 == 0,
            "unsafe validation Geo resource"
        );
        total = before
            .len()
            .checked_add(total)
            .context("validation Geo size overflow")?;
        ensure!(total <= MAX_GEO_BYTES, "validation Geo resources exceed 256 MiB");
        let mut output = private_file(&destination.join(name))?;
        let mut chunk = [0; 64 * 1024];
        let mut copied = 0;
        loop {
            budget.check()?;
            let count = input.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            copied += count as u64;
            ensure!(copied <= before.len(), "validation Geo resource changed");
            output.write_all(&chunk[..count])?;
        }
        ensure!(
            copied == before.len() && same(&before, &input.metadata()?) && same(&before, &fs::symlink_metadata(&path)?),
            "validation Geo resource changed"
        );
    }
    Ok(())
}

fn check_config(path: &Path, expected: &[u8], budget: &Budget) -> Result<Metadata> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let before = file.metadata()?;
    ensure!(
        before.is_file()
            && before.nlink() == 1
            && before.len() == expected.len() as u64
            && before.uid() == unsafe { libc::geteuid() }
            && before.mode() & 0o7022 == 0,
        "unsafe or changed restore candidate"
    );
    let mut chunk = [0; 64 * 1024];
    let mut at = 0;
    loop {
        budget.check()?;
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let end = at + count;
        ensure!(
            expected.get(at..end) == Some(&chunk[..count]),
            "restore candidate content changed"
        );
        at = end;
    }
    ensure!(
        at == expected.len() && same(&before, &file.metadata()?) && same(&before, &fs::symlink_metadata(path)?),
        "restore candidate identity changed"
    );
    Ok(before)
}

async fn validate_config(
    binary: &Path,
    data: &Path,
    path: &Path,
    bytes: &[u8],
    stop: &mut watch::Receiver<bool>,
    timeout: Duration,
) -> Result<()> {
    let before = check_config(path, bytes, &Budget::new(stop.clone()))?;
    crate::validation::validate(binary, data, path, stop, timeout)
        .await
        .map_err(|_| anyhow::anyhow!("restore runtime failed Mihomo validation"))?;
    let after = check_config(path, bytes, &Budget::new(stop.clone()))?;
    ensure!(same(&before, &after), "restore candidate changed during validation");
    Ok(())
}

struct Candidate {
    directory: PrivateDirectory,
    archive: headless_core::backup::BackupInspection,
    settings: ServiceSettings,
    generation: Option<GenerationPlan>,
    runtime: Vec<u8>,
}
fn prepare(bytes: &[u8], data: &Path, stop: watch::Receiver<bool>) -> Result<Candidate> {
    let budget = Budget::new(stop.clone());
    let archive = super::inspect::verify(bytes, stop.clone(), stop)?;
    budget.check()?;
    let directory = PrivateDirectory::new()?;
    fs::DirBuilder::new().mode(0o700).create(directory.0.join("profiles"))?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(directory.0.join("validation-data"))?;
    // Only verified, one-component paths are materialized. Never use ZipArchive::extract.
    for (name, contents) in &archive.contents {
        if *name != "manifest.json" {
            write(&directory.0.join(name), contents, &budget)?;
        }
    }
    let settings = serde_yaml_ng::from_slice(archive.contents["settings.yaml"])?;
    let profiles = ProfileStore::open(&directory.0)?;
    let generation = archive
        .manifest
        .active_profile
        .as_deref()
        .map(|uid| profiles.read_generation(uid))
        .transpose()?;
    let runtime = archive.contents["runtime.yaml"].to_vec();
    copy_geo(data, &directory.0.join("validation-data"), &budget)?;
    budget.check()?;
    Ok(Candidate {
        directory,
        archive: archive.report,
        settings,
        generation,
        runtime,
    })
}

/// Provider paths must remain within disposable validation data. Backup ZIPs do
/// not include provider caches; missing resources still fail Mihomo validation.
fn resource_paths(config: &serde_yaml_ng::Mapping) -> Result<()> {
    for key in ["proxy-providers", "rule-providers"] {
        if let Some(providers) = config.get(key).and_then(|v| v.as_mapping()) {
            for provider in providers.values().filter_map(|v| v.as_mapping()) {
                if let Some(path) = provider.get("path") {
                    let path = path.as_str().context("invalid provider resource path")?;
                    ensure!(
                        !path.is_empty()
                            && path.len() <= 1024
                            && !path.contains(['\\', ':'])
                            && !path.chars().any(char::is_control)
                            && Path::new(path)
                                .components()
                                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
                        "provider resource path escapes validation data"
                    );
                }
            }
        }
    }
    Ok(())
}
async fn script(
    config: serde_yaml_ng::Mapping,
    source: Option<String>,
    name: String,
    worker: &Path,
    stop: &mut watch::Receiver<bool>,
    timeout: Duration,
) -> Result<serde_yaml_ng::Mapping> {
    let Some(source) = source else {
        return Ok(config);
    };
    if source == DEFAULT_GLOBAL_SCRIPT {
        return runtime::generate(config, &serde_yaml_ng::Mapping::new());
    }
    let response = crate::script::execute(
        worker,
        headless_core::enhance::script::ScriptRequest {
            source,
            config,
            name,
            check_only: false,
        },
        stop,
        timeout,
    )
    .await?;
    // Uploaded console messages and diagnostics are not appended to live service logs.
    ensure!(response.error.is_none(), "restore enhancement script failed");
    runtime::generate(
        response.config.context("restore script returned no configuration")?,
        &serde_yaml_ng::Mapping::new(),
    )
}
async fn regenerate(
    generation: GenerationPlan,
    settings: &ServiceSettings,
    worker: &Path,
    stop: &mut watch::Receiver<bool>,
    timeout: Duration,
) -> Result<(Vec<u8>, bool)> {
    let requested = settings
        .profile_dns
        .get(&generation.profile_uid)
        .map_or(settings.runtime.dns.is_some(), |preference| preference.enabled);
    let dns = DnsOverrideState::new(&generation.profile_uid, generation.dns_source, requested, None);
    let mut authority = settings.runtime.clone();
    if !dns.enabled {
        authority.dns = None;
    }
    // Keep the same sequence -> settings/TUN/DNS -> global -> profile -> authority
    // -> finalization order as Actor::finish_generation/stage, without deriving twice.
    let config = authority.prepare(generation.config)?;
    let config = runtime::generate(config, &generation.global_merge)?;
    let config = script(
        config,
        generation.global_script,
        generation.name.clone(),
        worker,
        stop,
        timeout,
    )
    .await?;
    let config = runtime::generate(config, &generation.profile_merge)?;
    let config = script(config, generation.script, generation.name, worker, stop, timeout).await?;
    let config = headless_core::enhance::finalize::finalize(authority.enforce(config)?);
    resource_paths(&config)?;
    let bytes = serde_yaml_ng::to_string(&config)?.into_bytes();
    ensure!(
        bytes.len() <= MAX_CONFIG_BYTES,
        "regenerated restore runtime exceeds 8 MiB"
    );
    Ok((bytes, requested && !dns.enabled && dns.source.is_some()))
}

pub(crate) async fn validate(
    bytes: axum::body::Bytes,
    options: crate::core_manager::CoreOptions,
    mut stop: watch::Receiver<bool>,
) -> Result<BackupRestoreValidation> {
    let source = options.data_dir.clone();
    let cancellation = stop.clone();
    // Always join the cooperative filesystem worker before dropping its guard/permit.
    let candidate = tokio::task::spawn_blocking(move || prepare(&bytes, &source, cancellation))
        .await
        .context("restore preparation worker failed")??;
    let result = async {
        ensure!(!*stop.borrow(), "backup restore validation cancelled");
        let directory = &candidate.directory.0;
        let runtime_path = directory.join("runtime.yaml");
        resource_paths(&runtime::parse(std::str::from_utf8(&candidate.runtime)?)?)?;
        validate_config(
            &options.binary,
            &directory.join("validation-data"),
            &runtime_path,
            &candidate.runtime,
            &mut stop,
            options.policy.validation_timeout,
        )
        .await?;
        let mut regenerated = None;
        let mut confirmation = false;
        if let Some(generation) = candidate.generation.clone() {
            let worker = options.script_worker.clone().map_or_else(std::env::current_exe, Ok)?;
            let (bytes, requires_confirmation) = regenerate(
                generation,
                &candidate.settings,
                &worker,
                &mut stop,
                options.policy.script_timeout,
            )
            .await?;
            confirmation = requires_confirmation;
            let path = directory.join("regenerated.yaml");
            write(&path, &bytes, &Budget::new(stop.clone()))?;
            validate_config(
                &options.binary,
                &directory.join("validation-data"),
                &path,
                &bytes,
                &mut stop,
                options.policy.validation_timeout,
            )
            .await?;
            regenerated = Some(bytes);
        }
        ensure!(!*stop.borrow(), "backup restore validation cancelled");
        Ok(BackupRestoreValidation {
            archive: candidate.archive.clone(),
            runtime_bytes: candidate.runtime.len() as u64,
            runtime_sha256: hash(&candidate.runtime),
            regenerated_runtime_bytes: regenerated.as_ref().map(|bytes| bytes.len() as u64),
            regenerated_runtime_sha256: regenerated.as_ref().map(|bytes| hash(bytes)),
            dns_override_requires_confirmation: confirmation,
        })
    }
    .await;
    let cleanup = candidate.directory.cleanup();
    match result {
        Ok(report) => {
            cleanup?;
            Ok(report)
        }
        Err(error) => {
            cleanup?;
            Err(error)
        }
    }
}

#[cfg(test)]
#[path = "backup_restore_tests.rs"]
mod tests;
