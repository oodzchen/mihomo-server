#![cfg(target_os = "linux")]
use anyhow::{Context as _, Result, ensure};
use flate2::{Compression, write::GzEncoder};
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions, CorePhase},
    resources::{Resources, TARGET},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let p = std::env::temp_dir().join(format!("ms-core-activation-{}", hash(&random)));
        fs::create_dir_all(p.join("resources/core"))?;
        fs::create_dir(p.join("resources/web"))?;
        fs::write(p.join("resources/web/index.html"), "<html>fixture</html>")?;
        Ok(Self(p))
    }
    fn resources(&self, binary: &Path, version: &str) -> Result<Resources> {
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
        Resources::open(&self.0.join("resources"))
    }
    fn options(&self, resources: &Resources) -> Result<CoreOptions> {
        let mut o = CoreOptions::new(
            self.0.join("resources/core/verge-mihomo"),
            self.0.join("data"),
            resources.bootstrap()?,
        );
        o.resources = Some(resources.clone());
        o.policy.readiness_attempts = 3;
        o.policy.probe_interval = Duration::from_millis(30);
        Ok(o)
    }
    fn seed(&self, binary: &Path, version: &str) -> Result<String> {
        let mut zip = GzEncoder::new(Vec::new(), Compression::fast());
        zip.write_all(&fs::read(binary)?)?;
        let package = zip.finish()?;
        let sha256 = hash(&package);
        let id = format!("{version}-{sha256}");
        let dir = self.0.join("data/core/.upgrade-staging").join(&id);
        fs::create_dir(&dir)?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        let tag = if version.starts_with("alpha-") {
            "Prerelease-Alpha"
        } else {
            version
        };
        let release = json!({"version":version,"target":TARGET,"asset":format!("mihomo-linux-amd64-v2-{version}.gz"),"bytes":package.len(),"sha256":sha256,"download_url":format!("https://github.com/MetaCubeX/mihomo/releases/download/{tag}/mihomo-linux-amd64-v2-{version}.gz")});
        fs::write(dir.join("package.gz"), package)?;
        fs::write(
            dir.join("release.json"),
            serde_json::to_vec(&json!({"schema_version":1,"release":release}))?,
        )?;
        for name in ["package.gz", "release.json"] {
            fs::set_permissions(dir.join(name), fs::Permissions::from_mode(0o600))?;
        }
        Ok(id)
    }
    fn live(&self) -> PathBuf {
        self.0.join("data/core/verge-mihomo")
    }
}
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
fn version(binary: &Path) -> Result<String> {
    let o = std::process::Command::new(binary).arg("-v").output()?;
    ensure!(o.status.success(), "version probe failed");
    Ok(String::from_utf8(o.stdout)?
        .split_whitespace()
        .nth(2)
        .context("version missing")?
        .into())
}
fn fixture(dir: &Directory, name: &str, version: &str, hang: bool) -> Result<PathBuf> {
    let source = dir.0.join(format!("{name}.rs"));
    let binary = dir.0.join(name);
    let marker = dir.0.join(format!("{name}-pid"));
    fs::write(
        &source,
        format!(
            r#"fn main() {{
        let args:Vec<_>=std::env::args().collect();
        if args.get(1).map(String::as_str)==Some("-v") {{ println!("Mihomo Meta {{}} linux amd64",{version:?});return; }}
        if args.get(1).map(String::as_str)==Some("-t") {{return;}}
        std::fs::write({marker:?},std::process::id().to_string()).unwrap();
        if {hang} {{std::thread::sleep(std::time::Duration::from_secs(60));}}
        std::process::exit(1);
    }}"#,
            marker = marker.to_str().unwrap()
        ),
    )?;
    let output = std::process::Command::new("rustc")
        .args(["--edition=2024", "--crate-name", "core_fixture"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()?;
    ensure!(output.status.success(), "compile core fixture failed");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
    Ok(binary)
}
#[tokio::test]
async fn stopped_activation_rejects_failed_start_restores_bytes_and_leaves_configuration_uncommitted() -> Result<()> {
    let dir = Directory::new()?;
    let old = fixture(&dir, "old", "v1.2.0", false)?;
    let resources = dir.resources(&old, "v1.2.0")?;
    let manager = CoreManager::spawn(dir.options(&resources)?)?;
    let result = async {
        let candidate = fixture(&dir, "bad", "v1.2.3", false)?;
        let id = dir.seed(&candidate, "v1.2.3")?;
        let staged = manager.stage_core_upgrade(id).await?;
        let old = fs::read(dir.live())?;
        let revision = manager.status().config_revision;
        let error = manager.activate_core_upgrade(staged.stage_id).await.unwrap_err();
        assert!(format!("{error:#}").contains("previous core restored"));
        assert_eq!(fs::read(dir.live())?, old);
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        assert_eq!(manager.status().config_revision, revision);
        assert!(manager.core_installation().await?.is_none());
        assert!(!dir.0.join("data/core/.core-upgrade").exists());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; running/stopped activation and restored node selections"]
async fn real_activation_verifies_runtime_preserves_profiles_and_retains_installation_after_restart() -> Result<()> {
    let real = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let version = version(&real)?;
    let dir = Directory::new()?;
    let resources = dir.resources(&real, &version)?;
    let manager = CoreManager::spawn(dir.options(&resources)?)?;
    let result=async {
        let profile=manager.import_profile_yaml("mode: rule\nproxies: []\nproxy-groups: [{name: Main, type: select, proxies: [DIRECT, REJECT]}]\nrules: ['MATCH,Main']\n".into(),"core activation".into()).await?;let uid=profile.uid.unwrap().to_string();manager.select_profile(uid.clone()).await?;manager.start().await?;manager.select_node("Main".into(),"REJECT".into()).await?;
        let before=manager.status();let config=manager.runtime_config().await?;let records=serde_json::to_value(manager.profiles())?;let inode=fs::metadata(dir.live())?.ino();
        let id=dir.seed(&real,&version)?;let staged=manager.stage_core_upgrade(id).await?;
        let report=manager.activate_core_upgrade(staged.stage_id.clone()).await?;assert!(report.upgraded);assert_eq!(report.to,version);assert_eq!(report.status.version.as_deref(),Some(version.as_str()));assert_eq!(report.status.phase,CorePhase::Running);assert_ne!(report.status.pid,before.pid);assert_ne!(fs::metadata(dir.live())?.ino(),inode);assert_eq!(manager.runtime_config().await?,config);assert_eq!(serde_json::to_value(manager.profiles())?,records);assert_eq!(manager.status().active_profile.as_deref(),Some(uid.as_str()));assert_eq!(manager.client().get_proxies().await?.proxies["Main"].now.as_deref(),Some("REJECT"));assert_eq!(manager.core_installation().await?,Some(report.installation.clone()));
        manager.stop().await?;let stopped=manager.activate_core_upgrade(staged.stage_id).await?;assert_eq!(stopped.status.phase,CorePhase::Stopped);assert!(stopped.status.pid.is_none());assert_eq!(manager.runtime_config().await?,config);
        Ok::<_,anyhow::Error>(stopped.installation)
    }.await;
    let cleanup = manager.shutdown().await;
    let receipt = result?;
    cleanup?;
    drop(manager);
    // A changed bundle seed must not replace the independently activated persistent core.
    fs::write(dir.0.join("resources/core/verge-mihomo"), b"changed bundle seed")?;
    let restarted = CoreManager::spawn(dir.options(&resources)?)?;
    let result = async {
        assert_eq!(restarted.core_installation().await?, Some(receipt.clone()));
        restarted.start().await?;
        assert_eq!(restarted.status().version.as_deref(), Some(version.as_str()));
        assert_eq!(hash(&fs::read(dir.live())?), receipt.executable_sha256);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restarted.shutdown().await;
    result.and(cleanup)
}
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; failed running activation and shutdown rollback"]
async fn real_failed_activation_restarts_old_core_and_shutdown_restores_without_restarting() -> Result<()> {
    failed_running_switch(false).await
}
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; Alpha readiness failure and shutdown rollback"]
async fn real_failed_alpha_activation_restores_stable_core_and_shutdown_reaps_candidate() -> Result<()> {
    failed_running_switch(true).await
}
async fn failed_running_switch(alpha: bool) -> Result<()> {
    let real = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let version = version(&real)?;
    let candidate_version = if alpha { "alpha-63bd52e" } else { version.as_str() };
    let dir = Directory::new()?;
    let resources = dir.resources(&real, &version)?;
    let manager = CoreManager::spawn(dir.options(&resources)?)?;
    let result = async {
        manager.start().await?;
        let before = manager.status();
        let config = manager.runtime_config().await?;
        let old = hash(&fs::read(dir.live())?);
        let candidate = fixture(&dir, "exits", candidate_version, false)?;
        let id = dir.seed(&candidate, candidate_version)?;
        let staged = manager.stage_core_upgrade(id).await?;
        assert!(manager.activate_core_upgrade(staged.stage_id.clone()).await.is_err());
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_ne!(manager.status().pid, before.pid);
        assert_eq!(manager.status().version.as_deref(), Some(version.as_str()));
        assert_eq!(manager.runtime_config().await?, config);
        assert_eq!(hash(&fs::read(dir.live())?), old);
        assert!(manager.core_installation().await?.is_none());
        let mut changed = manager.runtime_config().await?;
        changed.insert(
            serde_yaml_ng::Value::String("mode".into()),
            serde_yaml_ng::Value::String("global".into()),
        );
        manager.edit_config(changed).await?;
        let unchanged_pid = manager.status().pid;
        let stale = manager.activate_core_upgrade(staged.stage_id).await.unwrap_err();
        assert!(format!("{stale:#}").contains("configuration changed"));
        assert_eq!(manager.status().pid, unchanged_pid);
        assert_eq!(hash(&fs::read(dir.live())?), old);
        let candidate = fixture(&dir, "hangs", candidate_version, true)?;
        let id = dir.seed(&candidate, candidate_version)?;
        let staged = manager.stage_core_upgrade(id).await?;
        let own = manager.clone();
        let activation = tokio::spawn(async move { own.activate_core_upgrade(staged.stage_id).await });
        tokio::time::timeout(Duration::from_secs(10), async {
            while !dir.0.join("hangs-pid").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
        })
        .await?;
        manager.shutdown().await?;
        assert!(activation.await?.is_err());
        assert_eq!(manager.status().phase, CorePhase::Shutdown);
        assert_eq!(hash(&fs::read(dir.live())?), old);
        assert!(!dir.0.join("data/core/.core-upgrade").exists());
        let pid = fs::read_to_string(dir.0.join("hangs-pid"))?;
        assert!(!Path::new("/proc").join(pid.trim()).exists());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Subreaper(i32);
impl Subreaper {
    fn new() -> Result<Self> {
        let mut old = 0;
        ensure!(
            unsafe { libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut old as *mut i32) } == 0,
            "read subreaper failed"
        );
        ensure!(
            unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) } == 0,
            "set fixture subreaper failed"
        );
        Ok(Self(old))
    }
}
impl Drop for Subreaper {
    fn drop(&mut self) {
        unsafe {
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, self.0);
        }
    }
}
async fn reap_candidate(pid: i32) -> Result<()> {
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut status = 0;
            let wait = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            if wait == pid {
                ensure!(
                    libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGKILL,
                    "candidate was not killed on parent death"
                );
                return Ok::<_, anyhow::Error>(());
            }
            if wait < 0 {
                ensure!(
                    !Path::new("/proc").join(pid.to_string()).exists(),
                    "candidate still exists after parent death"
                );
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if result.is_err() {
        // Only signal a still-unreaped child owned by this fixture's subreaper.
        let mut status = 0;
        if unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } == 0 {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
                libc::waitpid(pid, &mut status, 0);
            }
        }
    }
    result.context("parent death did not terminate the candidate")?
}
async fn api(client: &reqwest::Client, base: &str, token: &str, command: Value) -> Result<Value> {
    let response = client
        .post(format!("{base}/api/commands"))
        .bearer_auth(token)
        .json(&command)
        .send()
        .await?;
    ensure!(response.status().is_success(), "fixture management command rejected");
    Ok(response.json().await?)
}
async fn interrupted_cli_switch(repair: bool, alpha: bool) -> Result<()> {
    let _subreaper = Subreaper::new()?;
    let real = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let version = version(&real)?;
    let dir = Directory::new()?;
    let _resources = dir.resources(&real, &version)?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    let base = format!("http://{address}");
    let spawn = || -> Result<Server> {
        Ok(Server(
            std::process::Command::new(env!("CARGO_BIN_EXE_mihomo-server"))
                .arg("--resource-dir")
                .arg(dir.0.join("resources"))
                .arg("--data-dir")
                .arg(dir.0.join("data"))
                .args(["--listen", &address.to_string(), "--no-start"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        ))
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut server = spawn()?;
    let mut token = String::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(value) = fs::read_to_string(dir.0.join("data/management-token")) {
                token = value.trim().into();
                if api(&client, &base, &token, json!({"command":"status"})).await.is_ok() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await?;
    api(&client, &base, &token, json!({"command":"start"})).await?;
    if repair {
        api(&client, &base, &token, json!({"command":"stop"})).await?;
        fs::write(dir.live(), [])?;
        fs::set_permissions(dir.live(), fs::Permissions::from_mode(0o0))?;
    }
    let previous_inode = fs::metadata(dir.live())?.ino();
    let old = if repair {
        hash(&[])
    } else {
        hash(&fs::read(dir.live())?)
    };
    let candidate_version = if alpha { "alpha-63bd52e" } else { version.as_str() };
    let candidate = fixture(&dir, "crash_hangs", candidate_version, true)?;
    let id = dir.seed(&candidate, candidate_version)?;
    let staged = api(&client, &base, &token, json!({"command":"stage_core_upgrade","id":id})).await?;
    let own_client = client.clone();
    let own_base = base.clone();
    let own_token = token.clone();
    let caller = tokio::spawn(async move {
        api(
            &own_client,
            &own_base,
            &own_token,
            json!({"command":"activate_core_upgrade","id":staged["stage_id"]}),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !dir.0.join("crash_hangs-pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await?;
    let pid: i32 = fs::read_to_string(dir.0.join("crash_hangs-pid"))?.trim().parse()?;
    assert_ne!(hash(&fs::read(dir.live())?), old);
    assert!(dir.0.join("data/core/.core-upgrade/journal.json").exists());
    server.0.kill()?;
    server.0.wait()?;
    assert!(caller.await?.is_err());
    reap_candidate(pid).await?;
    drop(server);
    let _restarted = spawn()?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if api(&client, &base, &token, json!({"command":"status"})).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await?;
    if repair {
        let metadata = fs::metadata(dir.live())?;
        assert_eq!(metadata.ino(), previous_inode);
        assert_eq!(metadata.len(), 0);
        assert_eq!(metadata.permissions().mode() & 0o777, 0);
        assert_eq!(
            api(&client, &base, &token, json!({"command":"installed_core_version"})).await?,
            "unknown"
        );
        // Recovery must leave management available for a subsequent successful repair.
        let id = dir.seed(&real, &version)?;
        let staged = api(&client, &base, &token, json!({"command":"stage_core_upgrade","id":id})).await?;
        let activated = api(
            &client,
            &base,
            &token,
            json!({"command":"activate_core_upgrade","id":staged["stage_id"]}),
        )
        .await?;
        assert_eq!(activated["from"], "unknown");
        assert_eq!(activated["status"]["phase"], "stopped");
    } else {
        assert_eq!(hash(&fs::read(dir.live())?), old);
    }
    assert!(!dir.0.join("data/core/.core-upgrade").exists());
    let receipt = api(&client, &base, &token, json!({"command":"core_installation"})).await?;
    assert_eq!(receipt.is_null(), !repair);
    let status = api(&client, &base, &token, json!({"command":"start"})).await?;
    assert_eq!(status["phase"], "running");
    assert_eq!(status["version"], version);
    // Normal SIGTERM ensures the restarted service reaps its working core before exit.
    let mut restarted = _restarted;
    unsafe {
        libc::kill(restarted.0.id() as i32, libc::SIGTERM);
    }
    let exit = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(exit) = restarted.0.try_wait()? {
                break Ok::<_, anyhow::Error>(exit);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("restarted service shutdown timed out")??;
    ensure!(exit.success(), "restarted service did not shut down cleanly");
    Ok(())
}

#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; SIGKILL during replacement, orphan termination and startup rollback"]
async fn interrupted_cli_switch_kills_candidate_and_restores_previous_core_before_next_start() -> Result<()> {
    interrupted_cli_switch(false, false).await
}
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; SIGKILL during broken-core repair, inode rollback and retry"]
async fn interrupted_cli_repair_restores_broken_inode_and_keeps_management_available_for_retry() -> Result<()> {
    interrupted_cli_switch(true, false).await
}

#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; SIGKILL during Alpha activation and startup rollback"]
async fn interrupted_alpha_cli_switch_restores_stable_core_before_restart() -> Result<()> {
    interrupted_cli_switch(false, true).await
}
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; SIGKILL during Alpha repair and original inode recovery"]
async fn interrupted_alpha_cli_repair_restores_broken_inode_before_retry() -> Result<()> {
    interrupted_cli_switch(true, true).await
}
