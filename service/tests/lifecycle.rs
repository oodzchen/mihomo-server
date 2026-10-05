#![cfg(unix)]

use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context as _, Result, ensure};
use headless_core::config::{
    profile_store::ProfileStore,
    runtime::{RuntimeStore, parse},
};
use mihomo_client::models::ClashMode;
use mihomo_server::core_manager::{CoreManager, CoreOptions, CorePhase, CoreStatus};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    process::Command,
    time::{sleep, timeout},
};

struct Directory(PathBuf);

impl Directory {
    async fn new() -> Result<Self> {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = Self(std::env::temp_dir().join(format!("ms-{}-{suffix:x}", std::process::id())));
        tokio::fs::create_dir_all(&root.0).await?;
        Ok(root)
    }

    async fn config(&self, mode: &str) -> Result<PathBuf> {
        let path = self.0.join("runtime.yaml");
        let config = json!({
            "mixed-port": 0, "mode": mode, "log-level": "debug", "external-controller": "",
            "dns": {"enable": false}, "tun": {"enable": false},
            "proxy-groups": [{"name": "Main", "type": "select", "proxies": ["DIRECT", "REJECT"]}],
            "rules": ["MATCH,DIRECT"]
        });
        tokio::fs::write(&path, serde_json::to_vec(&config)?).await?;
        Ok(path)
    }

