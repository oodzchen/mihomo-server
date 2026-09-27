use anyhow::{Context as _, Result, ensure};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse as _, Response},
    routing::get,
};
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions},
    management::{
        Management,
        auth::Authentication,
        http::{HttpState, router},
    },
    remote::{RemoteOptions, download},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::watch,
    time::{sleep, timeout},
};
use tower::ServiceExt as _;
const YAML: &str = "# remote fixture\nmixed-port: 0\nmode: rule\nproxies: []\nproxy-groups: [{name: Main, type: select, proxies: [DIRECT, REJECT]}]\nprofile: {store-selected: false}\nrules: ['MATCH,Main']\n";

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY and local sockets"]
async fn subscription_controllers_are_ignored_on_selection_raw_edit_refresh_and_restart() -> Result<()> {
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    let manager = CoreManager::spawn(options.clone())?;
    let source = format!(
        "{YAML}external-controller: '127.0.0.1:9090'\nexternal-controller-tls: '0.0.0.0:9443'\nexternal-controller-unix: /tmp/source.sock\nexternal-controller-pipe: source-pipe\nsecret: subscription-secret\n"
    );
    let result = async {
        let local = manager
            .import_profile_yaml(source.clone(), "desktop export".into())
            .await?;
        let local_uid = local.uid.unwrap().to_string();
        manager.select_profile(local_uid.clone()).await?;
        manager.start().await?;
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Running);
        assert_eq!(manager.profile_raw(local_uid.clone()).await?.yaml, source);
        let original = manager.profile_raw(local_uid.clone()).await?;
        let edited = source.replace("mode: rule", "mode: direct");
        assert_eq!(
            manager
                .set_profile_raw(local_uid.clone(), original.revision, edited.clone())
                .await?
                .yaml,
            edited
        );
        let config = manager.runtime_config().await?;
        assert_eq!(config["mode"].as_str(), Some("direct"));
        assert!(!config.contains_key("external-controller"));
        let revision = manager.status().config_revision;
        assert!(
            manager
                .set_profile_merge(local_uid.clone(), Some("external-controller: '0.0.0.0:9099'".into()))
                .await
                .is_err()
        );
        assert_eq!(manager.status().config_revision, revision);
        *fixture.state.content.lock().unwrap() = (source.clone(), 2);
        let remote = manager
            .import_remote_profile(fixture.url("/refresh"), None, RemoteOptions::default())
            .await?;
        let uid = remote.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        *fixture.state.content.lock().unwrap() = (edited.clone(), 3);
        manager.refresh_profile(uid.clone()).await?;
        assert_eq!(manager.profile_raw(uid.clone()).await?.yaml, edited);
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("direct"));
        for field in [
            "external-controller",
            "external-controller-tls",
            "external-controller-unix",
            "external-controller-pipe",
        ] {
            assert!(!manager.runtime_config().await?.contains_key(field));
        }
        // Inactive raw edits also validate a candidate rather than source controllers.
        let inactive = manager.profile_raw(local_uid.clone()).await?;
        manager
            .set_profile_raw(local_uid, inactive.revision, source.clone())
            .await?;
        Ok::<_, anyhow::Error>(uid)
    }
    .await;
    let cleanup = manager.shutdown().await;
    let uid = result?;
    cleanup?;
    let restored = CoreManager::spawn(options)?;
    let result = async {
        restored.start().await?;
        assert_eq!(restored.status().active_profile.as_deref(), Some(uid.as_str()));
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

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires real Mihomo, script workers and local sockets"]
async fn linked_script_runs_after_merge_and_survives_refresh_failures_stopped_edits_and_restart() -> Result<()> {
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let mut options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let base = manager
            .import_remote_profile(
                fixture.url("/refresh"),
                Some("script profile".into()),
                RemoteOptions::default(),
            )
            .await?;
        let uid = base.uid.as_deref().unwrap().to_string();
        manager
            .set_profile_merge(uid.clone(), Some("mode: global".into()))
            .await?;
        let source = "function main(c,name) { console.info(name); c.mode='direct'; return c; }";
        manager.set_profile_script(uid.clone(), Some(source.into())).await?;
        assert!(manager.status().config_revision.is_none());
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("direct"));
        assert!(
            manager
                .logs()
                .iter()
                .any(|log| log.stream == "script" && log.message.contains("script profile"))
        );
        let raw = std::fs::read(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?;
        let catalog = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let pid = manager.status().pid;
        for source in [
            "function main( {",
            "function main(c) { console.warn('failure'); throw 'bad'; }",
            "function main(c) { c.rules=['INVALID,DIRECT']; return c; }",
            "function main(c) { c['external-controller']='0.0.0.0:9090'; return c; }",
        ] {
            assert!(
                manager
                    .set_profile_script(uid.clone(), Some(source.into()))
                    .await
                    .is_err()
            );
            assert_eq!(serde_json::to_value(manager.profiles())?, catalog);
            assert_eq!(manager.status().config_revision, revision);
            assert_eq!(manager.status().pid, pid);
        }
        let metadata = directory.0.join("profiles.yaml");
        let backup = directory.0.join("saved.yaml");
        std::fs::rename(&metadata, &backup)?;
        std::fs::create_dir(&metadata)?;
        let rejected = manager
            .set_profile_script(
                uid.clone(),
                Some("function main(c) { c.mode='rule'; return c; }".into()),
            )
            .await;
        std::fs::remove_dir(&metadata)?;
        std::fs::rename(&backup, &metadata)?;
        assert!(rejected.is_err());
        assert_eq!(serde_json::to_value(manager.profiles())?, catalog);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(
            manager.client().get_base_config().await?.mode,
            mihomo_client::models::ClashMode::Direct
        );
        assert_eq!(
            std::fs::read(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,
            raw
        );
        let script = manager.profile_script(uid.clone()).await?.uid;
        manager.refresh_profile(uid.clone()).await?;
        assert_eq!(manager.profile_script(uid.clone()).await?.uid, script);
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("direct"));
        assert_eq!(
            manager.client().get_proxies().await?.proxies["Main"].now.as_deref(),
            Some("REJECT")
        );
        manager.stop().await?;
        manager
            .set_profile_script(
                uid.clone(),
                Some("function main(c) { c.mode='rule'; return c; }".into()),
            )
            .await?;
        assert!(manager.status().pid.is_none());
        manager.shutdown().await?;
        let requests = fixture.state.requests.lock().unwrap().len();
        let restored = CoreManager::spawn(options)?;
        let result = async {
            restored.start().await?;
            assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("rule"));
            assert_eq!(fixture.state.requests.lock().unwrap().len(), requests);
            assert_eq!(
                restored.client().get_proxies().await?.proxies["Main"].now.as_deref(),
                Some("REJECT")
            );
            restored.set_profile_script(uid.clone(), None).await?;
            assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("global"));
            assert_eq!(restored.status().active_profile.as_deref(), Some(uid.as_str()));
            assert!(restored.profile_script(uid).await?.uid.is_none());
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = restored.shutdown().await;
        result.and(cleanup)
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires controlled core, script workers and local sockets"]
async fn script_candidate_restart_failure_rolls_back_and_shutdown_cancels_running_script() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = Directory::new()?;
    let binary = directory.0.join("controlled-core.py");
    std::fs::write(&binary, include_str!("fixtures/mihomo.py"))?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
    let mut options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options)?;
    let result = async {
        let base = manager.import_profile_yaml("mode: rule".into(), "base".into()).await?;
        let uid = base.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        let catalog = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let error = manager
            .set_profile_script(
                uid.clone(),
                Some("function main(c) { c['fixture-fail-start']=true; return c; }".into()),
            )
            .await
            .unwrap_err();
        ensure!(format!("{error:#}").contains("exited before readiness"));
        assert_eq!(serde_json::to_value(manager.profiles())?, catalog);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Running);
        let running = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .set_profile_script(uid, Some("function main(c) { while(true) {} }".into()))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        timeout(Duration::from_secs(2), manager.shutdown()).await??;
        assert!(running.await?.is_err());
        assert_eq!(serde_json::to_value(manager.profiles())?, catalog);
        assert!(!directory.0.join("profile-merge.yaml").exists());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY and local sockets"]
