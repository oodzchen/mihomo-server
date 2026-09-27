use super::*;
use crate::{
    core_release::CoreDownloads,
    resources::{Resources, TARGET},
};
use flate2::{Compression, write::GzEncoder};
use serde_json::json;
use std::{
    fs,
    io::Write as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random)?;
        // Keep the nested private controller below Linux's Unix socket path limit.
        let root = std::env::temp_dir().join(format!("ms-core-upgrade-adapter-{}", &hash(&random)[..32]));
        fs::create_dir_all(root.join("resources/core"))?;
        fs::create_dir(root.join("resources/web"))?;
        fs::write(root.join("resources/web/index.html"), "<html>fixture</html>")?;
        Ok(Self(root))
    }
    fn manager(&self, binary: &Path, version: &str) -> Result<CoreManager> {
        fs::copy(binary, self.0.join("resources/core/verge-mihomo"))?;
        fs::write(
            self.0.join("resources/manifest.json"),
            serde_json::to_vec(
                &json!({"schema_version":1,"target":TARGET,"core":{"version":version,"sha256":hash(&fs::read(binary)?)}}),
            )?,
        )?;
        fs::write(
            self.0.join("resources/minimal.yaml"),
            "mode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']\n",
        )?;
        let resources = Resources::open(&self.0.join("resources"))?;
        let mut options = CoreOptions::new(binary.to_path_buf(), self.0.join("data"), resources.bootstrap()?);
        options.resources = Some(resources);
        options.policy.readiness_attempts = 40;
        options.policy.probe_interval = Duration::from_millis(30);
        CoreManager::spawn(options)
    }
    fn live(&self) -> PathBuf {
        self.0.join("data/core/verge-mihomo")
    }
    fn seed(
        &self,
        binary: &Path,
        version: &str,
        downloads: &CoreDownloads,
    ) -> Result<crate::core_release::PreparedCore> {
        let mut zip = GzEncoder::new(Vec::new(), Compression::fast());
        zip.write_all(&fs::read(binary)?)?;
        let package = zip.finish()?;
        let sha256 = hash(&package);
        let id = format!("{version}-{sha256}");
        let path = self.0.join("data/core/.upgrade-staging").join(&id);
        fs::create_dir(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        let release = json!({"version":version,"target":TARGET,"asset":format!("mihomo-linux-amd64-v2-{version}.gz"),"bytes":package.len(),"sha256":sha256,"download_url":format!("https://github.com/MetaCubeX/mihomo/releases/download/{version}/mihomo-linux-amd64-v2-{version}.gz")});
        fs::write(path.join("package.gz"), package)?;
        fs::write(
            path.join("release.json"),
            serde_json::to_vec(&json!({"schema_version":1,"release":release}))?,
        )?;
        for name in ["package.gz", "release.json"] {
            fs::set_permissions(path.join(name), fs::Permissions::from_mode(0o600))?;
        }
        downloads.inspect(&id)
    }
    fn fixture(&self) -> Result<PathBuf> {
        let source = self.0.join("fixture.rs");
        let binary = self.0.join("fixture");
        fs::write(
            &source,
            r#"fn main(){let a:Vec<_>=std::env::args().collect();if a.get(1).map(String::as_str)==Some("-v"){println!("Mihomo Meta v1.2.3 linux amd64");return;}if a.get(1).map(String::as_str)==Some("-t"){return;}std::process::exit(1);}"#,
        )?;
        let status = std::process::Command::new("rustc")
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .status()?;
        ensure!(status.success(), "compile fixture");
        Ok(binary)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn hash(bytes: &[u8]) -> String {
    crate::core_upgrade::config_hash(bytes)
}
async fn check(manager: &CoreManager, version: &str, force: bool) -> Result<Option<CoreUpgradeReport>> {
    let (reply, response) = oneshot::channel();
    manager
        .commands
        .send(CommandMessage::CheckCoreUpgrade {
            version: version.into(),
            force,
            reply,
        })
        .await?;
    response.await?
}
async fn upgrade(
    manager: &CoreManager,
    prepared: crate::core_release::PreparedCore,
    force: bool,
) -> Result<CoreUpgradeReport> {
    let downloads = manager.core_downloads.clone().unwrap();
    let permit = Arc::clone(&manager.core_release_admission).try_acquire_owned()?;
    let (reply, response) = oneshot::channel();
    manager
        .commands
        .send(CommandMessage::UpgradePreparedCore {
            prepared,
            force,
            downloads,
            reply,
            _permit: permit,
        })
        .await?;
    response.await?
}

#[tokio::test]
async fn same_version_skips_candidate_and_preserves_state_but_force_validates_and_rolls_back_failure() -> Result<()> {
    let dir = Directory::new()?;
    let binary = dir.fixture()?;
    let manager = dir.manager(&binary, "v1.2.3")?;
    let result = async {
        let before = manager.status();
        let live = fs::read(dir.live())?;
        let inode = fs::metadata(dir.live())?.ino();
        assert_eq!(manager.installed_core_version().await?, "v1.2.3");
        let report = check(&manager, "v1.2.3", false).await?.unwrap();
        assert!(!report.upgraded);
        assert_eq!(report.from, report.to);
        assert!(check(&manager, "v1.2.3", true).await?.is_none());
        assert!(check(&manager, "v1.2.4", false).await?.is_none());
        let downloads = manager.core_downloads.as_ref().unwrap();
        let prepared = dir.seed(&binary, "v1.2.3", downloads)?;
        assert!(!upgrade(&manager, prepared.clone(), false).await?.upgraded);
        assert_eq!(manager.status().generation, before.generation);
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(fs::metadata(dir.live())?.ino(), inode);
        assert_eq!(fs::read(dir.live())?, live);
        assert!(manager.core_installation().await?.is_none());
        assert!(
            format!("{:#}", upgrade(&manager, prepared, true).await.unwrap_err()).contains("previous core restored")
        );
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        assert_eq!(fs::read(dir.live())?, live);
        assert!(manager.core_installation().await?.is_none());
        // The owned permit is released before the result reaches its caller.
        assert!(check(&manager, "v1.2.3", false).await?.is_some());
        assert!(manager.core_release_admission.clone().try_acquire_owned().is_ok());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; force/no-op runtime integration"]
async fn real_force_replaces_running_pid_and_stopped_force_preserves_stop_and_receipt() -> Result<()> {
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let output = std::process::Command::new(&binary).arg("-v").output()?;
    let version = String::from_utf8(output.stdout)?
        .split_whitespace()
        .nth(2)
        .context("version missing")?
        .to_string();
    let dir = Directory::new()?;
    let manager = dir.manager(&binary, &version)?;
    let result = async {
        manager.start().await?;
        let before = manager.status();
        let config = manager.runtime_config().await?;
        let inode = fs::metadata(dir.live())?.ino();
        let prepared = dir.seed(&binary, &version, manager.core_downloads.as_ref().unwrap())?;
        assert!(!upgrade(&manager, prepared.clone(), false).await?.upgraded);
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(fs::metadata(dir.live())?.ino(), inode);
        let report = upgrade(&manager, prepared.clone(), true).await?;
        assert!(report.upgraded);
        assert_eq!(report.from, version);
        assert_eq!(report.to, version);
        assert_ne!(manager.status().pid, before.pid);
        assert_ne!(fs::metadata(dir.live())?.ino(), inode);
        assert_eq!(manager.runtime_config().await?, config);
        assert_eq!(manager.status().config_revision, before.config_revision);
        manager.stop().await?;
        assert!(upgrade(&manager, prepared, true).await?.upgraded);
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        assert!(manager.status().pid.is_none());
        assert_eq!(manager.installed_core_version().await?, version);
        assert_eq!(manager.core_installation().await?.unwrap().version, version);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
