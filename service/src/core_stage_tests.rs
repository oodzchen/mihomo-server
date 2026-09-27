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
    let dir = Directory::new()?;
    let downloads = CoreDownloads::new(&dir.0)?;
    let bytes = elf();
    let prepared = seed(&downloads, &bytes, "v1.2.3")?;
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