async fn linked_sequences_select_refresh_validate_rollback_restore_and_reject_stale_downloads() -> Result<()> {
    use headless_core::config::profile_store::SequenceKind::{Groups, Proxies, Rules};
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let base = manager
            .import_remote_profile(
                fixture.url("/refresh"),
                Some("sequence".into()),
                RemoteOptions::default(),
            )
            .await?;
        let uid = base.uid.as_deref().unwrap().to_string();
        for (kind, yaml) in [
            (
                Rules,
                "prepend: ['DOMAIN,sequence.test,AddedGroup']\nappend: []\ndelete: []",
            ),
            (
                Proxies,
                "prepend: [{name: Added, type: direct}]\nappend: []\ndelete: []",
            ),
            (
                Groups,
                "prepend: [{name: AddedGroup, type: select, proxies: [Added, DIRECT]}]\nappend: []\ndelete: []",
            ),
        ] {
            manager
                .set_profile_sequence(uid.clone(), kind, Some(yaml.into()))
                .await?;
        }
        assert!(manager.status().config_revision.is_none());
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        let config = manager.runtime_config().await?;
        assert_eq!(config["proxy-groups"][1]["proxies"][0].as_str(), Some("Added"));
        assert_eq!(
            manager.client().get_proxies().await?.proxies["Main"].now.as_deref(),
            Some("REJECT")
        );
        let raw = std::fs::read(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?;
        let saved = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let pid = manager.status().pid;
        assert!(
            manager
                .set_profile_sequence(
                    uid.clone(),
                    Rules,
                    Some("prepend: ['INVALID,DIRECT']\nappend: []\ndelete: []".into())
                )
                .await
                .is_err()
        );
        assert_eq!(serde_json::to_value(manager.profiles())?, saved);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.status().pid, pid);
        let metadata = directory.0.join("profiles.yaml");
        let backup = directory.0.join("saved.yaml");
        std::fs::rename(&metadata, &backup)?;
        std::fs::create_dir(&metadata)?;
        let rejected = manager
            .set_profile_sequence(
                uid.clone(),
                Rules,
                Some("prepend: ['DOMAIN,changed.test,DIRECT']\nappend: []\ndelete: []".into()),
            )
            .await;
        std::fs::remove_dir(&metadata)?;
        std::fs::rename(&backup, &metadata)?;
        assert!(rejected.is_err());
        assert_eq!(serde_json::to_value(manager.profiles())?, saved);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.runtime_config().await?, config);
        assert_eq!(
            std::fs::read(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,
            raw
        );
        let links = manager
            .profiles()
            .items
            .unwrap()
            .into_iter()
            .find(|item| matches!(item.itype.as_deref(), Some("local" | "remote")))
            .unwrap()
            .option
            .clone();
        *fixture.state.content.lock().unwrap() = (YAML.replace("# remote fixture", "# refreshed sequence base"), 88);
        manager.refresh_profile(uid.clone()).await?;
        assert_eq!(
            manager
                .profiles()
                .items
                .unwrap()
                .into_iter()
                .find(|item| matches!(item.itype.as_deref(), Some("local" | "remote")))
                .unwrap()
                .option,
            links
        );
        assert_eq!(
            manager.runtime_config().await?["rules"][0].as_str(),
            Some("DOMAIN,sequence.test,AddedGroup")
        );
        assert_eq!(
            manager.client().get_proxies().await?.proxies["Main"].now.as_deref(),
            Some("REJECT")
        );
        // Changing a linked option during an in-flight download supersedes that response.
        fixture.state.delay_next.store(true, Ordering::SeqCst);
        let requests = fixture.state.requests.lock().unwrap().len();
        let downloaded = {
            let manager = manager.clone();
            let uid = uid.clone();
            tokio::spawn(async move { manager.refresh_profile(uid).await })
        };
        fixture.wait_requests(requests + 1).await?;
        manager
            .set_profile_sequence(
                uid.clone(),
                Rules,
                Some("prepend: ['DOMAIN,new-sequence.test,DIRECT']\nappend: []\ndelete: []".into()),
            )
            .await?;
        let saved = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        fixture.state.release.add_permits(1);
        assert!(format!("{:#}", downloaded.await?.unwrap_err()).contains("changed during download"));
        assert_eq!(serde_json::to_value(manager.profiles())?, saved);
        assert_eq!(manager.status().config_revision, revision);
        manager.stop().await?;
        manager
            .set_profile_sequence(
                uid.clone(),
                Groups,
                Some("prepend: [{name: StoppedGroup, type: select, proxies: [DIRECT]}]\nappend: []\ndelete: []".into()),
            )
            .await?;
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Stopped);
        assert!(manager.status().pid.is_none());
        let saved = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        manager.shutdown().await?;
        let requests = fixture.state.requests.lock().unwrap().len();
        let restored = CoreManager::spawn(options)?;
        let result = async {
            restored.start().await?;
            assert_eq!(serde_json::to_value(restored.profiles())?, saved);
            assert_eq!(restored.status().config_revision, revision);
            assert_eq!(fixture.state.requests.lock().unwrap().len(), requests);
            assert_eq!(
                restored.client().get_proxies().await?.proxies["Main"].now.as_deref(),
                Some("REJECT")
            );
            for kind in [Rules, Groups, Proxies] {
                restored.set_profile_sequence(uid.clone(), kind, None).await?;
            }
            assert_eq!(
                restored.runtime_config().await?["rules"][0].as_str(),
                Some("MATCH,Main")
            );
            assert_eq!(restored.profiles().items.unwrap().len(), 3);
            assert_eq!(restored.status().active_profile.as_deref(), Some(uid.as_str()));
            assert_eq!(
                restored.client().get_proxies().await?.proxies["Main"].now.as_deref(),
                Some("REJECT")
            );
            assert!(!directory.0.join("profile-merge.yaml").exists());
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = restored.shutdown().await;
        result.and(cleanup)
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires controlled Unix sockets"]
async fn sequence_candidate_restart_failure_restores_previous_catalog_and_running_core() -> Result<()> {
    use headless_core::config::profile_store::SequenceKind;
    use std::os::unix::fs::PermissionsExt as _;
    let directory = Directory::new()?;
    let binary = directory.0.join("controlled-core.py");
    std::fs::write(&binary, include_str!("fixtures/mihomo.py"))?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
    let manager = CoreManager::spawn(CoreOptions::new(
        binary,
        directory.0.clone(),
        directory.0.join("missing.yaml"),
    ))?;
    let result = async {
        let base = manager
            .import_profile_yaml("mode: rule\nrules: ['MATCH,DIRECT']".into(), "base".into())
            .await?;
        let uid = base.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        let catalog = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let error = manager
            .set_profile_sequence(
                uid.clone(),
                SequenceKind::Rules,
                Some("prepend: ['DOMAIN,fail-start.test,DIRECT']\nappend: []\ndelete: []".into()),
            )
            .await
            .unwrap_err();
        ensure!(format!("{error:#}").contains("exited before readiness"));
        assert_eq!(serde_json::to_value(manager.profiles())?, catalog);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Running);
        assert!(manager.profile_sequence(uid, SequenceKind::Rules).await?.uid.is_none());
        assert!(!directory.0.join("profile-merge.yaml").exists());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = Self(std::env::temp_dir().join(format!("ms-remote-{}-{stamp:x}", std::process::id())));
        std::fs::create_dir_all(&directory.0)?;
        Ok(directory)
    }
    #[cfg(unix)]
    fn manager(&self) -> Result<CoreManager> {
        use std::os::unix::fs::PermissionsExt as _;
        let binary = self.0.join("validator.py");
        std::fs::write(
            &binary,
            "#!/usr/bin/python3\nimport sys\nsys.exit(0 if '-t' in sys.argv else 1)\n",
        )?;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
        CoreManager::spawn(CoreOptions::new(binary, self.0.clone(), self.0.join("missing.yaml")))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Clone)]
