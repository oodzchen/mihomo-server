#![cfg(unix)]

use anyhow::{Context as _, Result, ensure};
use headless_core::config::{PrfSelected, profile_store::ProfileStore};
use mihomo_client::{Builder, models::Protocol};
use mihomo_server::core_manager::{CoreManager, CoreOptions, CorePhase};
use serde_json::json;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    process::Command,
    time::{sleep, timeout},
};

struct Directory(PathBuf);
impl Directory {
    async fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let dir = Self(std::env::temp_dir().join(format!("ms-nodes-{}-{stamp:x}", std::process::id())));
        tokio::fs::create_dir_all(&dir.0).await?;
        Ok(dir)
    }

    async fn source(&self) -> Result<PathBuf> {
        let path = self.0.join("source.yaml");
        tokio::fs::write(&path, serde_json::to_vec(&json!({
            "mixed-port": 0, "mode": "rule", "external-controller": "",
            "dns": {"enable": false}, "tun": {"enable": false},
            "profile": {"store-selected": false},
            "proxy-groups": [
                {"name": "Main", "type": "select", "proxies": ["DIRECT", "REJECT"]},
                {"name": "Automatic", "type": "url-test", "url": "http://127.0.0.1:1/", "interval": 86400, "proxies": ["DIRECT", "REJECT"]}
            ], "rules": ["MATCH,DIRECT"]
        }))?).await?;
        Ok(path)
    }

    fn options(&self, binary: PathBuf) -> CoreOptions {
        let mut options = CoreOptions::new(binary, self.0.clone(), self.0.join("missing-bootstrap.yaml"));
        options.policy.stop_timeout = Duration::from_millis(500);
        options.policy.recovery_backoff = Duration::from_millis(20);
        options.policy.selection_interval = Duration::from_millis(30);
        options
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn now(manager: &CoreManager, group: &str) -> Result<String> {
    manager
        .client()
        .get_proxies()
        .await?
        .proxies
        .get(group)
        .and_then(|proxy| proxy.now.clone())
        .context("group has no current node")
}

#[tokio::test]
#[ignore = "requires a real core and local Unix socket permissions"]
async fn live_node_selection_persists_per_profile_and_recovers_write_failures() -> Result<()> {
    let directory = Directory::new().await?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let source = directory.source().await?;
    let manager = CoreManager::spawn(directory.options(binary.clone()))?;
    let result = async {
        let first = manager.import_profile(source.clone(), Some("first".into())).await?;
        let first_uid = first.uid.context("first UID missing")?.to_string();
        manager.select_profile(first_uid.clone()).await?;
        manager.start().await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        assert_eq!(now(&manager, "Main").await?, "REJECT");
        assert_eq!(
            manager
                .profiles()
                .items
                .as_ref()
                .unwrap()
                .iter()
                .find(|item| matches!(item.itype.as_deref(), Some("local" | "remote")))
                .unwrap()
                .selected
                .as_ref()
                .unwrap()[0]
                .now
                .as_deref(),
            Some("REJECT")
        );
        let old = manager.profiles();
        ensure!(
            manager
                .select_node("Main".into(), "not-in-this-group".into())
                .await
                .is_err()
        );
        ensure!(manager.select_node("DIRECT".into(), "REJECT".into()).await.is_err());
        assert_eq!(
            serde_yaml_ng::to_value(manager.profiles())?,
            serde_yaml_ng::to_value(old)?
        );
        assert_eq!(now(&manager, "Main").await?, "REJECT");
        manager.restart().await?;
        assert_eq!(now(&manager, "Main").await?, "REJECT");
        ensure!(manager.status().selection_pending.is_empty());
        manager.select_node("Automatic".into(), "REJECT".into()).await?;
        assert_eq!(
            manager.client().get_proxies().await?.proxies["Automatic"]
                .fixed
                .as_deref(),
            Some("REJECT")
        );
        manager.unfix_node("Automatic".into()).await?;
        ensure!(
            manager.client().get_proxies().await?.proxies["Automatic"]
                .fixed
                .as_deref()
                .is_none_or(str::is_empty)
        );
        ensure!(
            manager
                .profiles()
                .items
                .as_ref()
                .unwrap()
                .iter()
                .find(|item| matches!(item.itype.as_deref(), Some("local" | "remote")))
                .unwrap()
                .selected
                .as_ref()
                .unwrap()
                .iter()
                .all(|entry| entry.name.as_deref() != Some("Automatic"))
        );
        let second = manager.import_profile(source.clone(), Some("second".into())).await?;
        let second_uid = second.uid.context("second UID missing")?.to_string();
        manager.select_profile(second_uid.clone()).await?;
        manager.select_node("Main".into(), "DIRECT".into()).await?;
        manager.select_profile(first_uid.clone()).await?;
        assert_eq!(now(&manager, "Main").await?, "REJECT");
        manager.select_profile(second_uid).await?;
        assert_eq!(now(&manager, "Main").await?, "DIRECT");
        manager.select_profile(first_uid.clone()).await?;
        let metadata = directory.0.join("profiles.yaml");
        let backup = directory.0.join("profiles-backup.yaml");
        tokio::fs::rename(&metadata, &backup).await?;
        tokio::fs::create_dir(&metadata).await?;
        let failed = manager.select_node("Main".into(), "DIRECT".into()).await;
        tokio::fs::remove_dir(&metadata).await?;
        tokio::fs::rename(&backup, &metadata).await?;
        ensure!(failed.is_err());
        assert_eq!(now(&manager, "Main").await?, "REJECT");
        assert_eq!(
            ProfileStore::open(&directory.0)?.selections(&first_uid)?[0]
                .now
                .as_deref(),
            Some("REJECT")
        );
        manager.restart().await?;
        assert_eq!(now(&manager, "Main").await?, "REJECT");
        Ok::<_, anyhow::Error>(first_uid)
    }
    .await;
    let cleanup = manager.shutdown().await;
    let uid = result?;
    cleanup?;
    tokio::fs::remove_file(source).await?;
    let manager = CoreManager::spawn(directory.options(binary))?;
    let result = async {
        manager.start().await?;
        assert_eq!(manager.status().active_profile.as_deref(), Some(uid.as_str()));
        assert_eq!(now(&manager, "Main").await?, "REJECT");
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires a real core and local Unix socket permissions"]
async fn live_cli_selection_is_saved_and_restored_without_the_core_cache() -> Result<()> {
    let directory = Directory::new().await?;
    let source = directory.source().await?;
    let binary = std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?;
    let socket = directory.0.join("run/core.sock");
    let client = Builder::new()
        .protocol(Protocol::LocalSocket)
        .socket_path(socket.to_str().context("socket path is not UTF-8")?)
        .build()?;
    for initial in [true, false] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mihomo-server"));
        command.arg("--mihomo").arg(&binary).arg("--data-dir").arg(&directory.0);
        command
            .arg("--listen")
            .arg(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.to_string());
        if initial {
            command
                .arg("--import-profile")
                .arg(&source)
                .arg("--select-node")
                .arg("Main")
                .arg("REJECT");
        }
        let mut service = command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let pid = service.id().context("service PID missing")?;
        let mut lines = BufReader::new(service.stdout.take().context("stdout missing")?).lines();
        let observed = timeout(Duration::from_secs(8), async {
            let core_pid = loop {
                let line = lines.next_line().await?.context("service exited before readiness")?;
                let state: serde_json::Value = serde_json::from_str(&line)?;
                if state["phase"] == "running" {
                    break state["pid"].as_u64().context("core PID missing")?;
                }
            };
            loop {
                let profiles = ProfileStore::open(&directory.0)?;
                let uid = profiles
                    .snapshot()
                    .current
                    .context("current profile missing")?
                    .to_string();
                let saved = profiles.selections(&uid)?;
                let snapshot = client.get_proxies().await?;
                if saved
                    .iter()
                    .any(|record| record.name.as_deref() == Some("Main") && record.now.as_deref() == Some("REJECT"))
                    && snapshot.proxies["Main"].now.as_deref() == Some("REJECT")
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
            Ok::<_, anyhow::Error>(core_pid)
        })
        .await;
        // Signal only the owned service and keep draining its state pipe during cleanup.
        ensure!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0);
        let (exit, drained) = tokio::join!(timeout(Duration::from_secs(5), service.wait()), async {
            while lines.next_line().await?.is_some() {}
            Ok::<(), anyhow::Error>(())
        });
        ensure!(exit??.success());
        drained?;
        let core_pid = observed??;
        ensure!(
            unsafe { libc::kill(core_pid as libc::pid_t, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH),
            "core was not reaped"
        );
        if initial {
            tokio::fs::remove_file(&source).await?;
        }
    }
    Ok(())
}

async fn controlled(directory: &Directory, flags: &str, selected: bool) -> Result<(CoreManager, String)> {
    use std::os::unix::fs::PermissionsExt as _;
    let binary = directory.0.join("controlled.py");
    tokio::fs::write(&binary, include_str!("fixtures/mihomo.py")).await?;
    tokio::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).await?;
    let source = directory.0.join("source.yaml");
    tokio::fs::write(&source, format!("mode: rule\nfixture-proxy-api: true\n{flags}\n")).await?;
    let mut options = directory.options(binary);
    options.policy.selection_first_pass = Duration::from_millis(200);
    options.policy.selection_timeout = Duration::from_millis(200);
    options.policy.selection_settle = Duration::from_millis(400);
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let profile = manager.import_profile(source, None).await?;
        let uid = profile.uid.context("UID missing")?.to_string();
        manager.select_profile(uid.clone()).await?;
        Ok::<_, anyhow::Error>(uid)
    }
    .await;
    let cleanup = manager.shutdown().await;
    let uid = result?;
    cleanup?;
    if selected {
        let mut store = ProfileStore::open(&directory.0)?;
        store.record_selection(&uid, "Main", "REJECT")?;
    }
    Ok((CoreManager::spawn(options)?, uid))
}

async fn settled(manager: &CoreManager) -> Result<()> {
    let mut status = manager.subscribe_status();
    timeout(Duration::from_secs(3), async {
        loop {
            if status.borrow_and_update().selection_pending.is_empty() {
                break;
            }
            status.changed().await?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated controlled Unix socket"]
async fn controlled_provider_loading_is_retried_and_manual_choice_supersedes_restore() -> Result<()> {
    for manual in [false, true] {
        let directory = Directory::new().await?;
        let flags = if manual {
            "fixture-empty-snapshots: 4"
        } else {
            "fixture-empty-snapshots: 2"
        };
        let (manager, uid) = controlled(&directory, flags, true).await?;
        let result = async {
            manager.start().await?;
            ensure!(!manager.status().selection_pending.is_empty());
            if manual {
                manager.select_node("Manual".into(), "REJECT".into()).await?;
                sleep(Duration::from_millis(150)).await;
                for _ in 0..3 {
                    let _ = manager.client().get_proxies().await?;
                }
                assert_eq!(now(&manager, "Main").await?, "DIRECT");
                assert_eq!(now(&manager, "Manual").await?, "REJECT");
                assert_eq!(
                    manager
                        .profiles()
                        .items
                        .as_ref()
                        .unwrap()
                        .iter()
                        .find(|item| matches!(item.itype.as_deref(), Some("local" | "remote")))
                        .unwrap()
                        .selected
                        .as_ref()
                        .unwrap()
                        .len(),
                    2
                );
            } else {
                settled(&manager).await?;
                assert_eq!(now(&manager, "Main").await?, "REJECT");
                ensure!(manager.status().selection_error.is_none());
            }
            assert_eq!(manager.status().active_profile.as_deref(), Some(uid.as_str()));
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = manager.shutdown().await;
        result.and(cleanup)?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated controlled Unix socket"]
async fn controlled_restore_deadline_keeps_startup_records_and_shutdown_cancels_query() -> Result<()> {
    for cancel in [false, true] {
        let directory = Directory::new().await?;
        let flags = if cancel {
            "fixture-proxy-delay: true"
        } else {
            "fixture-group-never-loads: true"
        };
        let (manager, uid) = controlled(&directory, flags, true).await?;
        let starter = manager.clone();
        let startup = tokio::spawn(async move { starter.start().await });
        let observed = timeout(Duration::from_secs(3), async {
            let mut states = manager.subscribe_status();
            loop {
                if !states.borrow_and_update().selection_pending.is_empty() {
                    break;
                }
                states.changed().await?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .await;
        let result = async {
            observed??;
            if cancel {
                timeout(Duration::from_secs(1), manager.shutdown()).await??;
                startup.await??;
                ensure!(manager.status().pid.is_none());
            } else {
                startup.await??;
                settled(&manager).await?;
                assert_eq!(manager.status().phase, CorePhase::Running);
                ensure!(
                    manager
                        .status()
                        .selection_error
                        .as_deref()
                        .is_some_and(|error| error.contains("deadline"))
                );
            }
            assert_eq!(
                ProfileStore::open(&directory.0)?.selections(&uid)?,
                vec![PrfSelected {
                    name: Some("Main".into()),
                    now: Some("REJECT".into())
                }]
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = manager.shutdown().await;
        result.and(cleanup)?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated controlled Unix socket"]
async fn controlled_failed_and_unconfirmed_node_changes_do_not_persist() -> Result<()> {
    for flags in ["fixture-select-rejected: true", "fixture-ignore-selection: true"] {
        let directory = Directory::new().await?;
        let (manager, uid) = controlled(&directory, flags, false).await?;
        let result = async {
            manager.start().await?;
            ensure!(manager.select_node("Main".into(), "REJECT".into()).await.is_err());
            assert_eq!(now(&manager, "Main").await?, "DIRECT");
            ensure!(ProfileStore::open(&directory.0)?.selections(&uid)?.is_empty());
            ensure!(manager.status().selection_error.is_some());
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = manager.shutdown().await;
        result.and(cleanup)?;
    }
    Ok(())
}
