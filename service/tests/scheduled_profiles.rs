//! Real loopback downloads exercise the automatic path without minute-long sleeps.
#![cfg(unix)]
use anyhow::{Context as _, Result};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::Response,
};
use headless_core::config::{
    IProfiles, PrfItem, PrfOption,
    profile_store::{ProfilePatch, ProfileStore, RemoteOptionsPatch},
};
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions},
    remote::RemoteOptions,
};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::Semaphore,
    task::JoinHandle,
    time::{sleep, timeout},
};
const YAML: &str = "proxies: []\nmode: rule\nmixed-port: 0\ndns: {enable: false}\nproxy-groups: [{name: Main, type: select, proxies: [DIRECT, REJECT]}]\nrules: ['MATCH,Main']\n";
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-scheduler-{}-{stamp:x}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn options(&self) -> Result<CoreOptions> {
        use std::os::unix::fs::PermissionsExt as _;
        let binary = self.0.join("validator.py");
        fs::write(
            &binary,
            "#!/usr/bin/python3\nimport sys\nsys.exit(0 if '-t' in sys.argv else 1)\n",
        )?;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
        Ok(CoreOptions::new(binary, self.0.clone(), self.0.join("missing.yaml")))
    }
    fn overdue(&self, uid: &str, automatic: bool) -> Result<()> {
        let path = self.0.join("profiles.yaml");
        let mut catalog: IProfiles = serde_yaml_ng::from_slice(&fs::read(&path)?)?;
        let item = catalog
            .items
            .as_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p.uid.as_deref() == Some(uid))
            .unwrap();
        item.updated = Some(1);
        item.option.as_mut().unwrap().allow_auto_update = Some(automatic);
        fs::write(path, serde_yaml_ng::to_string(&catalog)?)?;
        Ok(())
    }
    fn seed(&self, url: &str, option: PrfOption) -> Result<PrfItem> {
        let mut store = ProfileStore::open(&self.0)?;
        let source =
            headless_core::config::remote::from_response(&url::Url::parse(url)?, Some("Scheduled"), &[], YAML, option)?;
        let item = store.import_remote_with_defaults(source)?;
        drop(store);
        self.overdue(
            item.uid.as_deref().unwrap(),
            item.option.as_ref().unwrap().allow_auto_update != Some(false),
        )?;
        Ok(item)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Provider {
    requests: Mutex<Vec<String>>,
    response: Mutex<(StatusCode, String)>,
    hold: AtomicBool,
    release: Semaphore,
}
struct Fixture {
    base: String,
    state: Arc<Provider>,
    task: JoinHandle<std::io::Result<()>>,
}
impl Fixture {
    async fn new() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let state = Arc::new(Provider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new((StatusCode::OK, YAML.replace("mode: rule", "mode: direct"))),
            hold: AtomicBool::new(false),
            release: Semaphore::new(0),
        });
        let task = tokio::spawn(
            axum::serve(listener, Router::new().fallback(serve).with_state(Arc::clone(&state))).into_future(),
        );
        Ok(Self { base, state, task })
    }
    fn count(&self) -> usize {
        self.state.requests.lock().unwrap().len()
    }
    async fn wait(&self, count: usize) -> Result<()> {
        timeout(Duration::from_secs(5), async {
            while self.count() < count {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.state.release.add_permits(100);
        self.task.abort();
    }
}
async fn serve(State(state): State<Arc<Provider>>, request: Request) -> Response {
    assert!(!request.headers().contains_key("authorization"));
    assert!(!request.headers().contains_key("proxy-authorization"));
    state.requests.lock().unwrap().push(request.uri().to_string());
    if state.hold.load(Ordering::SeqCst) {
        state.release.acquire().await.unwrap().forget();
    }
    let (status, body) = state.response.lock().unwrap().clone();
    Response::builder().status(status).body(Body::from(body)).unwrap()
}
fn automatic(allow: Option<bool>) -> PrfOption {
    PrfOption {
        update_interval: Some(1),
        allow_auto_update: allow,
        ..Default::default()
    }
}
async fn log_count(manager: &CoreManager, word: &str, count: usize) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        while manager
            .logs()
            .iter()
            .filter(|line| line.stream == "scheduler" && line.message.contains(word))
            .count()
            < count
        {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}
fn patch(allow: bool) -> ProfilePatch {
    ProfilePatch {
        options: Some(RemoteOptionsPatch {
            allow_auto_update: Some(allow),
            ..Default::default()
        }),
        ..Default::default()
    }
}
#[tokio::test]
async fn overdue_updates_use_saved_options_and_links_and_new_timestamp_survives_restart() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    let item = dir.seed(&format!("{}/?token=provider-secret", fixture.base), automatic(None))?;
    let uid = item.uid.as_deref().unwrap().to_string();
    dir.seed(&fixture.base, automatic(Some(false)))?;
    dir.seed(
        &fixture.base,
        PrfOption {
            update_interval: Some(0),
            ..Default::default()
        },
    )?;
    dir.seed(
        &fixture.base,
        PrfOption {
            update_interval: Some(u64::MAX),
            ..Default::default()
        },
    )?;
    let options = dir.options()?;
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        fixture.wait(1).await?;
        log_count(&manager, "completed", 1).await?;
        let current = manager
            .profiles()
            .items
            .unwrap()
            .into_iter()
            .find(|p| p.uid.as_deref() == Some(&uid))
            .unwrap();
        assert_ne!(current.file, item.file);
        assert!(current.updated.unwrap() > 1);
        assert_eq!(current.url, item.url);
        assert_eq!(current.option, item.option);
        assert!(manager.profile_raw(uid.clone()).await?.yaml.contains("mode: direct"));
        assert_eq!(fixture.count(), 1);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)?;
    let restored = CoreManager::spawn(options)?;
    sleep(Duration::from_millis(100)).await;
    assert_eq!(fixture.count(), 1);
    restored.shutdown().await
}
#[tokio::test]
async fn failed_overdue_update_does_not_spin_and_interval_edit_rearms_it() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    *fixture.state.response.lock().unwrap() = (StatusCode::SERVICE_UNAVAILABLE, "private provider body".into());
    let item = dir.seed(&fixture.base, automatic(Some(true)))?;
    let uid = item.uid.as_deref().unwrap().to_string();
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        log_count(&manager, "failed", 1).await?;
        sleep(Duration::from_millis(200)).await;
        assert_eq!(fixture.count(), 1);
        let current = manager
            .profiles()
            .items
            .unwrap()
            .into_iter()
            .find(|p| p.uid.as_deref() == Some(&uid))
            .unwrap();
        assert_eq!(current.file, item.file);
        assert_eq!(current.updated, Some(1));
        assert!(
            manager
                .logs()
                .iter()
                .all(|line| !line.message.contains("private provider body"))
        );
        *fixture.state.response.lock().unwrap() = (StatusCode::OK, YAML.into());
        manager
            .edit_profile(
                uid,
                ProfilePatch {
                    options: Some(RemoteOptionsPatch {
                        update_interval: Some(2),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        log_count(&manager, "completed", 1).await?;
        assert_eq!(fixture.count(), 2);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
#[tokio::test]
async fn disable_reenable_inflight_keeps_one_worker_and_changed_options_reject_old_result() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    fixture.state.hold.store(true, Ordering::SeqCst);
    let item = dir.seed(&fixture.base, automatic(Some(true)))?;
    let uid = item.uid.as_deref().unwrap().to_string();
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        fixture.wait(1).await?;
        manager.edit_profile(uid.clone(), patch(false)).await?;
        manager.edit_profile(uid.clone(), patch(true)).await?;
        manager
            .edit_profile(
                uid.clone(),
                ProfilePatch {
                    options: Some(RemoteOptionsPatch {
                        update_interval: Some(2),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        sleep(Duration::from_millis(100)).await;
        assert_eq!(fixture.count(), 1);
        fixture.state.release.add_permits(1);
        log_count(&manager, "failed", 1).await?;
        assert_eq!(
            manager
                .profiles()
                .items
                .unwrap()
                .into_iter()
                .find(|p| p.uid.as_deref() == Some(&uid))
                .unwrap()
                .file,
            item.file
        );
        sleep(Duration::from_millis(100)).await;
        assert_eq!(fixture.count(), 1);
        manager.edit_profile(uid.clone(), patch(false)).await?;
        fixture.state.hold.store(false, Ordering::SeqCst);
        manager.refresh_profile(uid).await?;
        assert_eq!(fixture.count(), 2); // Manual updates remain allowed.
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
#[tokio::test]
async fn due_worker_admission_is_bounded_and_shutdown_drains_downloads_and_keeps_catalog() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    fixture.state.hold.store(true, Ordering::SeqCst);
    for _ in 0..6 {
        dir.seed(&fixture.base, automatic(Some(true)))?;
    }
    let manager = CoreManager::spawn(dir.options()?)?;
    let catalog = serde_json::to_value(manager.profiles())?;
    fixture.wait(4).await?;
    sleep(Duration::from_millis(100)).await;
    assert_eq!(fixture.count(), 4);
    timeout(Duration::from_secs(1), manager.stop()).await??;
    timeout(Duration::from_secs(1), manager.shutdown()).await??;
    assert_eq!(serde_json::to_value(manager.profiles())?, catalog);
    assert_eq!(fixture.count(), 4);
    Ok(())
}
#[tokio::test]
async fn scheduled_download_waiting_for_manual_admission_rechecks_disable_before_network() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    fixture.state.hold.store(true, Ordering::SeqCst);
    let mut manual = Vec::new();
    for _ in 0..4 {
        manual.push(
            dir.seed(&fixture.base, automatic(Some(false)))?
                .uid
                .unwrap()
                .to_string(),
        );
    }
    let scheduled = dir
        .seed(&fixture.base, automatic(Some(false)))?
        .uid
        .unwrap()
        .to_string();
    let manager = CoreManager::spawn(dir.options()?)?;
    let mut jobs = Vec::new();
    for uid in manual {
        let clone = manager.clone();
        jobs.push(tokio::spawn(async move { clone.refresh_profile(uid).await }));
    }
    let result = async {
        fixture.wait(4).await?;
        manager.edit_profile(scheduled.clone(), patch(true)).await?;
        log_count(&manager, "started", 1).await?;
        manager.edit_profile(scheduled.clone(), patch(false)).await?;
        fixture.state.release.add_permits(1);
        log_count(&manager, "failed", 1).await?;
        assert_eq!(fixture.count(), 4);
        manager.edit_profile(scheduled, patch(true)).await?;
        fixture.wait(5).await?;
        timeout(Duration::from_secs(1), manager.shutdown()).await??;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    for job in jobs {
        let _ = timeout(Duration::from_secs(1), job).await?;
    }
    result.and(cleanup)
}
#[tokio::test]
async fn dropping_last_manager_still_releases_actor_directory_ownership() -> Result<()> {
    let dir = Directory::new()?;
    let options = dir.options()?;
    drop(CoreManager::spawn(options.clone())?);
    let manager = timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(manager) = CoreManager::spawn(options.clone()) {
                break manager;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    manager.shutdown().await
}

#[tokio::test]
#[ignore = "requires real Mihomo, script workers and local proxy sockets"]
async fn scheduled_managed_active_refresh_preserves_selection_applies_transaction_and_rolls_back_invalid_candidate()
-> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let initial = YAML.replace("mixed-port: 0", &format!("mixed-port: {port}")).replace(
        "rules: ['MATCH,Main']",
        "rules: ['IP-CIDR,127.0.0.0/8,DIRECT,no-resolve', 'MATCH,Main']",
    );
    *fixture.state.response.lock().unwrap() = (StatusCode::OK, initial.clone());
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let mut options = CoreOptions::new(binary, dir.0.clone(), dir.0.join("missing.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let item = manager
            .import_remote_profile(
                fixture.base.clone(),
                Some("Active".into()),
                RemoteOptions {
                    update_interval: Some(1),
                    allow_auto_update: Some(false),
                    ..Default::default()
                },
            )
            .await?;
        let uid = item.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        manager
            .edit_profile(
                uid.clone(),
                ProfilePatch {
                    options: Some(RemoteOptionsPatch {
                        self_proxy: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        Ok::<_, anyhow::Error>(uid)
    }
    .await;
    let cleanup = manager.shutdown().await;
    cleanup?;
    let uid = result?;
    dir.overdue(&uid, true)?;
    *fixture.state.response.lock().unwrap() = (StatusCode::OK, initial.replace("mode: rule", "mode: direct"));
    fixture.state.hold.store(true, Ordering::SeqCst);
    let restored = CoreManager::spawn(options.clone())?;
    let result = async {
        restored.start().await?;
        let pid = restored.status().pid;
        fixture
            .wait(2)
            .await
            .context("managed automatic download did not reach fixture")?;
        fixture.state.release.add_permits(1);
        log_count(&restored, "completed", 1)
            .await
            .context("managed automatic refresh did not commit")?;
        assert_eq!(restored.status().pid, pid);
        assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("direct"));
        assert_eq!(
            restored.client().get_proxies().await?.proxies["Main"].now.as_deref(),
            Some("REJECT")
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result.and(cleanup)?;
    dir.overdue(&uid, true)?;
    fixture.state.hold.store(false, Ordering::SeqCst);
    *fixture.state.response.lock().unwrap() = (StatusCode::OK, initial.replace("MATCH,Main", "INVALID,DIRECT"));
    let restored = CoreManager::spawn(options)?;
    let result = async {
        restored.start().await?;
        let revision = restored.status().config_revision;
        let catalog = serde_json::to_value(restored.profiles())?;
        log_count(&restored, "failed", 1)
            .await
            .context("invalid scheduled candidate did not report failure")?;
        assert_eq!(restored.status().config_revision, revision);
        assert_eq!(serde_json::to_value(restored.profiles())?, catalog);
        assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("direct"));
        assert_eq!(
            restored.client().get_proxies().await?.proxies["Main"].now.as_deref(),
            Some("REJECT")
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn actual_scheduler_timer_retries_once_after_full_interval_without_waiting_a_real_minute() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    *fixture.state.response.lock().unwrap() = (StatusCode::SERVICE_UNAVAILABLE, "unavailable".into());
    dir.seed(&fixture.base, automatic(Some(true)))?;
    let manager = CoreManager::spawn(dir.options()?)?;
    log_count(&manager, "failed", 1).await?;
    // Keep a runnable test task while IO is pending, so paused Tokio time cannot
    // auto-advance past the deadline under test. No external processes are running.
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(59)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.count(), 1);
    *fixture.state.response.lock().unwrap() = (StatusCode::OK, YAML.into());
    tokio::time::advance(Duration::from_secs(2)).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !manager
        .logs()
        .iter()
        .any(|line| line.stream == "scheduler" && line.message.contains("completed"))
    {
        assert!(std::time::Instant::now() < deadline, "scheduled retry did not complete");
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.count(), 2);
    tokio::time::advance(Duration::from_secs(20)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.count(), 2);
    tokio::time::resume();
    manager.shutdown().await
}