    fn options(&self, config: PathBuf) -> Result<CoreOptions> {
        let binary = std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?;
        let mut options = CoreOptions::new(binary.into(), self.0.clone(), config);
        options.policy.recovery_backoff = Duration::from_millis(50);
        options.policy.stop_timeout = Duration::from_millis(500);
        Ok(options)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn send_signal(pid: u32, signal: i32) -> Result<()> {
    // Test PIDs are obtained exclusively from the owned manager or service child.
    ensure!(
        unsafe { libc::kill(pid as libc::pid_t, signal) } == 0,
        "signal failed: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

fn reaped(pid: u32) -> Result<()> {
    ensure!(
        unsafe { libc::kill(pid as libc::pid_t, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH),
        "owned PID {pid} remains alive or unreaped"
    );
    Ok(())
}

async fn wait_state(manager: &CoreManager, predicate: impl Fn(&CoreStatus) -> bool) -> Result<CoreStatus> {
    let mut states = manager.subscribe_status();
    timeout(Duration::from_secs(8), async {
        loop {
            let state = states.borrow_and_update().clone();
            if predicate(&state) {
                return Ok::<_, anyhow::Error>(state);
            }
            states.changed().await?;
        }
    })
    .await?
}

#[tokio::test]
#[ignore = "requires real Mihomo and local TCP/Unix sockets"]
async fn switching_listener_type_on_the_same_port_recovers_and_occupied_ports_roll_back() -> Result<()> {
    let directory = Directory::new().await?;
    let reserved = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = reserved.local_addr()?.port();
    drop(reserved);
    let source = directory.0.join("bootstrap.yaml");
    tokio::fs::write(&source, format!("mixed-port: {port}\nmode: direct\nallow-lan: false\n")).await?;
    let manager = CoreManager::spawn(directory.options(source)?)?;
    let result = async {
        manager.start().await?;
        assert_eq!(manager.client().get_base_config().await?.mixed_port, port);
        let imported = manager
            .import_profile_yaml(
                format!("port: {port}\nmode: direct\nallow-lan: false\n"),
                "HTTP profile".into(),
            )
            .await?;
        manager.select_profile(imported.uid.unwrap().to_string()).await?;
        let core = manager.client().get_base_config().await?;
        assert_eq!(core.mixed_port, 0);
        assert_eq!(core.port, port);
        tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        let before = manager.status().config_revision;
        let blocked = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let occupied = blocked.local_addr()?.port();
        let error = manager
            .edit_config(parse(&format!("port: {occupied}\nmode: direct\nallow-lan: false\n"))?)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("listener mismatch"), "{error:#}");
        assert!(
            format!("{error:#}").contains(&format!("(pid {})", std::process::id())),
            "{error:#}"
        );
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_eq!(manager.status().config_revision, before);
        assert_eq!(manager.client().get_base_config().await?.port, port);
        tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires a real core and local socket binding permissions"]
async fn live_lifecycle_serializes_operations_and_recovers_configuration_failure() -> Result<()> {
    let directory = Directory::new().await?;
    let config = directory.0.join("runtime.yaml");
    let manager = CoreManager::spawn(directory.options(config.clone())?)?;
    let result = async {
        ensure!(CoreManager::spawn(directory.options(config)?).is_err());
        ensure!(manager.start().await.is_err());
        assert_eq!(manager.status().phase, CorePhase::Failed);
        ensure!(manager.status().pid.is_none());
        directory.config("rule").await?;
        let (first, second) = tokio::join!(manager.start(), manager.start());
        let first = first?;
        assert_eq!(first.phase, CorePhase::Running);
        assert_eq!(first.pid, second?.pid);
        ensure!(first.version.is_some());
        let first_pid = first.pid.context("missing core PID")?;
        let invalid = directory.0.join("invalid.yaml");
        tokio::fs::write(&invalid, "external-controller: ''\nrules: ['INVALID,DIRECT']\n").await?;
        ensure!(manager.reload_config(invalid).await.is_err());
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Rule);
        let candidate = directory.0.join("candidate.yaml");
        let yaml = tokio::fs::read_to_string(directory.0.join("runtime.yaml")).await?;
        let mut config: Value = serde_json::from_str(&yaml)?;
        config["mode"] = json!("direct");
        tokio::fs::write(&candidate, serde_json::to_vec(&config)?).await?;
        manager.reload_config(candidate).await?;
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Direct);
        let restarted = manager.restart().await?;
        ensure!(restarted.pid != first.pid);
        ensure!(restarted.generation > first.generation);
        reaped(first_pid)?;
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Direct);
        let second_pid = restarted.pid.context("missing restarted PID")?;
        let (restart, stop) = tokio::join!(manager.restart(), manager.stop());
        restart?;
        assert_eq!(stop?.phase, CorePhase::Stopped);
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        reaped(second_pid)?;
        ensure!(!manager.logs().is_empty());
        ensure!(manager.logs().len() <= 200);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)?;
    assert_eq!(manager.status().phase, CorePhase::Shutdown);
    ensure!(manager.start().await.is_err());
    let replacement = CoreManager::spawn(directory.options(directory.0.join("runtime.yaml"))?)?;
    replacement.shutdown().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a real core and local socket binding permissions"]
async fn live_crash_recovery_is_bounded_and_manual_start_resets_the_budget() -> Result<()> {
    let directory = Directory::new().await?;
    let mut options = directory.options(directory.config("rule").await?)?;
    options.policy.recovery_limit = 2;
    let manager = CoreManager::spawn(options)?;
    let result = async {
        let mut status = manager.start().await?;
        for attempt in 1..=2 {
            let pid = status.pid.context("missing PID")?;
            send_signal(pid, libc::SIGKILL)?;
            status = wait_state(&manager, |state| {
                state.phase == CorePhase::Running && state.generation > status.generation
            })
            .await?;
            assert_eq!(status.recovery_attempt, attempt);
            reaped(pid)?;
        }
        let pid = status.pid.context("missing PID")?;
        send_signal(pid, libc::SIGKILL)?;
        let failed = wait_state(&manager, |state| {
            state.phase == CorePhase::Failed && state.pid.is_none()
        })
        .await?;
        assert_eq!(failed.recovery_attempt, 2);
        ensure!(failed.error.is_some());
        sleep(Duration::from_millis(250)).await;
        assert_eq!(manager.status().generation, failed.generation);
        reaped(pid)?;
        assert_eq!(manager.start().await?.recovery_attempt, 0);
        manager.stop().await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires a real core and local socket binding permissions"]
async fn live_service_signals_reap_the_child_and_failed_start_keeps_service_alive() -> Result<()> {
    for (signal, fail, import) in [
        (libc::SIGTERM, false, false),
        (libc::SIGINT, false, false),
        (libc::SIGHUP, false, false),
        (libc::SIGTERM, true, false),
        (libc::SIGTERM, false, true),
    ] {
        let directory = Directory::new().await?;
        let config = if fail {
            directory.0.join("missing.yaml")
        } else {
            directory.config("rule").await?
        };
        let binary = std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?;
        let mut service = Command::new(env!("CARGO_BIN_EXE_mihomo-server"))
            .arg("--listen")
            .arg(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.to_string())
            .arg("--mihomo")
            .arg(binary)
            .arg("--data-dir")
            .arg(&directory.0)
            .arg(if import { "--import-config" } else { "--config" })
            .arg(config)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let service_pid = service.id().context("missing service PID")?;
        let mut output = BufReader::new(service.stdout.take().context("missing service stdout")?).lines();
        let status = timeout(Duration::from_secs(8), async {
            loop {
                let line = output
                    .next_line()
                    .await?
                    .context("service exited before expected state")?;
                let status: Value = serde_json::from_str(&line)?;
                if status["phase"] == if fail { "failed" } else { "running" } {
                    return Ok::<_, anyhow::Error>(status);
                }
            }
        })
        .await??;
        ensure!(service.try_wait()?.is_none());
        send_signal(service_pid, signal)?;
        // Continue draining state output while waiting for orderly service shutdown.
        let (exit, drained) = tokio::join!(timeout(Duration::from_secs(8), service.wait()), async {
            while output.next_line().await?.is_some() {}
            Ok::<_, anyhow::Error>(())
        });
        ensure!(exit??.success());
        drained?;
        if let Some(pid) = status["pid"].as_u64() {
            reaped(pid as u32)?;
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a real core and local socket binding permissions"]
async fn live_configuration_commits_recovers_and_applies_while_stopped() -> Result<()> {
    let directory = Directory::new().await?;
    let source = directory.config("rule").await?;
    let manager = CoreManager::spawn(directory.options(source.clone())?)?;
    let result = async {
        let initial = manager.start().await?;
        ensure!(initial.config_revision.is_some());
        let bytes = tokio::fs::read(&source).await?;
        let rejected = parse("mode: direct\nrules: ['INVALID,DIRECT']")?;
        ensure!(manager.apply_config(rejected).await.is_err());
        assert_eq!(manager.status().pid, initial.pid);
        assert_eq!(manager.status().config_revision, initial.config_revision);
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Rule);
        manager.apply_overlay(parse("mode: direct")?).await?;
        assert_eq!(manager.status().pid, initial.pid);
        ensure!(manager.status().config_revision != initial.config_revision);
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Direct);
        assert_eq!(tokio::fs::read(&source).await?, bytes);
        manager.apply_overlay(parse("mode: rule")?).await?;
        let restored = manager.restart().await?;
        ensure!(restored.pid != initial.pid);
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Rule);
        reaped(initial.pid.context("initial PID missing")?)?;
        Ok::<_, anyhow::Error>(restored.config_revision)
    }
    .await;
    let cleanup = manager.shutdown().await;
    let committed = result?;
    cleanup?;
    tokio::fs::remove_file(&source).await?;
    // Simulate a crash with a staged, uncommitted revision on disk.
    let mut store = RuntimeStore::open(&directory.0)?;
    let unfinished = store.stage(parse("mode: direct")?)?;
    store.begin(unfinished)?;
    drop(store);
    let manager = CoreManager::spawn(directory.options(source)?)?;
    let result = async {
        assert_eq!(manager.status().config_revision, committed);
        manager.start().await?;
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Rule);
        manager.stop().await?;
        let stored = manager.apply_overlay(parse("mode: direct")?).await?;
        assert_eq!(stored.phase, CorePhase::Stopped);
        ensure!(stored.pid.is_none());
        manager.start().await?;
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Direct);
        manager.stop().await?;
        let store = RuntimeStore::open(&directory.0)?;
        tokio::fs::remove_file(store.current_path()?.context("missing committed path")?).await?;
        ensure!(manager.start().await.is_err());
        assert_eq!(manager.status().phase, CorePhase::Failed);
        let replacement = directory.config("rule").await?;
        manager.import_config(replacement).await?;
        manager.start().await?;
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Rule);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires a real core and local socket binding permissions"]
async fn live_profile_selection_preserves_old_state_and_recovers_interrupted_switch() -> Result<()> {
    let directory = Directory::new().await?;
    let source = directory.config("rule").await?;
    let manager = CoreManager::spawn(directory.options(source.clone())?)?;
    let result = async {
        let first = manager.import_profile(source.clone(), Some("first".into())).await?;
        let first_uid = first.uid.as_deref().context("first UID missing")?.to_owned();
        ensure!(manager.profiles().current.is_none());
        let selected = manager.select_profile(first_uid.clone()).await?;
        assert_eq!(selected.phase, CorePhase::Stopped);
        assert_eq!(selected.active_profile.as_deref(), Some(first_uid.as_str()));
        assert_eq!(manager.profiles().current.as_deref(), Some(first_uid.as_str()));
        let initial = manager.start().await?;
        let invalid = directory.0.join("invalid-profile.yaml");
        tokio::fs::write(&invalid, "mode: direct\nrules: ['INVALID,DIRECT']").await?;
        let invalid = manager.import_profile(invalid, None).await?;
        ensure!(
            manager
                .select_profile(invalid.uid.context("invalid UID missing")?.to_string())
                .await
                .is_err()
        );
        ensure!(manager.select_profile("unknown-profile".into()).await.is_err());
        assert_eq!(manager.status().active_profile, initial.active_profile);
        assert_eq!(manager.status().config_revision, initial.config_revision);
        assert_eq!(manager.status().pid, initial.pid);
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Rule);
        directory.config("direct").await?;
        let second = manager.import_profile(source.clone(), Some("second".into())).await?;
        let second_uid = second.uid.as_deref().context("second UID missing")?.to_owned();
        assert_eq!(manager.profiles().current.as_deref(), Some(first_uid.as_str()));
        let second_status = manager.select_profile(second_uid.clone()).await?;
        assert_eq!(second_status.pid, initial.pid);
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Direct);
        manager.apply_overlay(parse("mode: rule")?).await?;
        assert_eq!(manager.status().active_profile.as_deref(), Some(second_uid.as_str()));
        manager
            .apply_config(parse(&tokio::fs::read_to_string(&source).await?)?)
            .await?;
        ensure!(manager.status().active_profile.is_none() && manager.profiles().current.is_none());
        let committed = manager.select_profile(second_uid.clone()).await?;
        // Force only the compatibility catalog write to fail after successful core reload.
        let metadata = directory.0.join("profiles.yaml");
        let backup = directory.0.join("profiles-backup.yaml");
        tokio::fs::rename(&metadata, &backup).await?;
        tokio::fs::create_dir(&metadata).await?;
        let rejected = manager.select_profile(first_uid.clone()).await;
        tokio::fs::remove_dir(&metadata).await?;
        tokio::fs::rename(&backup, &metadata).await?;
        ensure!(rejected.is_err());
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_eq!(manager.status().config_revision, committed.config_revision);
        assert_eq!(manager.status().active_profile, committed.active_profile);
        assert_eq!(manager.profiles().current.as_deref(), Some(second_uid.as_str()));
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Direct);
        Ok::<_, anyhow::Error>((first_uid, second_uid))
    }
    .await;
    let cleanup = manager.shutdown().await;
    let (first_uid, second_uid) = result?;
    cleanup?;
    tokio::fs::remove_file(&source).await?;
    let mut profiles = ProfileStore::open(&directory.0)?;
    let mut runtime = RuntimeStore::open(&directory.0)?;
    let candidate = runtime.stage(profiles.read_mapping(&first_uid)?)?;
    runtime.begin_profile(candidate, Some(first_uid.clone()))?;
    profiles.set_current(Some(&first_uid))?; // Crash after mirror save, before journal commit.
    drop((profiles, runtime));
    let manager = CoreManager::spawn(directory.options(source)?)?;
    let result = async {
        assert_eq!(manager.status().active_profile.as_deref(), Some(second_uid.as_str()));
        assert_eq!(manager.profiles().current.as_deref(), Some(second_uid.as_str()));
        manager.start().await?;
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Direct);
        ensure!(RuntimeStore::open(&directory.0)?.state().pending.is_none());
        assert_eq!(
            ProfileStore::open(&directory.0)?.snapshot().current.as_deref(),
            Some(second_uid.as_str())
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires a real core and local socket binding permissions"]
async fn live_cli_import_select_and_restart_restore_a_local_profile() -> Result<()> {
    let directory = Directory::new().await?;
    let source = directory.config("rule").await?;
    let binary = std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?;
    let mut uid = None::<String>;
    let mut revision = None::<String>;
    for step in 0..3 {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mihomo-server"));
        command.arg("--mihomo").arg(&binary).arg("--data-dir").arg(&directory.0);
        command
            .arg("--listen")
            .arg(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.to_string());
        if step == 0 {
            command
                .arg("--import-profile")
                .arg(&source)
                .arg("--profile-name")
                .arg("CLI subscription");
        } else if step == 1 {
            command
                .arg("--select-profile")
                .arg(uid.as_ref().context("profile UID missing")?)
                .arg("--no-start");
        }
        let mut service = command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let service_pid = service.id().context("service PID missing")?;
        let mut output = BufReader::new(service.stdout.take().context("stdout missing")?).lines();
        let observed = timeout(Duration::from_secs(8), async {
            loop {
                let line = output
                    .next_line()
                    .await?
                    .context("service exited before profile operation completed")?;
                let status: Value = serde_json::from_str(&line)?;
                let ready = if step == 1 {
                    status["phase"] == "stopped" && status["config_revision"].as_str() != revision.as_deref()
                } else {
                    status["phase"] == "running"
                };
                if ready && status["active_profile"].is_string() && status["config_revision"].is_string() {
                    break Ok::<_, anyhow::Error>(status);
                }
            }
        })
        .await;
        send_signal(service_pid, libc::SIGTERM)?;
        let (exit, drained) = tokio::join!(timeout(Duration::from_secs(8), service.wait()), async {
            while output.next_line().await?.is_some() {}
            Ok::<(), anyhow::Error>(())
        });
        ensure!(exit??.success());
        drained?;
        let status = observed??;
        let selected = status["active_profile"]
            .as_str()
            .context("selected UID missing")?
            .to_owned();
        if let Some(uid) = &uid {
            assert_eq!(selected, *uid);
        }
        uid = Some(selected);
        revision = status["config_revision"].as_str().map(ToOwned::to_owned);
        if let Some(pid) = status["pid"].as_u64() {
            reaped(pid as u32)?;
        }
        if step == 0 {
            tokio::fs::remove_file(&source).await?;
        }
    }
    assert_eq!(
        ProfileStore::open(&directory.0)?.snapshot().current.as_deref(),
        uid.as_deref()
    );
    Ok(())
}

#[tokio::test]
async fn validation_timeout_and_shutdown_reap_the_validator_without_committing() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    for cancel in [false, true] {
        let directory = Directory::new().await?;
        let config = directory.config("rule").await?;
        let binary = directory.0.join("validator.py");
        tokio::fs::write(&binary, "#!/usr/bin/python3\nimport sys,os,time,signal\nfrom pathlib import Path\nassert '-t' in sys.argv\nsignal.signal(signal.SIGTERM,signal.SIG_IGN)\nPath(sys.argv[sys.argv.index('-d')+1]+'/validator.pid').write_text(str(os.getpid()))\nwhile True: time.sleep(1)\n").await?;
        tokio::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).await?;
        let mut options = CoreOptions::new(binary, directory.0.clone(), config);
        options.policy.validation_timeout = if cancel {
            Duration::from_secs(30)
        } else {
            Duration::from_millis(200)
        };
        let manager = CoreManager::spawn(options)?;
        let starter = manager.clone();
        let startup = tokio::spawn(async move { starter.start().await });
        let ready = timeout(Duration::from_secs(3), async {
            let path = directory.0.join("validator.pid");
            loop {
                if let Ok(pid) = tokio::fs::read_to_string(&path).await
                    && let Ok(pid) = pid.parse::<u32>()
                {
                    break pid;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if cancel {
            timeout(Duration::from_secs(3), manager.shutdown()).await??;
        }
        let started = timeout(Duration::from_secs(3), startup).await;
        let cleanup = manager.shutdown().await;
        cleanup?;
        ensure!(started??.is_err());
        reaped(ready?)?;
        let store = RuntimeStore::open(&directory.0)?;
        ensure!(store.state().current.is_none() && store.state().pending.is_none());
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires permission to bind an owned test Unix socket"]
async fn controlled_configuration_rolls_back_when_candidate_restart_fails() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = Directory::new().await?;
    let config = directory.config("rule").await?;
    let binary = directory.0.join("controlled-core.py");
    tokio::fs::write(&binary, include_str!("fixtures/mihomo.py")).await?;
    tokio::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).await?;
    let mut options = CoreOptions::new(binary, directory.0.clone(), config);
    options.policy.probe_interval = Duration::from_millis(10);
    let manager = CoreManager::spawn(options)?;
    let result = async {
        let initial = manager.start().await?;
        let failed = manager
            .apply_overlay(parse("mode: direct\nfixture-fail-start: true")?)
            .await;
        ensure!(format!("{:#}", failed.unwrap_err()).contains("exited before readiness"));
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_eq!(manager.status().config_revision, initial.config_revision);
        ensure!(manager.status().error.is_some());
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Rule);
        reaped(initial.pid.context("missing initial PID")?)?;
        let store = RuntimeStore::open(&directory.0)?;
        ensure!(store.state().pending.is_none());
        assert_eq!(store.read_current()?["mode"].as_str(), Some("rule"));
        // The failed overlay must not survive into the next valid application.
        manager.apply_overlay(parse("mode: direct")?).await?;
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Direct);
        ensure!(manager.status().error.is_none());
        let good = manager
            .import_profile(directory.config("rule").await?, Some("good".into()))
            .await?;
        let good_uid = good.uid.context("good UID missing")?.to_string();
        let working = manager.select_profile(good_uid.clone()).await?;
        let invalid = directory.0.join("failed-switch.yaml");
        tokio::fs::write(&invalid, "mode: direct\nfixture-fail-start: true").await?;
        let invalid = manager.import_profile(invalid, Some("fails at startup".into())).await?;
        ensure!(
            manager
                .select_profile(invalid.uid.context("failed UID missing")?.to_string())
                .await
                .is_err()
        );
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_eq!(manager.status().active_profile, working.active_profile);
        assert_eq!(manager.status().config_revision, working.config_revision);
        assert_eq!(manager.profiles().current.as_deref(), Some(good_uid.as_str()));
        assert_eq!(manager.client().get_base_config().await?.mode, ClashMode::Rule);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn validation_rejects_fatal_output_and_drains_both_pipes_with_bounded_capture() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    for (body, expected) in [
        (
            "import sys\nprint('level=fatal controlled failure', file=sys.stderr)",
            "rejected configuration",
        ),
        (
            "import sys\nsys.stdout.write('x'*100000)\nsys.stderr.write('y'*100000)",
            "exceeds 64 KiB",
        ),
    ] {
        let directory = Directory::new().await?;
        let config = directory.config("rule").await?;
        let binary = directory.0.join("invalid-validator.py");
        tokio::fs::write(&binary, format!("#!/usr/bin/python3\n{body}\n")).await?;
        tokio::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).await?;
        let manager = CoreManager::spawn(CoreOptions::new(binary, directory.0.clone(), config))?;
        let result = manager.start().await;
        manager.shutdown().await?;
        ensure!(format!("{:#}", result.unwrap_err()).contains(expected));
        let store = RuntimeStore::open(&directory.0)?;
        ensure!(store.state().current.is_none() && store.state().pending.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn shutdown_interrupts_readiness_and_forces_an_unresponsive_child_to_exit() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = Directory::new().await?;
    let config = directory.config("rule").await?;
    let binary = directory.0.join("unresponsive.py");
    tokio::fs::write(&binary, "#!/usr/bin/python3\nimport signal,time,sys\nif '-t' in sys.argv: sys.exit(0)\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\nfor i in range(300): print('fixture log', i, flush=True)\nprint('fixture ready', flush=True)\nwhile True: time.sleep(1)\n").await?;
    tokio::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).await?;
    let mut options = CoreOptions::new(binary, directory.0.clone(), config);
    options.policy.readiness_attempts = 100;
    options.policy.stop_timeout = Duration::from_millis(100);
    let manager = CoreManager::spawn(options)?;
    let starter = manager.clone();
    let startup = tokio::spawn(async move { starter.start().await });
    let ready = timeout(Duration::from_secs(3), async {
        while !manager.logs().iter().any(|line| line.message == "fixture ready") {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let pid = manager.status().pid;
    let shutdown = timeout(Duration::from_secs(3), manager.shutdown()).await;
    shutdown??;
    ready?;
    ensure!(startup.await?.is_err());
    reaped(pid.context("missing unresponsive child PID")?)?;
    assert_eq!(manager.status().phase, CorePhase::Shutdown);
    ensure!(
        manager
            .logs()
            .iter()
            .any(|line| line.message.contains("forcing termination"))
    );
    ensure!(manager.logs().len() <= 200);
    Ok(())
}
