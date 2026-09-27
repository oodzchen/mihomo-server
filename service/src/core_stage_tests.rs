#![cfg(all(unix, target_arch = "x86_64"))]
use super::*;
use flate2::{Compression, write::GzEncoder};
use std::os::unix::fs::{PermissionsExt as _, symlink};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!("ms-core-stage-{}", hex(&random)));
        fs::create_dir(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        fs::write(path.join("live-core"), b"original live core")?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut z = GzEncoder::new(Vec::new(), Compression::fast());
    z.write_all(bytes)?;
    Ok(z.finish()?)
}
fn elf() -> Vec<u8> {
    let mut header = vec![0; 64];
    header[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    header[16] = 2;
    header[18] = 62;
    header[20] = 1;
    header[52] = 64;
    header
}
fn seed(downloads: &CoreDownloads, bytes: &[u8], version: &str) -> Result<PreparedCore> {
    let package = gzip(bytes)?;
    let release = CoreRelease {
        version: version.into(),
        target: TARGET.into(),
        asset: asset_name(version)?,
        bytes: package.len() as u64,
        sha256: hash(&package),
        download_url: downloads.repository.package_url(version)?,
    };
    let id = format!("{version}-{}", release.sha256);
    let path = downloads.root.join(&id);
    private_directory(&path)?;
    private_file(&path.join("package.gz"))?.write_all(&package)?;
    private_file(&path.join("release.json"))?.write_all(&serde_json::to_vec(&Manifest {
        schema_version: 1,
        release: release.clone(),
    })?)?;
    Ok(PreparedCore { id, release })
}
#[test]
fn extraction_checks_crc_eof_single_member_size_elf_and_cancellation() -> Result<()> {
    let dir = Directory::new()?;
    let (stop, rx) = watch::channel(false);
    let bytes = elf();
    let package = gzip(&bytes)?;
    let mut output = private_file(&dir.0.join("valid"))?;
    assert_eq!(unpack(&package, &mut output, &rx, 64)?, (64, hash(&bytes)));
    let mut crc = package.clone();
    let n = crc.len();
    crc[n - 8] ^= 1;
    let mut architecture = bytes.clone();
    architecture[18] = 183;
    let cases = vec![
        crc,
        package[..package.len() - 1].to_vec(),
        [package.as_slice(), b"trailing"].concat(),
        [package.as_slice(), package.as_slice()].concat(),
        gzip(&[])?,
        gzip(b"#!/bin/sh\nexit 0")?,
        gzip(&architecture)?,
    ];
    for (i, package) in cases.into_iter().enumerate() {
        let mut output = private_file(&dir.0.join(format!("bad-{i}")))?;
        assert!(unpack(&package, &mut output, &rx, 64).is_err());
    }
    let mut output = private_file(&dir.0.join("limit"))?;
    assert!(unpack(&package, &mut output, &rx, 63).is_err());
    stop.send_replace(true);
    let mut output = private_file(&dir.0.join("cancel"))?;
    assert!(unpack(&package, &mut output, &rx, 64).is_err());
    Ok(())
}
#[tokio::test]
async fn failed_version_probe_and_shutdown_preserve_packages_live_core_and_remove_pending() -> Result<()> {
    let dir = Directory::new()?;
    let downloads = CoreDownloads::new(&dir.0)?;
    let prepared = seed(&downloads, &fs::read("/bin/true")?, "v1.2.3")?;
    let (stop, mut rx) = watch::channel(false);
    let error = downloads
        .stage(&prepared.id, "mode: direct\n".into(), None, &dir.0, &mut rx)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("version"));
    assert_eq!(fs::read_dir(&downloads.root)?.count(), 1);
    assert_eq!(fs::read(dir.0.join("live-core"))?, b"original live core");
    assert_eq!(downloads.inspect(&prepared.id)?.release, prepared.release);
    stop.send_replace(true);
    assert!(
        downloads
            .stage(&prepared.id, "mode: direct\n".into(), None, &dir.0, &mut rx)
            .await
            .is_err()
    );
    assert_eq!(fs::read_dir(&downloads.root)?.count(), 1);
    Ok(())
}
#[test]
fn staged_readback_requires_matching_private_manifest_executable_config_and_source_package() -> Result<()> {
    for version in ["v1.2.3", "alpha-63bd52e"] {
        let dir = Directory::new()?;
        let downloads = CoreDownloads::new(&dir.0)?;
        let bytes = elf();
        let prepared = seed(&downloads, &bytes, version)?;
        let yaml = b"mode: direct\n";
        let config_sha256 = hash(yaml);
        let stage_id = format!("{}-{config_sha256}", prepared.id);
        let core = StagedCore {
            stage_id: stage_id.clone(),
            prepared,
            executable_bytes: bytes.len() as u64,
            executable_sha256: hash(&bytes),
            config_sha256,
            config_revision: None,
        };
        let path = downloads.root.join(format!(".validated-{stage_id}"));
        private_directory(&path)?;
        private_file(&path.join("verge-mihomo"))?.write_all(&bytes)?;
        fs::set_permissions(path.join("verge-mihomo"), fs::Permissions::from_mode(0o700))?;
        private_file(&path.join("candidate.yaml"))?.write_all(yaml)?;
        private_file(&path.join("stage.json"))?.write_all(&serde_json::to_vec(&StageManifest {
            schema_version: 1,
            core: core.clone(),
        })?)?;
        assert_eq!(downloads.inspect_stage(&stage_id)?, core);
        for id in ["../private", "v1.2.3-../../private", "v1.2.3-a-b"] {
            assert!(downloads.inspect_stage(id).is_err());
        }
        fs::write(path.join("candidate.yaml"), b"mode: global\n")?;
        assert!(downloads.inspect_stage(&stage_id).is_err());
        fs::write(path.join("candidate.yaml"), yaml)?;
        fs::set_permissions(path.join("verge-mihomo"), fs::Permissions::from_mode(0o600))?;
        assert!(downloads.inspect_stage(&stage_id).is_err());
        fs::remove_file(path.join("verge-mihomo"))?;
        symlink(dir.0.join("live-core"), path.join("verge-mihomo"))?;
        assert!(downloads.inspect_stage(&stage_id).is_err());
    }
    Ok(())
}
#[tokio::test]
async fn version_probe_bounds_output_rejects_failures_and_reaps_on_timeout_and_shutdown() -> Result<()> {
    let dir = Directory::new()?;
    let script = dir.0.join("probe.py");
    let (_stop, mut rx) = watch::channel(false);
    for body in [
        "print('unexpected private diagnostic')",
        "print('Mihomo Meta v1.2.3');raise SystemExit(1)",
        "print('x'*70000)",
        "import sys;sys.stderr.write('x'*70000);print('Mihomo Meta v1.2.3')",
    ] {
        fs::write(&script, format!("#!/usr/bin/python3\n{body}\n"))?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700))?;
        let error = crate::validation::probe_version(&script, &mut rx, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(!format!("{error:#}").contains("private diagnostic"));
    }
    fs::write(
        &script,
        format!(
            "#!/usr/bin/python3\nimport os,time\nopen({:?},'w').write(str(os.getpid()))\ntime.sleep(60)\n",
            dir.0.join("pid").to_str().unwrap()
        ),
    )?;
    let error = crate::validation::probe_version(&script, &mut rx, Duration::from_millis(200))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("timed out"));
    let pid = fs::read_to_string(dir.0.join("pid"))?;
    assert!(!Path::new("/proc").join(pid.trim()).exists());
    fs::remove_file(dir.0.join("pid"))?;
    let (stop, mut rx) = watch::channel(false);
    let own = script.clone();
    let job =
        tokio::spawn(async move { crate::validation::probe_version(&own, &mut rx, Duration::from_secs(5)).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !dir.0.join("pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await?;
    stop.send_replace(true);
    assert!(job.await?.is_err());
    let pid = fs::read_to_string(dir.0.join("pid"))?;
    assert!(!Path::new("/proc").join(pid.trim()).exists());
    Ok(())
}
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; validates real executable and configuration without activation"]
async fn real_core_stages_validates_reads_after_restart_and_rejects_wrong_version_config_and_tampering() -> Result<()> {
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let (_stop, mut rx) = watch::channel(false);
    let version = crate::validation::probe_version(&binary, &mut rx, Duration::from_secs(5)).await?;
    let dir = Directory::new()?;
    let downloads = CoreDownloads::new(&dir.0)?;
    let bytes = fs::read(&binary)?;
    let prepared = seed(&downloads, &bytes, &version)?;
    let yaml = "mode: direct\ndns: {enable: false}\ntun: {enable: false}\nrules: [MATCH,DIRECT]\n";
    // MATCH,DIRECT must be one rule, not two YAML sequence scalars.
    let yaml = yaml.replace("[MATCH,DIRECT]", "['MATCH,DIRECT']");
    let staged = downloads
        .stage(&prepared.id, yaml.clone(), Some("revision-a".into()), &dir.0, &mut rx)
        .await?;
    assert_eq!(staged.executable_sha256, hash(&bytes));
    assert_eq!(downloads.inspect_stage(&staged.stage_id)?, staged);
    let restarted = CoreDownloads::new(&dir.0)?;
    assert_eq!(restarted.inspect_stage(&staged.stage_id)?, staged);
    assert_eq!(
        restarted
            .stage(&prepared.id, yaml, None, &dir.0, &mut rx)
            .await?
            .stage_id,
        staged.stage_id
    );
    assert!(
        downloads
            .stage(&prepared.id, "mode: invalid\n".into(), None, &dir.0, &mut rx)
            .await
            .is_err()
    );
    let wrong = seed(&downloads, &bytes, "v0.0.1")?;
    assert!(
        downloads
            .stage(&wrong.id, "mode: direct\n".into(), None, &dir.0, &mut rx)
            .await
            .is_err()
    );
    fs::write(
        downloads
            .root
            .join(format!(".validated-{}", staged.stage_id))
            .join("verge-mihomo"),
        b"changed",
    )?;
    assert!(downloads.inspect_stage(&staged.stage_id).is_err());
    assert_eq!(fs::read(dir.0.join("live-core"))?, b"original live core");
    assert!(!fs::read_dir(&downloads.root)?.any(|e| e.unwrap().file_name().to_str().is_some_and(pending_name)));
    Ok(())
}

fn alpha_fixture(dir: &Directory, behavior: &str, version: &str) -> Result<PathBuf> {
    let source = dir.0.join(format!("{behavior}.rs"));
    let binary = dir.0.join(behavior);
    let marker = dir.0.join(format!("{behavior}-pid"));
    let count = dir.0.join(format!("{behavior}-versions"));
    fs::write(
        &source,
        format!(
            r#"
        fn main() {{
            let args: Vec<_> = std::env::args().collect();
            let behavior = {behavior:?};
            let version_probe = args.get(1).map(String::as_str) == Some("-v");
            if (version_probe && behavior == "hang-version") || (!version_probe && behavior == "hang-config") {{
                std::fs::write({marker:?}, std::process::id().to_string()).unwrap();
                std::thread::sleep(std::time::Duration::from_secs(60));
            }}
            if version_probe {{
                let count = std::fs::read_to_string({count:?}).ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
                std::fs::write({count:?}, (count + 1).to_string()).unwrap();
                println!("Mihomo Meta {{}} linux amd64", {version:?});
                return;
            }}
            assert_eq!(args.get(1).map(String::as_str), Some("-t"));
            let data = std::path::Path::new(&args[3]);
            let config = std::path::Path::new(&args[5]);
            if data.join("geoip.metadb").exists() {{
                assert_eq!(std::fs::read(data.join("geoip.metadb")).unwrap(), b"isolated resource fixture");
            }}
            std::fs::write(data.join("probe-created"), b"isolated probe").unwrap();
            if behavior == "reject-config" {{ println!("private config diagnostic"); std::process::exit(1); }}
            if behavior == "mutate-config" {{ std::fs::write(config, b"mode: global\n").unwrap(); }}
            if behavior == "mutate-binary" {{
                use std::os::unix::fs::PermissionsExt as _;
                let own = std::env::current_exe().unwrap();
                let replacement = own.with_extension("replaced");
                let mut bytes = std::fs::read(&own).unwrap();
                *bytes.last_mut().unwrap() ^= 1;
                std::fs::write(&replacement, bytes).unwrap();
                std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o700)).unwrap();
                std::fs::rename(replacement, own).unwrap();
            }}
        }}
    "#,
            marker = marker.to_str().unwrap(),
            count = count.to_str().unwrap()
        ),
    )?;
    let output = std::process::Command::new("rustc")
        .args(["--edition=2024", "--crate-name", "alpha_stage_fixture"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()?;
    ensure!(output.status.success(), "compile Alpha staging fixture");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
    Ok(binary)
}
#[tokio::test]
async fn alpha_staging_proves_exact_version_config_isolation_and_rechecks_cached_files_after_restart() -> Result<()> {
    let dir = Directory::new()?;
    fs::write(dir.0.join("geoip.metadb"), b"isolated resource fixture")?;
    let binary = alpha_fixture(&dir, "success", "alpha-63bd52e")?;
    let downloads = CoreDownloads::new(&dir.0)?;
    let bytes = fs::read(binary)?;
    let prepared = seed(&downloads, &bytes, "alpha-63bd52e")?;
    let (_stop, mut rx) = watch::channel(false);
    let yaml = "mode: direct\nrules: ['MATCH,DIRECT']\n";
    let staged = downloads
        .stage(
            &prepared.id,
            yaml.into(),
            Some("revision-alpha".into()),
            &dir.0,
            &mut rx,
        )
        .await?;
    assert!(staged.stage_id.starts_with("alpha-63bd52e-"));
    assert_eq!(staged.executable_sha256, hash(&bytes));
    assert_eq!(staged.config_sha256, hash(yaml.as_bytes()));
    assert_eq!(staged.config_revision.as_deref(), Some("revision-alpha"));
    let path = downloads.staged_binary(&staged.stage_id)?;
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o7777, 0o700);
    assert!(!dir.0.join("probe-created").exists());
    assert!(!path.parent().unwrap().join("validation-data").exists());
    let restarted = CoreDownloads::new(&dir.0)?;
    assert_eq!(restarted.inspect_stage(&staged.stage_id)?, staged);
    assert_eq!(
        restarted
            .stage(&prepared.id, yaml.into(), None, &dir.0, &mut rx)
            .await?,
        staged
    );
    assert_eq!(fs::read(dir.0.join("success-versions"))?, b"2");
    fs::write(&path, b"tampered Alpha executable")?;
    assert!(restarted.inspect_stage(&staged.stage_id).is_err());
    assert_eq!(fs::read(dir.0.join("live-core"))?, b"original live core");
    assert_eq!(fs::read(dir.0.join("geoip.metadb"))?, b"isolated resource fixture");
    assert!(!fs::read_dir(&downloads.root)?.any(|e| e.unwrap().file_name().to_str().is_some_and(pending_name)));
    Ok(())
}
#[tokio::test]
async fn alpha_staging_rejects_wrong_version_configuration_and_probe_mutation_without_publishing() -> Result<()> {
    for (behavior, version, message) in [
        (
            "wrong-version",
            "alpha-abcdef0",
            "candidate core version differs from release",
        ),
        (
            "reject-config",
            "alpha-63bd52e",
            "candidate core configuration validation failed",
        ),
        (
            "mutate-config",
            "alpha-63bd52e",
            "candidate configuration changed during validation",
        ),
        (
            "mutate-binary",
            "alpha-63bd52e",
            "candidate executable changed during validation",
        ),
    ] {
        let dir = Directory::new()?;
        let binary = alpha_fixture(&dir, behavior, version)?;
        let downloads = CoreDownloads::new(&dir.0)?;
        let prepared = seed(&downloads, &fs::read(binary)?, "alpha-63bd52e")?;
        let (_stop, mut rx) = watch::channel(false);
        let error = downloads
            .stage(&prepared.id, "mode: direct\n".into(), None, &dir.0, &mut rx)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains(message));
        assert!(!format!("{error:#}").contains("private config diagnostic"));
        assert_eq!(fs::read_dir(&downloads.root)?.count(), 1);
        assert_eq!(downloads.inspect(&prepared.id)?, prepared);
        assert_eq!(fs::read(dir.0.join("live-core"))?, b"original live core");
    }
    Ok(())
}
#[tokio::test]
async fn alpha_probe_cancellation_terminates_reaps_and_removes_pending_in_both_phases() -> Result<()> {
    for behavior in ["hang-version", "hang-config"] {
        let dir = Directory::new()?;
        let binary = alpha_fixture(&dir, behavior, "alpha-63bd52e")?;
        let downloads = CoreDownloads::new(&dir.0)?;
        let root = downloads.root.clone();
        let prepared = seed(&downloads, &fs::read(binary)?, "alpha-63bd52e")?;
        let (stop, mut rx) = watch::channel(false);
        let data = dir.0.clone();
        let job = tokio::spawn(async move {
            downloads
                .stage(&prepared.id, "mode: direct\n".into(), None, &data, &mut rx)
                .await
        });
        let marker = dir.0.join(format!("{behavior}-pid"));
        tokio::time::timeout(Duration::from_secs(3), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let pid = fs::read_to_string(marker)?;
        stop.send_replace(true);
        assert!(tokio::time::timeout(Duration::from_secs(3), job).await??.is_err());
        assert!(!Path::new("/proc").join(pid.trim()).exists());
        assert_eq!(fs::read_dir(root)?.count(), 1);
        assert_eq!(fs::read(dir.0.join("live-core"))?, b"original live core");
    }
    Ok(())
}
#[tokio::test]
async fn actor_alpha_activation_revalidates_and_restores_stopped_core_after_failed_readiness() -> Result<()> {
    use crate::{
        core_manager::{CoreManager, CoreOptions},
        resources::Resources,
    };
    let dir = Directory::new()?;
    let stable = alpha_fixture(&dir, "stable", "v1.2.3")?;
    let alpha = alpha_fixture(&dir, "alpha", "alpha-63bd52e")?;
    fs::create_dir_all(dir.0.join("resources/core"))?;
    fs::copy(&stable, dir.0.join("resources/core/verge-mihomo"))?;
    fs::write(
        dir.0.join("resources/manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version":1,"target":TARGET,"core":{"version":"v1.2.3","sha256":hash(&fs::read(stable)?)}}
        ))?,
    )?;
    fs::write(
        dir.0.join("resources/minimal.yaml"),
        "mode: direct\nrules: ['MATCH,DIRECT']\n",
    )?;
    let resources = Resources::open(&dir.0.join("resources"))?;
    let mut options = CoreOptions::new(
        dir.0.join("resources/core/verge-mihomo"),
        dir.0.join("data"),
        resources.bootstrap()?,
    );
    options.resources = Some(resources);
    let manager = CoreManager::spawn(options)?;
    let result = async {
        let downloads = CoreDownloads::new(&dir.0.join("data/core"))?;
        let prepared = seed(&downloads, &fs::read(alpha)?, "alpha-63bd52e")?;
        let live = dir.0.join("data/core/verge-mihomo");
        let old = fs::read(&live)?;
        let staged = manager.stage_core_upgrade(prepared.id.clone()).await?;
        assert_eq!(manager.staged_core_upgrade(&staged.stage_id)?, staged);
        assert_eq!(fs::read(dir.0.join("alpha-versions"))?, b"1");
        let error = manager
            .activate_core_upgrade(staged.stage_id.clone())
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("previous core restored"));
        assert_eq!(fs::read(dir.0.join("alpha-versions"))?, b"2");
        assert_eq!(manager.status().phase, crate::core_manager::CorePhase::Stopped);
        assert!(manager.status().pid.is_none());
        assert_eq!(fs::read(live)?, old);
        assert!(manager.core_installation().await?.is_none());
        assert!(!dir.0.join("data/core/.core-upgrade").exists());
        assert!(manager.staged_core_upgrade(&staged.stage_id).is_ok());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