struct FixtureState {
    requests: Arc<Mutex<Vec<(String, HeaderMap)>>>,
    closing: watch::Receiver<bool>,
    content: Arc<Mutex<(String, u64)>>,
    delay_next: Arc<AtomicBool>,
    release: Arc<tokio::sync::Semaphore>,
}
struct Fixture {
    base: String,
    state: FixtureState,
    close: watch::Sender<bool>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Fixture {
    async fn new() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let (close, closing) = watch::channel(false);
        let state = FixtureState {
            requests: Arc::new(Mutex::new(Vec::new())),
            closing,
            content: Arc::new(Mutex::new((YAML.into(), 2))),
            delay_next: Arc::new(AtomicBool::new(false)),
            release: Arc::new(tokio::sync::Semaphore::new(0)),
        };
        let mut shutdown = close.subscribe();
        let task = tokio::spawn(
            axum::serve(
                listener,
                Router::new().route("/{*path}", get(serve)).with_state(state.clone()),
            )
            .with_graceful_shutdown(async move {
                if !*shutdown.borrow() {
                    let _ = shutdown.changed().await;
                }
            })
            .into_future(),
        );
        Ok(Self {
            base,
            state,
            close,
            task,
        })
    }
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
    async fn wait_requests(&self, count: usize) -> Result<()> {
        timeout(Duration::from_secs(5), async {
            while self.state.requests.lock().unwrap().len() < count {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.close.send_replace(true);
        self.task.abort();
    }
}
async fn serve(State(mut state): State<FixtureState>, request: Request) -> Response {
    state
        .requests
        .lock()
        .unwrap()
        .push((request.uri().to_string(), request.headers().clone()));
    let (body, used) = state.content.lock().unwrap().clone();
    if state.delay_next.swap(false, Ordering::SeqCst) {
        tokio::select! {
            permit = state.release.acquire() => { permit.unwrap().forget(); },
            _ = state.closing.changed() => {},
        }
    }
    match request.uri().path() {
        "/refresh" => (
            [
                (
                    "content-disposition",
                    "attachment; filename=provider-renamed.yaml".to_string(),
                ),
                (
                    "subscription-userinfo",
                    format!("upload=1; download={used}; total=100; expire=4"),
                ),
            ],
            body,
        )
            .into_response(),
        "/redirect" => (StatusCode::FOUND, [(header::LOCATION, "/ok")]).into_response(),
        "/error" => (StatusCode::SERVICE_UNAVAILABLE, "provider error").into_response(),
        "/missing" => "mode: direct".into_response(),
        "/invalid" => "<html>error</html>".into_response(),
        "/oversize-length" => Response::builder()
            .header(header::CONTENT_LENGTH, (8 * 1024 * 1024 + 1).to_string())
            .body(Body::from(vec![b' '; 8 * 1024 * 1024 + 1]))
            .unwrap(),
        "/oversize-stream" => Response::new(Body::from_stream(futures_util::stream::iter([
            Ok::<_, std::io::Error>(Bytes::from(vec![b' '; 8 * 1024 * 1024])),
            Ok(Bytes::from_static(b"x")),
        ]))),
        "/slow" => {
            tokio::select! {_=sleep(Duration::from_secs(60))=>{},_=state.closing.changed()=>{}};
            "proxies: []".into_response()
        }
        _ => (
            [
                ("content-disposition", "attachment; filename*=UTF-8''remote.yaml"),
                (
                    "x-obs-meta-subscription-userinfo",
                    "upload=1; download=2; total=100; expire=4",
                ),
                ("profile-update-interval", "2"),
                ("profile-web-page-url", "https://example.test/account"),
            ],
            format!("\u{feff}{YAML}"),
        )
            .into_response(),
    }
}

#[tokio::test]
async fn remote_options_reject_unsupported_modes_and_invalid_inputs_before_network() -> Result<()> {
    use headless_core::config::PrfOption;
    for option in [
        PrfOption {
            with_proxy: Some(true),
            ..Default::default()
        },
        PrfOption {
            self_proxy: Some(true),
            ..Default::default()
        },
        PrfOption {
            danger_accept_invalid_certs: Some(true),
            ..Default::default()
        },
    ] {
        assert!(RemoteOptions::from_profile(Some(&option)).is_err());
    }
    assert_eq!(
        RemoteOptions::from_profile(Some(&PrfOption {
            allow_auto_update: Some(false),
            ..Default::default()
        }))?
        .allow_auto_update,
        Some(false)
    );
    for options in [
        json!({"with_proxy":true}),
        json!({"self_proxy":true}),
        json!({"danger_accept_invalid_certs":true}),
        json!({"merge":"m1"}),
    ] {
        assert!(serde_json::from_value::<RemoteOptions>(options).is_err());
    }
    assert!(
        download("file:///private", None, RemoteOptions::default())
            .await
            .is_err()
    );
    assert!(
        download(
            "http://127.0.0.1:1",
            None,
            RemoteOptions {
                timeout_seconds: Some(0),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY and local sockets"]
async fn manual_refresh_preserves_uid_nodes_and_recovers_active_validation_failures() -> Result<()> {
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let local = manager.import_profile_yaml(YAML.into(), "local".into()).await?;
        let local_uid = local.uid.unwrap().to_string();
        manager.select_profile(local_uid.clone()).await?;
        manager.start().await?;
        let remote = manager
            .import_remote_profile(
                fixture.url("/refresh"),
                Some("retained title".into()),
                RemoteOptions {
                    user_agent: Some("refresh-agent".into()),
                    allow_auto_update: Some(false),
                    ..Default::default()
                },
            )
            .await?;
        let uid = remote.uid.as_deref().unwrap().to_string();
        let requests = fixture.state.requests.lock().unwrap().len();
        assert!(manager.refresh_profile(local_uid).await.is_err());
        assert!(manager.refresh_profile("unknown".into()).await.is_err());
        assert_eq!(fixture.state.requests.lock().unwrap().len(), requests);
        let prior_revision = manager.status().config_revision;
        let pid = manager.status().pid;
        *fixture.state.content.lock().unwrap() = (YAML.replace("mode: rule", "mode: direct"), 20);
        let item = manager.refresh_profile(uid.clone()).await?;
        assert_eq!(item.uid, remote.uid);
        assert_ne!(item.file, remote.file);
        assert_eq!(item.name.as_deref(), Some("retained title"));
        assert_eq!(item.extra.unwrap().download, 20);
        assert_eq!(manager.status().config_revision, prior_revision);
        assert_eq!(manager.status().pid, pid);
        assert_eq!(manager.profiles().items.unwrap().len(), 4);
        manager.select_profile(uid.clone()).await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        *fixture.state.content.lock().unwrap() = (YAML.into(), 30);
        let before = manager.status().config_revision;
        let updated = manager.refresh_profile(uid.clone()).await?;
        assert_ne!(manager.status().config_revision, before);
        assert_eq!(manager.status().pid, pid);
        assert_eq!(updated.selected.unwrap()[0].now.as_deref(), Some("REJECT"));
        assert_eq!(
            manager.client().get_proxies().await?.proxies["Main"].now.as_deref(),
            Some("REJECT")
        );
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("rule"));
        let saved = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let catalog = std::fs::read(directory.0.join("profiles.yaml"))?;
        *fixture.state.content.lock().unwrap() = ("proxies: []\nrules: ['INVALID,DIRECT']\n".into(), 99);
        let error = manager.refresh_profile(uid.clone()).await.unwrap_err();
        ensure!(
            format!("{error:#}").contains("Mihomo rejected configuration"),
            "unexpected validation error: {error:#}"
        );
        assert_eq!(serde_json::to_value(manager.profiles())?, saved);
        assert_eq!(std::fs::read(directory.0.join("profiles.yaml"))?, catalog);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.status().pid, pid);
        assert!(!directory.0.join("profile-refresh.yaml").exists());
        assert!(
            headless_core::config::runtime::RuntimeStore::open(&directory.0)?
                .state()
                .pending
                .is_none()
        );
        // Failure after real reload must restore content, metadata and the old core config.
        *fixture.state.content.lock().unwrap() = (YAML.replace("mode: rule", "mode: direct"), 70);
        let metadata = directory.0.join("profiles.yaml");
        let backup = directory.0.join("saved-catalog.yaml");
        std::fs::rename(&metadata, &backup)?;
        std::fs::create_dir(&metadata)?;
        let rejected = manager.refresh_profile(uid.clone()).await;
        std::fs::remove_dir(&metadata)?;
        std::fs::rename(&backup, &metadata)?;
        assert!(rejected.is_err());
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("rule"));
        assert_eq!(serde_json::to_value(manager.profiles())?, saved);
        assert_eq!(
            manager.client().get_base_config().await?.mode,
            mihomo_client::models::ClashMode::Rule
        );
        assert!(!directory.0.join("profile-refresh.yaml").exists());
        // A node changed during download is included, instead of the stale snapshot record.
        fixture.state.delay_next.store(true, Ordering::SeqCst);
        let refresher = manager.clone();
        let refreshing_uid = uid.clone();
        let count = fixture.state.requests.lock().unwrap().len();
        let pending = tokio::spawn(async move { refresher.refresh_profile(refreshing_uid).await });
        fixture.wait_requests(count + 1).await?;
        manager.select_node("Main".into(), "DIRECT".into()).await?;
        fixture.state.release.add_permits(1);
        let refreshed = timeout(Duration::from_secs(10), pending).await???;
        assert_eq!(refreshed.selected.unwrap()[0].now.as_deref(), Some("DIRECT"));
        manager.select_node("Main".into(), "REJECT".into()).await?;
        manager.stop().await?;
        *fixture.state.content.lock().unwrap() = (YAML.replace("mode: rule", "mode: direct"), 40);
        manager.refresh_profile(uid.clone()).await?;
        assert!(manager.status().pid.is_none());
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Stopped);
        let saved = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        manager.shutdown().await?;
        let requests = fixture.state.requests.lock().unwrap().len();
        let restored = CoreManager::spawn(options)?;
        let result = async {
            restored.start().await?;
            assert_eq!(restored.status().active_profile.as_deref(), Some(uid.as_str()));
            assert_eq!(restored.status().config_revision, revision);
            assert_eq!(serde_json::to_value(restored.profiles())?, saved);
            // Readiness and node restoration are asynchronous; a version response
            // does not imply the group snapshot has already been populated.
            timeout(Duration::from_secs(10), async {
                loop {
                    let proxies = restored.client().get_proxies().await?;
                    if proxies.proxies.get("Main").and_then(|group| group.now.as_deref()) == Some("REJECT") {
                        break Ok::<_, anyhow::Error>(());
                    }
                    sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .context("restored group selection did not converge")??;
            assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("direct"));
            assert_eq!(fixture.state.requests.lock().unwrap().len(), requests);
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = restored.shutdown().await;
        result.and(cleanup)
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires local TCP socket permissions"]
async fn concurrent_refresh_rejects_stale_content_and_cancels_download() -> Result<()> {
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let result = async {
        let remote = manager
            .import_remote_profile(fixture.url("/refresh"), None, RemoteOptions::default())
            .await?;
        let uid = remote.uid.unwrap().to_string();
        fixture.state.delay_next.store(true, Ordering::SeqCst);
        let stale_manager = manager.clone();
        let stale_uid = uid.clone();
        let stale = tokio::spawn(async move { stale_manager.refresh_profile(stale_uid).await });
        fixture.wait_requests(2).await?;
        timeout(Duration::from_secs(1), manager.stop()).await??;
        *fixture.state.content.lock().unwrap() = (YAML.replace("mode: rule", "mode: direct"), 88);
        let updated = manager.refresh_profile(uid.clone()).await?;
        fixture.state.release.add_permits(1);
        let error = timeout(Duration::from_secs(2), stale).await??.unwrap_err();
        ensure!(format!("{error:#}").contains("changed during download"));
        assert_eq!(
            manager
                .profiles()
                .items
                .unwrap()
                .into_iter()
                .find(|item| matches!(item.itype.as_deref(), Some("local" | "remote")))
                .unwrap()
                .file,
            updated.file
        );
        fixture.state.delay_next.store(true, Ordering::SeqCst);
        let cancelled_manager = manager.clone();
        let cancelled = tokio::spawn(async move { cancelled_manager.refresh_profile(uid).await });
        fixture.wait_requests(4).await?;
        timeout(Duration::from_secs(1), manager.shutdown()).await??;
        assert!(timeout(Duration::from_secs(1), cancelled).await??.is_err());
        assert_eq!(
            manager
                .profiles()
                .items
                .unwrap()
                .into_iter()
                .find(|item| matches!(item.itype.as_deref(), Some("local" | "remote")))
                .unwrap()
                .file,
            updated.file
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY and local sockets"]
async fn metadata_edits_and_deletion_preserve_runtime_and_supersede_inflight_refreshes() -> Result<()> {
    use headless_core::config::profile_store::{ProfilePatch, RemoteOptionsPatch};
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let local = manager.import_profile_yaml(YAML.into(), "local".into()).await?;
        let local_uid = local.uid.as_deref().unwrap().to_string();
        let remote = manager
            .import_remote_profile(fixture.url("/refresh"), Some("remote".into()), RemoteOptions::default())
            .await?;
        let uid = remote.uid.as_deref().unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        let revision = manager.status().config_revision;
        let pid = manager.status().pid;
        let requests = fixture.state.requests.lock().unwrap().len();
        let edited = manager
            .edit_profile(
                uid.clone(),
                ProfilePatch {
                    name: Some("edited remote".into()),
                    desc: Some("retained description".into()),
                    options: Some(RemoteOptionsPatch {
                        user_agent: Some("edit-agent".into()),
                        timeout_seconds: Some(5),
                        allow_auto_update: Some(false),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(edited.file, remote.file);
        assert_eq!(edited.selected.unwrap()[0].now.as_deref(), Some("REJECT"));
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.status().pid, pid);
        assert_eq!(fixture.state.requests.lock().unwrap().len(), requests);
        assert!(manager.delete_profile(uid.clone()).await.is_err());
        manager.stop().await?;
        assert!(manager.delete_profile(uid.clone()).await.is_err());
        manager.start().await?;
        // A title/description edit while fetching is preserved at refresh commit.
        fixture.state.delay_next.store(true, Ordering::SeqCst);
        let clone = manager.clone();
        let refresh_uid = uid.clone();
        let pending = tokio::spawn(async move { clone.refresh_profile(refresh_uid).await });
        fixture.wait_requests(requests + 1).await?;
        manager
            .edit_profile(
                uid.clone(),
                ProfilePatch {
                    name: Some("latest title".into()),
                    ..Default::default()
                },
            )
            .await?;
        fixture.state.release.add_permits(1);
        let result = timeout(Duration::from_secs(10), pending).await???;
        assert_eq!(result.name.as_deref(), Some("latest title"));
        assert_eq!(result.desc.as_deref(), Some("retained description"));
        assert_eq!(
            fixture.state.requests.lock().unwrap().last().unwrap().1[header::USER_AGENT],
            "edit-agent"
        );
        // Editing URL/options makes the old download stale without changing raw content/runtime.
        fixture.state.delay_next.store(true, Ordering::SeqCst);
        let clone = manager.clone();
        let refresh_uid = uid.clone();
        let requests = fixture.state.requests.lock().unwrap().len();
        let pending = tokio::spawn(async move { clone.refresh_profile(refresh_uid).await });
        fixture.wait_requests(requests + 1).await?;
        manager
            .edit_profile(
                uid.clone(),
                ProfilePatch {
                    url: Some(fixture.url("/refresh?updated=1")),
                    ..Default::default()
                },
            )
            .await?;
        let saved = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        fixture.state.release.add_permits(1);
        assert!(timeout(Duration::from_secs(10), pending).await??.is_err());
        assert_eq!(serde_json::to_value(manager.profiles())?, saved);
        assert_eq!(manager.status().config_revision, revision);
        manager.select_profile(local_uid.clone()).await?;
        let revision = manager.status().config_revision;
        // Deletion while fetching cannot reimport or reactivate the removed UID.
        fixture.state.delay_next.store(true, Ordering::SeqCst);
        let clone = manager.clone();
        let refresh_uid = uid.clone();
        let requests = fixture.state.requests.lock().unwrap().len();
        let pending = tokio::spawn(async move { clone.refresh_profile(refresh_uid).await });
        fixture.wait_requests(requests + 1).await?;
        let file = manager
            .profiles()
            .items
            .unwrap()
            .iter()
            .find(|item| item.uid.as_deref() == Some(uid.as_str()))
            .unwrap()
            .file
            .clone()
            .unwrap();
        let catalog = manager.delete_profile(uid.clone()).await?;
        assert_eq!(catalog.items.as_ref().unwrap().len(), 3);
        assert!(!directory.0.join("profiles").join(file.as_str()).exists());
        fixture.state.release.add_permits(1);
        assert!(timeout(Duration::from_secs(10), pending).await??.is_err());
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.status().active_profile.as_deref(), Some(local_uid.as_str()));
        manager.shutdown().await?;
        let restored = CoreManager::spawn(options)?;
        let result = async {
            restored.start().await?;
            assert_eq!(
                serde_json::to_value(restored.profiles())?,
                serde_json::to_value(catalog)?
            );
            assert_eq!(restored.status().config_revision, revision);
            assert!(restored.refresh_profile(uid).await.is_err());
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = restored.shutdown().await;
        result.and(cleanup)
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY and local sockets"]
async fn linked_merge_applies_to_selection_and_refresh_and_rolls_back_invalid_or_failed_updates() -> Result<()> {
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let base = manager
            .import_remote_profile(fixture.url("/refresh"), Some("merged".into()), RemoteOptions::default())
            .await?;
        let uid = base.uid.as_deref().unwrap().to_string();
        manager
            .set_profile_merge(uid.clone(), Some("# linked merge\nmode: direct".into()))
            .await?;
        assert!(manager.status().config_revision.is_none());
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("direct"));
        let raw = std::fs::read(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?;
        let saved = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let pid = manager.status().pid;
        let merge = manager.profile_merge(uid.clone()).await?.uid;
        assert!(
            manager
                .set_profile_merge(uid.clone(), Some("rules: ['INVALID,DIRECT']".into()))
                .await
                .is_err()
        );
        assert_eq!(serde_json::to_value(manager.profiles())?, saved);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.status().pid, pid);
        assert!(!directory.0.join("profile-merge.yaml").exists());
        let metadata = directory.0.join("profiles.yaml");
        let backup = directory.0.join("saved.yaml");
        std::fs::rename(&metadata, &backup)?;
        std::fs::create_dir(&metadata)?;
        let rejected = manager.set_profile_merge(uid.clone(), Some("mode: rule".into())).await;
        std::fs::remove_dir(&metadata)?;
        std::fs::rename(&backup, &metadata)?;
        assert!(rejected.is_err());
        assert_eq!(serde_json::to_value(manager.profiles())?, saved);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(
            manager.client().get_base_config().await?.mode,
            mihomo_client::models::ClashMode::Direct
        );
        assert_eq!(
            std::fs::read(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,
            raw
        );
        *fixture.state.content.lock().unwrap() = (YAML.replace("# remote fixture", "# downloaded again"), 55);
        manager.refresh_profile(uid.clone()).await?;
        assert_eq!(manager.profile_merge(uid.clone()).await?.uid, merge);
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("direct"));
        assert_eq!(
            manager.client().get_proxies().await?.proxies["Main"].now.as_deref(),
            Some("REJECT")
        );
        manager.stop().await?;
        manager
            .set_profile_merge(uid.clone(), Some("mode: global".into()))
            .await?;
        assert!(manager.status().pid.is_none());
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Stopped);
        let saved = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        manager.shutdown().await?;
        let count = fixture.state.requests.lock().unwrap().len();
        let restored = CoreManager::spawn(options)?;
        let result = async {
            restored.start().await?;
            assert_eq!(serde_json::to_value(restored.profiles())?, saved);
            assert_eq!(restored.status().config_revision, revision);
            assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("global"));
            assert_eq!(fixture.state.requests.lock().unwrap().len(), count);
            restored.set_profile_merge(uid.clone(), None).await?;
            assert!(restored.profile_merge(uid.clone()).await?.uid.is_none());
            assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("rule"));
            assert_eq!(
                restored.client().get_proxies().await?.proxies["Main"].now.as_deref(),
                Some("REJECT")
            );
            assert_eq!(restored.status().active_profile.as_deref(), Some(uid.as_str()));
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = restored.shutdown().await;
        result.and(cleanup)
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires controlled Unix sockets"]
async fn merge_transaction_rolls_back_when_candidate_reload_and_restart_fail() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = Directory::new()?;
    let binary = directory.0.join("controlled-core.py");
    std::fs::write(&binary, include_str!("fixtures/mihomo.py"))?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
    let manager = CoreManager::spawn(CoreOptions::new(
        binary,
        directory.0.clone(),
        directory.0.join("missing.yaml"),
    ))?;
    let result = async {
        let base = manager.import_profile_yaml("mode: rule".into(), "base".into()).await?;
        let uid = base.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        let saved = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let error = manager
            .set_profile_merge(uid.clone(), Some("mode: direct\nfixture-fail-start: true".into()))
            .await
            .unwrap_err();
        ensure!(format!("{error:#}").contains("exited before readiness"));
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Running);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(serde_json::to_value(manager.profiles())?, saved);
        assert!(manager.profile_merge(uid).await?.uid.is_none());
        assert!(!directory.0.join("profile-merge.yaml").exists());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires controlled Unix and local TCP sockets"]
async fn active_refresh_rolls_back_raw_content_when_reload_and_candidate_restart_fail() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let binary = directory.0.join("controlled-core.py");
    std::fs::write(&binary, include_str!("fixtures/mihomo.py"))?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
    let manager = CoreManager::spawn(CoreOptions::new(
        binary,
        directory.0.clone(),
        directory.0.join("missing.yaml"),
    ))?;
    let result = async {
        let item = manager
            .import_remote_profile(fixture.url("/refresh"), None, RemoteOptions::default())
            .await?;
        let uid = item.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        let before = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let initial_pid = manager.status().pid.unwrap();
        *fixture.state.content.lock().unwrap() = ("proxies: []\nmode: direct\nfixture-fail-start: true\n".into(), 99);
        let error = manager.refresh_profile(uid).await.unwrap_err();
        ensure!(format!("{error:#}").contains("exited before readiness"));
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Running);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(
            manager.client().get_base_config().await?.mode,
            mihomo_client::models::ClashMode::Rule
        );
        assert_eq!(serde_json::to_value(manager.profiles())?, before);
        assert!(!directory.0.join("profile-refresh.yaml").exists());
        assert_eq!(unsafe { libc::kill(initial_pid as i32, 0) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires local TCP socket permissions"]
async fn authenticated_remote_import_retains_upstream_metadata_without_activating() -> Result<()> {
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let auth = Authentication::load_or_create(&directory.0.join("management-token"), "127.0.0.1:9090".parse()?, None)?;
    let token = std::fs::read_to_string(directory.0.join("management-token"))?;
    let app = router(HttpState::new(Management::new(manager.clone(), auth)));
    let result = async {
        let payload = json!({
            "command": "import_remote_profile",
            "url": fixture.url("/redirect?token=secret"),
            "options": {"user_agent": "fixture-agent", "allow_auto_update": false}
        });
        let request = |authorized: bool, payload: &Value| {
            let mut builder = axum::http::Request::builder()
                .method("POST")
                .uri("/api/commands")
                .header(header::HOST, "127.0.0.1:9090")
                .header(header::CONTENT_TYPE, "application/json");
            if authorized {
                builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", token.trim()));
            }
            builder.body(Body::from(payload.to_string())).unwrap()
        };
        assert_eq!(
            app.clone().oneshot(request(false, &payload)).await?.status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(fixture.state.requests.lock().unwrap().is_empty());
        let response = app.clone().oneshot(request(true, &payload)).await?;
        ensure!(response.status().is_success(), "remote HTTP import rejected");
        let item: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1024 * 1024).await?)?;
        assert_eq!(item["type"], "remote");
        assert!(item["uid"].as_str().unwrap().starts_with('R'));
        assert_eq!(item["name"], "remote.yaml");
        assert_eq!(item["extra"]["download"], 2);
        assert_eq!(item["option"]["update_interval"], 120);
        assert_eq!(item["option"]["allow_auto_update"], false);
        assert_eq!(item["url"], fixture.url("/redirect?token=secret"));
        assert_eq!(item["home"], "https://example.test/account");
        assert!(manager.status().active_profile.is_none());
        assert_eq!(manager.profiles().items.unwrap().len(), 3);
        {
            let requests = fixture.state.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            for (_, headers) in requests.iter() {
                assert!(!headers.contains_key(header::AUTHORIZATION));
                assert_eq!(headers[header::USER_AGENT], "fixture-agent");
            }
        }
        let refresh = json!({"command": "refresh_profile", "uid": item["uid"]});
        assert_eq!(
            app.clone().oneshot(request(false, &refresh)).await?.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
        let response = app.clone().oneshot(request(true, &refresh)).await?;
        ensure!(response.status().is_success(), "remote HTTP refresh rejected");
        let updated: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1024 * 1024).await?)?;
        assert_eq!(updated["uid"], item["uid"]);
        assert_ne!(updated["file"], item["file"]);
        assert_eq!(manager.profiles().items.unwrap().len(), 3);
        let unknown = json!({"command": "refresh_profile", "uid": "missing"});
        assert!(
            !app.clone()
                .oneshot(request(true, &unknown))
                .await?
                .status()
                .is_success()
        );
        let unsupported = json!({"command": "refresh_profile", "uid": item["uid"], "options": {}});
        assert!(
            !app.clone()
                .oneshot(request(true, &unsupported))
                .await?
                .status()
                .is_success()
        );
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 4);
        let basic_url = fixture.url("/ok").replacen("http://", "http://fixture:pass@", 1);
        download(&basic_url, None, RemoteOptions::default()).await?;
        let requests = fixture.state.requests.lock().unwrap();
        assert_eq!(
            requests.last().unwrap().1[header::AUTHORIZATION],
            "Basic Zml4dHVyZTpwYXNz"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires local TCP socket permissions"]
async fn failed_remote_downloads_preserve_catalog_runtime_and_redact_url_secrets() -> Result<()> {
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let result = async {
        let item = manager.import_profile_yaml(YAML.into(), "retained".into()).await?;
        manager.select_profile(item.uid.unwrap().to_string()).await?;
        let prior = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        for path in [
            "/error",
            "/missing",
            "/invalid",
            "/oversize-length",
            "/oversize-stream",
            "/slow",
        ] {
            let error = manager
                .import_remote_profile(
                    fixture.url(&format!("{path}?token=secret-value")),
                    None,
                    RemoteOptions {
                        timeout_seconds: Some(1),
                        ..Default::default()
                    },
                )
                .await
                .unwrap_err();
            ensure!(!format!("{error:#}").contains("secret-value"), "URL secret leaked");
            if path.starts_with("/oversize") {
                ensure!(
                    format!("{error:#}").contains("8 MiB"),
                    "download size boundary was not enforced for {path}: {error:#}"
                );
            }
            assert_eq!(serde_json::to_value(manager.profiles())?, prior);
            assert_eq!(manager.status().config_revision, revision);
            assert_eq!(std::fs::read_dir(directory.0.join("profiles"))?.count(), 3);
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires local TCP socket permissions"]
async fn stalled_downloads_are_bounded_do_not_block_lifecycle_and_cancel_on_shutdown() -> Result<()> {
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let manager = directory.manager()?;
    let mut tasks = Vec::new();
    for _ in 0..5 {
        let manager = manager.clone();
        let url = fixture.url("/slow");
        tasks.push(tokio::spawn(async move {
            manager.import_remote_profile(url, None, RemoteOptions::default()).await
        }));
    }
    let result = async {
        fixture.wait_requests(4).await?;
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 4);
        timeout(Duration::from_secs(1), manager.stop()).await??;
        timeout(Duration::from_secs(1), manager.runtime_config())
            .await?
            .unwrap_err();
        timeout(Duration::from_secs(1), manager.shutdown()).await??;
        for task in tasks {
            assert!(timeout(Duration::from_secs(1), task).await??.is_err());
        }
        assert_eq!(manager.profiles().items.unwrap_or_default().len(), 2);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY and local sockets"]
async fn real_core_activates_and_restores_a_downloaded_profile_without_refetching() -> Result<()> {
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let item = manager
            .import_remote_profile(fixture.url("/ok"), Some("live remote".into()), RemoteOptions::default())
            .await?;
        let uid = item.uid.context("downloaded UID missing")?.to_string();
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        let saved = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        assert!(
            manager
                .import_remote_profile(fixture.url("/error"), None, RemoteOptions::default())
                .await
                .is_err()
        );
        assert_eq!(serde_json::to_value(manager.profiles())?, saved);
        assert_eq!(manager.status().config_revision, revision);
        let pid = manager.status().pid.context("owned core PID missing")?;
        manager.shutdown().await?;
        ensure!(
            unsafe { libc::kill(pid as i32, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH),
            "owned remote core not reaped"
        );
        let request_count = fixture.state.requests.lock().unwrap().len();
        let restored = CoreManager::spawn(options)?;
        let result = async {
            restored.start().await?;
            timeout(Duration::from_secs(10), async {
                loop {
                    if restored.client().get_proxies().await?.proxies["Main"].now.as_deref() == Some("REJECT") {
                        break Ok::<_, anyhow::Error>(());
                    }
                    sleep(Duration::from_millis(20)).await;
                }
            })
            .await??;
            assert_eq!(restored.status().active_profile.as_deref(), Some(uid.as_str()));
            assert_eq!(restored.status().config_revision, revision);
            assert_eq!(fixture.state.requests.lock().unwrap().len(), request_count);
            assert_eq!(serde_json::to_value(restored.profiles())?, saved);
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = restored.shutdown().await;
        result.and(cleanup)
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires real Mihomo, global script workers and local sockets"]
async fn global_stages_order_refresh_detach_and_failed_regeneration_preserve_committed_runtime() -> Result<()> {
    use headless_core::config::profile_store::{ProfileStore, SequenceKind};
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let mut profiles = ProfileStore::open(&directory.0)?;
    profiles.ensure_global_defaults()?;
    let merge_file = directory
        .0
        .join("profiles")
        .join(profiles.get_item("Merge")?.file.as_deref().unwrap());
    let script_file = directory
        .0
        .join("profiles")
        .join(profiles.get_item("Script")?.file.as_deref().unwrap());
    std::fs::write(&merge_file, "mode: global")?;
    let global_script = "function main(c,name) { console.info('global '+name); c.pipeline=(c.pipeline||'')+c.mode+';'; c.mode='direct'; return c; }";
    std::fs::write(&script_file, global_script)?;
    drop(profiles);
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let mut options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let base = manager.import_remote_profile(fixture.url("/refresh"), Some("global profile".into()), RemoteOptions::default()).await?;
        let uid = base.uid.as_deref().unwrap().to_owned();
        manager.select_profile(uid.clone()).await?;
        assert_eq!(manager.runtime_config().await?["pipeline"].as_str(), Some("global;global;"));
        manager.set_profile_sequence(uid.clone(), SequenceKind::Rules, Some("prepend: ['DOMAIN,sequence.test,DIRECT']\nappend: []\ndelete: []".into())).await?;
        manager.set_profile_merge(uid.clone(), Some("mode: rule".into())).await?;
        let profile_script = "function main(c,name) { if(c.pipeline !== 'global;' || c.rules[0] !== 'DOMAIN,sequence.test,DIRECT') throw 'order'; c.pipeline+='profile:'+c.mode; c.mode='rule'; return c; }";
        manager.set_profile_script(uid.clone(), Some(profile_script.into())).await?;
        manager.start().await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        assert_eq!(manager.runtime_config().await?["pipeline"].as_str(), Some("global;profile:rule"));
        assert!(manager.logs().iter().any(|log| log.stream == "script" && log.message.contains("global profile")));
        *fixture.state.content.lock().unwrap() = (YAML.replace("mode: rule", "mode: direct"), 10);
        manager.refresh_profile(uid.clone()).await?;
        assert_eq!(manager.runtime_config().await?["pipeline"].as_str(), Some("global;profile:rule"));
        assert_eq!(manager.client().get_proxies().await?.proxies["Main"].now.as_deref(), Some("REJECT"));
        manager.stop().await?;
        manager.set_profile_script(uid.clone(), None).await?;
        assert_eq!(manager.runtime_config().await?["pipeline"].as_str(), Some("global;rule;"));
        assert!(manager.status().pid.is_none());
        manager.set_profile_merge(uid.clone(), None).await?;
        assert_eq!(manager.runtime_config().await?["pipeline"].as_str(), Some("global;global;"));
        let catalog = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let raw_file = manager.profiles().items.unwrap().into_iter().find(|item| item.uid.as_deref() == Some(uid.as_str())).unwrap().file.unwrap().to_string();
        let raw = std::fs::read(directory.0.join("profiles").join(&raw_file))?;
        manager.shutdown().await?;
        // Offline corruption must not invalidate the already committed startup snapshot.
        std::fs::write(&script_file, "function main(c) { console.warn('global failure'); throw 'broken global'; }")?;
        let restored = CoreManager::spawn(options)?;
        let result = async {
            let requests = fixture.state.requests.lock().unwrap().len();
            restored.start().await?;
            assert_eq!(fixture.state.requests.lock().unwrap().len(), requests);
            assert_eq!(restored.status().config_revision, revision);
            assert_eq!(restored.runtime_config().await?["pipeline"].as_str(), Some("global;global;"));
            let pid = restored.status().pid;
            assert!(restored.select_profile(uid.clone()).await.is_err());
            assert!(restored.refresh_profile(uid).await.is_err());
            assert_eq!(restored.status().config_revision, revision);
            assert_eq!(restored.status().pid, pid);
            assert_eq!(serde_json::to_value(restored.profiles())?, catalog);
            assert_eq!(std::fs::read(directory.0.join("profiles").join(&raw_file))?, raw);
            assert!(restored.logs().iter().any(|log| log.stream == "script" && log.message.contains("global failure")));
            assert!(!directory.0.join("profile-refresh.yaml").exists());
            assert!(!directory.0.join("profile-merge.yaml").exists());
            Ok::<_, anyhow::Error>(())
        }.await;
        let cleanup = restored.shutdown().await;
        result.and(cleanup)
    }.await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires real Mihomo, global editing workers and local sockets"]
async fn global_edit_transactions_apply_refresh_rollback_reset_and_restore_nodes() -> Result<()> {
    use headless_core::config::{
        profile_store::{DEFAULT_GLOBAL_MERGE, DEFAULT_GLOBAL_SCRIPT},
        runtime,
    };
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let mut options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        // Global edits must not alter a running standalone configuration.
        manager.apply_config(runtime::parse(YAML)?).await?;
        manager.start().await?;
        let revision = manager.status().config_revision;
        let pid = manager.status().pid;
        manager.set_global_merge(Some("mode: global".into())).await?;
        let source = "function main(c,name) { console.info('global edit '+name); c.trace=(c.trace||'')+'G'; c.mode='direct'; return c; }";
        manager.set_global_script(Some(source.into())).await?;
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.status().pid, pid);
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("rule"));
        let base = manager.import_remote_profile(fixture.url("/refresh"), Some("edited globals".into()), RemoteOptions::default()).await?;
        let uid = base.uid.as_deref().unwrap().to_owned();
        manager.select_profile(uid.clone()).await?;
        manager.select_node("Main".into(), "REJECT".into()).await?;
        assert_eq!(manager.runtime_config().await?["trace"].as_str(), Some("GG"));
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("direct"));
        manager.set_profile_merge(uid.clone(), Some("mode: rule".into())).await?;
        manager.set_profile_script(uid.clone(), Some("function main(c) { c.trace+='P'; c.mode='rule'; return c; }".into())).await?;
        manager.set_global_script(Some(source.replace("+'G'", "+'N'"))).await?;
        assert_eq!(manager.runtime_config().await?["trace"].as_str(), Some("NP"));
        assert_eq!(manager.client().get_proxies().await?.proxies["Main"].now.as_deref(), Some("REJECT"));
        let current = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        let pid = manager.status().pid;
        let original = manager.global_script().await?.source;
        for invalid in ["function main( {", "function main(c) { console.warn('global failure'); throw 'bad'; }", "function main(c) { c.rules=['INVALID,DIRECT']; return c; }"] {
            assert!(manager.set_global_script(Some(invalid.into())).await.is_err());
            assert_eq!(serde_json::to_value(manager.profiles())?, current);
            assert_eq!(manager.status().config_revision, revision);
            assert_eq!(manager.status().pid, pid);
            assert_eq!(manager.global_script().await?.source, original);
        }
        assert!(manager.set_global_merge(Some("rules: ['INVALID,DIRECT']".into())).await.is_err());
        assert_eq!(serde_json::to_value(manager.profiles())?, current);
        // Publication failure after application must restore the old global pointer/runtime/core.
        let metadata = directory.0.join("profiles.yaml");
        let backup = directory.0.join("saved-catalog.yaml");
        std::fs::rename(&metadata, &backup)?; std::fs::create_dir(&metadata)?;
        let rejected = manager.set_global_script(Some(source.into())).await;
        std::fs::remove_dir(&metadata)?; std::fs::rename(&backup, &metadata)?;
        assert!(rejected.is_err());
        assert_eq!(serde_json::to_value(manager.profiles())?, current);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.runtime_config().await?["trace"].as_str(), Some("NP"));
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Running);
        manager.refresh_profile(uid.clone()).await?;
        assert_eq!(manager.runtime_config().await?["trace"].as_str(), Some("NP"));
        manager.stop().await?;
        manager.set_global_merge(Some("mode: direct".into())).await?;
        assert!(manager.status().pid.is_none());
        manager.set_global_script(None).await?;
        assert!(manager.status().pid.is_none());
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("rule"));
        manager.set_profile_script(uid.clone(), None).await?;
        manager.set_profile_merge(uid.clone(), None).await?;
        manager.set_global_merge(None).await?;
        assert_eq!(manager.global_merge().await?.yaml.as_deref(), Some(DEFAULT_GLOBAL_MERGE));
        assert_eq!(manager.global_script().await?.source.as_deref(), Some(DEFAULT_GLOBAL_SCRIPT));
        assert_eq!(manager.profiles().items.unwrap().len(), 3);
        assert_eq!(manager.status().active_profile.as_deref(), Some(uid.as_str()));
        let raw = std::fs::read(directory.0.join("profiles").join(manager.profiles().items.unwrap().into_iter().find(|row| row.uid.as_deref() == Some(uid.as_str())).unwrap().file.unwrap().to_string()))?;
        assert_eq!(raw, YAML.as_bytes());
        let revision = manager.status().config_revision;
        manager.shutdown().await?;
        let requests = fixture.state.requests.lock().unwrap().len();
        let restored = CoreManager::spawn(options)?;
        let result = async {
            restored.start().await?;
            assert_eq!(restored.status().config_revision, revision);
            assert_eq!(fixture.state.requests.lock().unwrap().len(), requests);
            assert_eq!(restored.client().get_proxies().await?.proxies["Main"].now.as_deref(), Some("REJECT"));
            assert_eq!(restored.global_merge().await?.yaml.as_deref(), Some(DEFAULT_GLOBAL_MERGE));
            assert_eq!(restored.global_script().await?.source.as_deref(), Some(DEFAULT_GLOBAL_SCRIPT));
            assert!(!directory.0.join("profile-merge.yaml").exists());
            Ok::<_, anyhow::Error>(())
        }.await;
        let cleanup = restored.shutdown().await;
        result.and(cleanup)
    }.await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires controlled core, global script workers and local sockets"]
async fn global_edit_restart_failure_and_shutdown_restore_source_and_runtime() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = Directory::new()?;
    let binary = directory.0.join("global-controlled-core.py");
    std::fs::write(&binary, include_str!("fixtures/mihomo.py"))?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
    let mut options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("missing.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options)?;
    let result = async {
        let base = manager.import_profile_yaml("mode: rule".into(), "base".into()).await?;
        manager.select_profile(base.uid.unwrap().to_string()).await?;
        manager.start().await?;
        let catalog = serde_json::to_value(manager.profiles())?;
        let revision = manager.status().config_revision;
        for script in [false, true] {
            let rejected = if script {
                manager
                    .set_global_script(Some(
                        "function main(c) { c['fixture-fail-start']=true; return c; }".into(),
                    ))
                    .await
            } else {
                manager.set_global_merge(Some("fixture-fail-start: true".into())).await
            };
            assert!(format!("{:#}", rejected.unwrap_err()).contains("exited before readiness"));
            assert_eq!(serde_json::to_value(manager.profiles())?, catalog);
            assert_eq!(manager.status().config_revision, revision);
            assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Running);
        }
        let running = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .set_global_script(Some("function main(c) { while(true) {} }".into()))
                    .await
            })
        };
        sleep(Duration::from_millis(100)).await;
        timeout(Duration::from_secs(2), manager.shutdown()).await??;
        assert!(running.await?.is_err());
        assert_eq!(serde_json::to_value(manager.profiles())?, catalog);
        assert!(!directory.0.join("profile-merge.yaml").exists());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
