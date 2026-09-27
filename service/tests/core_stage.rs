#![cfg(all(unix, target_arch = "x86_64"))]
use anyhow::{Context as _, Result};
use flate2::{Compression, write::GzEncoder};
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions, CorePhase},
    resources::{Resources, TARGET},
};
use serde_json::json;
use std::{fs, io::Write as _, os::unix::fs::PermissionsExt as _, path::PathBuf};
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; actor-owned staging preserves a running core and configuration"]
async fn actor_staging_preserves_running_core_and_snapshot_across_validation_failure_and_restart() -> Result<()> {
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory =
        Directory(std::env::temp_dir().join(format!("ms-core-stage-actor-{}-{stamp:x}", std::process::id())));
    fs::create_dir_all(directory.0.join("resources/core"))?;
    fs::copy(&binary, directory.0.join("resources/core/verge-mihomo"))?;
    let bytes = fs::read(&binary)?;
    let output = std::process::Command::new(&binary).arg("-v").output()?;
    let version = String::from_utf8(output.stdout)?
        .split_whitespace()
        .nth(2)
        .context("version missing")?
        .to_owned();
    fs::write(
        directory.0.join("resources/manifest.json"),
        serde_json::to_vec(
            &json!({"schema_version":1,"target":TARGET,"core":{"version":version,"sha256":hash(&bytes)}}),
        )?,
    )?;
    fs::write(
        directory.0.join("resources/minimal.yaml"),
        "mode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']\n",
    )?;
    let resources = Resources::open(&directory.0.join("resources"))?;
    let options = || -> Result<CoreOptions> {
        let mut options = CoreOptions::new(binary.clone(), directory.0.join("data"), resources.bootstrap()?);
        options.resources = Some(resources.clone());
        Ok(options)
    };
    let manager = CoreManager::spawn(options()?)?;
    let result=async {
        manager.start().await?;
        let prior=manager.status();let config=manager.runtime_config().await?;
        let live=directory.0.join("data/core/verge-mihomo");let live_hash=hash(&fs::read(&live)?);
        let mut gzip=GzEncoder::new(Vec::new(),Compression::fast());gzip.write_all(&bytes)?;let package=gzip.finish()?;
        let seed=|version:&str|->Result<String> {
            let digest=hash(&package);let id=format!("{version}-{digest}");
            let path=directory.0.join("data/core/.upgrade-staging").join(&id);fs::create_dir(&path)?;fs::set_permissions(&path,fs::Permissions::from_mode(0o700))?;
            let release=json!({"version":version,"target":TARGET,"asset":format!("mihomo-linux-amd64-v2-{version}.gz"),"bytes":package.len(),"sha256":digest,"download_url":format!("https://github.com/MetaCubeX/mihomo/releases/download/{version}/mihomo-linux-amd64-v2-{version}.gz")});
            fs::write(path.join("package.gz"),&package)?;fs::write(path.join("release.json"),serde_json::to_vec(&json!({"schema_version":1,"release":release}))?)?;
            for name in ["package.gz","release.json"] {fs::set_permissions(path.join(name),fs::Permissions::from_mode(0o600))?;}
            Ok(id)
        };
        let id=seed(&version)?;let staged=manager.stage_core_upgrade(id).await?;
        assert_eq!(staged.executable_sha256,live_hash);assert_eq!(manager.staged_core_upgrade(&staged.stage_id)?,staged);
        assert_eq!(manager.status().phase,CorePhase::Running);assert_eq!(manager.status().pid,prior.pid);assert_eq!(manager.status().generation,prior.generation);assert_eq!(manager.runtime_config().await?,config);
        let wrong=seed("v0.0.1")?;assert!(manager.stage_core_upgrade(wrong).await.is_err());
        assert_eq!(manager.status().pid,prior.pid);assert_eq!(hash(&fs::read(&live)?),live_hash);assert_eq!(manager.runtime_config().await?,config);
        Ok::<_,anyhow::Error>(staged)
    }.await;
    let cleanup = manager.shutdown().await;
    let staged = result?;
    cleanup?;
    drop(manager);
    let restarted = CoreManager::spawn(options()?)?;
    let readback = restarted.staged_core_upgrade(&staged.stage_id);
    let cleanup = restarted.shutdown().await;
    assert_eq!(readback?, staged);
    cleanup
}

#[tokio::test]
async fn disconnected_staging_caller_keeps_actor_admission_until_shutdown_drains_queued_work() -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let dir = Directory(std::env::temp_dir().join(format!("ms-core-stage-admission-{}-{stamp:x}", std::process::id())));
    fs::create_dir_all(dir.0.join("resources/core"))?;
    let pid_path = dir.0.join("validator-pid");
    let script = format!(
        "#!/usr/bin/python3\nimport os,time\nopen({:?},'w').write(str(os.getpid()))\ntime.sleep(60)\n",
        pid_path.to_str().unwrap()
    );
    let binary = dir.0.join("resources/core/verge-mihomo");
    fs::write(&binary, &script)?;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
    fs::write(
        dir.0.join("resources/manifest.json"),
        serde_json::to_vec(
            &json!({"schema_version":1,"target":TARGET,"core":{"version":"v1.2.3","sha256":hash(script.as_bytes())}}),
        )?,
    )?;
    fs::write(dir.0.join("resources/minimal.yaml"), "mode: direct\n")?;
    let resources = Resources::open(&dir.0.join("resources"))?;
    let mut options = CoreOptions::new(binary, dir.0.join("data"), resources.bootstrap()?);
    options.resources = Some(resources);
    let manager = CoreManager::spawn(options)?;
    let own = manager.clone();
    let start = tokio::spawn(async move { own.start().await });
    let result = async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !pid_path.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
        })
        .await?;
        let own = manager.clone();
        let caller = tokio::spawn(async move { own.stage_core_upgrade("v1.2.3-invalid".into()).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let error = manager.prepared_core_upgrade("v1.2.3-invalid").unwrap_err();
                if format!("{error:#}").contains("already in progress") {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await?;
        caller.abort();
        let _ = caller.await;
        let error = manager.prepared_core_upgrade("v1.2.3-invalid").unwrap_err();
        assert!(format!("{error:#}").contains("already in progress"));
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    let _ = start.await;
    result?;
    cleanup?;
    let pid = fs::read_to_string(pid_path)?;
    assert!(!std::path::Path::new("/proc").join(pid.trim()).exists());
    assert_eq!(fs::read_dir(dir.0.join("data/core/.upgrade-staging"))?.count(), 0);
    Ok(())
}
