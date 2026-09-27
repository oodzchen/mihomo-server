//! Isolated service processes exercise real environment discovery without mutating
//! the Rust test process environment (which would race concurrent tests).
#![cfg(unix)]
use anyhow::{Context as _, Result, ensure};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};
const YAML: &str = "proxies: []\nmode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']";
const VARIABLES: &[&str] = &[
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
    "REQUEST_METHOD",
];
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-system-proxy-{}-{stamp:x}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn validator(&self) -> Result<PathBuf> {
        use std::os::unix::fs::PermissionsExt as _;
        let path = self.0.join("validator.py");
        fs::write(
            &path,
            "#!/usr/bin/python3\nimport sys\nsys.exit(0 if '-t' in sys.argv else 1)\n",
        )?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        Ok(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Provider {
    requests: Mutex<Vec<(String, HeaderMap)>>,
    hold: AtomicBool,
    release: Semaphore,
    redirect: Mutex<Option<String>>,
    authentication: Mutex<Option<String>>,
}
impl Default for Provider {
    fn default() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            hold: AtomicBool::new(false),
            release: Semaphore::new(0),
            redirect: Mutex::new(None),
            authentication: Mutex::new(None),
        }
    }
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
            release: Semaphore::new(0),
            ..Default::default()
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
    state
        .requests
        .lock()
        .unwrap()
        .push((request.uri().to_string(), request.headers().clone()));
    let authentication = state.authentication.lock().unwrap().clone();
    if authentication.as_ref().is_some_and(|auth| {
        request
            .headers()
            .get(header::PROXY_AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some(auth)
    }) {
        return StatusCode::PROXY_AUTHENTICATION_REQUIRED.into_response();
    }
    if state.hold.load(Ordering::SeqCst) {
        if let Ok(permit) = state.release.acquire().await {
            permit.forget();
        }
    }
    if request.method() == axum::http::Method::CONNECT {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    match request.uri().path() {
        "/redirect" => (
            StatusCode::FOUND,
            [(header::LOCATION, state.redirect.lock().unwrap().clone().unwrap())],
        )
            .into_response(),
        "/error" => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        "/large" => Response::builder()
            .header(header::CONTENT_LENGTH, runtime_limit() + 1)
            .body(Body::from(vec![b' '; runtime_limit() + 1]))
            .unwrap(),
        _ => ([("subscription-userinfo", "upload=1; download=2; total=10")], YAML).into_response(),
    }
}
fn runtime_limit() -> usize {
    headless_core::config::runtime::MAX_CONFIG_BYTES
}
struct Service {
    child: Child,
    base: String,
    token: String,
    client: reqwest::Client,
}
impl Service {
    async fn start(dir: &Directory, values: &BTreeMap<&str, String>, real: bool) -> Result<Self> {
        let address = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?;
        let binary = if real {
            PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?)
        } else {
            dir.validator()?
        };
        let mut command = Command::new(env!("CARGO_BIN_EXE_mihomo-server"));
        command
            .args(["--no-start", "--listen", &address.to_string(), "--data-dir"])
            .arg(&dir.0)
            .arg("--mihomo")
            .arg(binary)
            .arg("--config")
            .arg(dir.0.join("missing.yaml"));
        for name in VARIABLES {
            command.env_remove(name);
        }
        command.envs(values);
        let mut service = Self {
            child: command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()?,
            base: format!("http://{address}"),
            token: String::new(),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(10))
                .build()?,
        };
        timeout(Duration::from_secs(5), async {
            loop {
                ensure!(
                    service.child.try_wait()?.is_none(),
                    "fixture service exited before readiness"
                );
                if let Ok(token) = fs::read_to_string(dir.0.join("management-token")) {
                    service.token = token.trim().to_owned();
                    if service
                        .client
                        .get(format!("{}/api/status", service.base))
                        .bearer_auth(&service.token)
                        .send()
                        .await
                        .is_ok_and(|response| response.status().is_success())
                    {
                        break;
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(service)
    }
    async fn request(&self, body: Value) -> Result<(StatusCode, Value)> {
        send(&self.client, &self.base, &self.token, body).await
    }
    async fn api(&self, body: Value) -> Result<Value> {
        let (status, value) = self.request(body).await?;
        ensure!(status.is_success(), "fixture command failed with {status}");
        Ok(value)
    }
    async fn shutdown(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            let pid = self.child.id().context("fixture service PID missing")?;
            // Signal only the child spawned and owned by this fixture.
            ensure!(
                unsafe { libc::kill(pid as i32, libc::SIGTERM) } == 0,
                "failed to terminate fixture service"
            );
            ensure!(
                timeout(Duration::from_secs(5), self.child.wait()).await??.success(),
                "fixture service shutdown failed"
            );
        }
        Ok(())
    }
}
async fn send(client: &reqwest::Client, base: &str, token: &str, body: Value) -> Result<(StatusCode, Value)> {
    let response = client
        .post(format!("{base}/api/commands"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    Ok((status, response.json().await?))
}
fn import(url: &str, options: Value) -> Value {
    json!({"command":"import_remote_profile","url":url,"options":options})
}
fn environment(values: &[(&'static str, String)]) -> BTreeMap<&'static str, String> {
    values.iter().cloned().collect()
}

#[tokio::test]
async fn service_system_proxy_auth_redirect_refresh_metadata_guard_and_restart() -> Result<()> {
    let dir = Directory::new()?;
    let origin = Fixture::new().await?;
    let proxy = Fixture::new().await?;
    *proxy.state.authentication.lock().unwrap() = Some("Basic dXNlcjpwYXNzOnNlY3JldA==".into());
    *proxy.state.redirect.lock().unwrap() = Some(format!("{}/ok", origin.base));
    let values = environment(&[
        (
            "HTTP_PROXY",
            proxy.base.replacen("http://", "http://user:pass%3Asecret@", 1),
        ),
        ("NO_PROXY", "127.0.0.1,localhost".into()),
    ]);
    let mut service = Service::start(&dir, &values, false).await?;
    let result = async {
        let unauth = service
            .client
            .post(format!("{}/api/commands", service.base))
            .json(&import("http://subscription.invalid/ok", json!({"with_proxy":true})))
            .send()
            .await?;
        assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(proxy.count(), 0);
        assert!(
            !service
                .request(import("http://subscription.invalid/ok", json!({"with_proxy":"true"})))
                .await?
                .0
                .is_success()
        );
        assert_eq!(proxy.count(), 0);
        let item = service
            .api(import(
                "http://subscription.invalid/ok?token=provider-secret",
                json!({"with_proxy":true,"user_agent":"system-fixture","timeout_seconds":3,"allow_auto_update":false}),
            ))
            .await?;
        assert_eq!(item["option"]["with_proxy"], true);
        assert_eq!(item["extra"]["download"], 2);
        let uid = item["uid"].as_str().unwrap();
        service
            .api(import(
                "http://subscription.invalid/redirect",
                json!({"with_proxy":true}),
            ))
            .await?;
        service
            .api(import(&format!("{}/direct", origin.base), json!({})))
            .await?;
        assert_eq!(origin.count(), 2);
        for (_, headers) in origin.state.requests.lock().unwrap().iter() {
            assert!(!headers.contains_key(header::AUTHORIZATION));
            assert!(!headers.contains_key(header::PROXY_AUTHORIZATION));
        }
        let count = proxy.count() + 1;
        proxy.state.hold.store(true, Ordering::SeqCst);
        let mut refresh = Box::pin(service.request(json!({"command":"refresh_profile","uid":uid})));
        tokio::select! {
            result=&mut refresh=>{result?;anyhow::bail!("unexpected early refresh")},
            result=proxy.wait(count)=>result?,
        }
        service
            .api(json!({"command":"edit_profile","uid":uid,"patch":{"options":{"with_proxy":false}}}))
            .await?;
        proxy.state.release.add_permits(1);
        assert!(!refresh.await?.0.is_success());
        let saved = service.api(json!({"command":"profiles"})).await?;
        let saved = saved["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["uid"] == item["uid"])
            .unwrap();
        assert_eq!(saved["file"], item["file"]);
        assert_eq!(saved["option"]["with_proxy"], false);
        proxy.state.hold.store(false, Ordering::SeqCst);
        service
            .api(json!({"command":"edit_profile","uid":uid,"patch":{"options":{"with_proxy":true}}}))
            .await?;
        service.api(json!({"command":"refresh_profile","uid":uid})).await?;
        for (_, headers) in proxy.state.requests.lock().unwrap().iter() {
            assert!(!headers.contains_key(header::AUTHORIZATION));
            assert_eq!(headers[header::PROXY_AUTHORIZATION], "Basic dXNlcjpwYXNzOnNlY3JldA==");
        }
        Ok::<_, anyhow::Error>(item)
    }
    .await;
    let cleanup = service.shutdown().await;
    let item = result?;
    cleanup?;
    let count = proxy.count();
    let mut restored = Service::start(&dir, &values, false).await?;
    let result = async {
        let profiles = restored.api(json!({"command":"profiles"})).await?;
        let saved = profiles["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["uid"] == item["uid"])
            .unwrap();
        assert_eq!(saved["option"], item["option"]);
        assert_eq!(proxy.count(), count);
        restored
            .api(json!({"command":"refresh_profile","uid":item["uid"]}))
            .await?;
        assert_eq!(proxy.count(), count + 1);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn service_environment_precedence_bypass_defaults_and_https_connect() -> Result<()> {
    let origin = Fixture::new().await?;
    let proxy = Fixture::new().await?;
    for values in [
        environment(&[
            ("HTTP_PROXY", proxy.base.clone()),
            ("http_proxy", "http://127.0.0.1:1".into()),
        ]),
        environment(&[("http_proxy", proxy.base.clone())]),
        environment(&[("ALL_PROXY", proxy.base.clone())]),
    ] {
        let dir = Directory::new()?;
        let mut service = Service::start(&dir, &values, false).await?;
        let result = service
            .api(import("http://subscription.invalid/ok", json!({"with_proxy":true})))
            .await;
        let cleanup = service.shutdown().await;
        result?;
        cleanup?;
    }
    assert_eq!(proxy.count(), 3);
    for values in [
        environment(&[]),
        environment(&[("HTTP_PROXY", "".into()), ("http_proxy", "invalid secret".into())]),
        environment(&[("HTTP_PROXY", proxy.base.clone()), ("NO_PROXY", "127.0.0.0/8".into())]),
        environment(&[("HTTP_PROXY", proxy.base.clone()), ("no_proxy", "127.0.0.1".into())]),
        environment(&[
            ("HTTP_PROXY", "invalid secret".into()),
            ("REQUEST_METHOD", "GET".into()),
        ]),
    ] {
        let dir = Directory::new()?;
        let mut service = Service::start(&dir, &values, false).await?;
        let result = service
            .api(import(&format!("{}/ok", origin.base), json!({"with_proxy":true})))
            .await;
        let cleanup = service.shutdown().await;
        assert_eq!(result?["option"]["with_proxy"], true);
        cleanup?;
    }
    assert_eq!(origin.count(), 5);
    assert_eq!(proxy.count(), 3);
    let dir = Directory::new()?;
    let mut service = Service::start(
        &dir,
        &environment(&[
            ("HTTPS_PROXY", proxy.base.clone()),
            ("HTTP_PROXY", "http://127.0.0.1:1".into()),
        ]),
        false,
    )
    .await?;
    let result = service
        .request(import(
            "https://subscription.invalid/ok?token=private-query",
            json!({"with_proxy":true,"timeout_seconds":1}),
        ))
        .await;
    let cleanup = service.shutdown().await;
    let (status, error) = result?;
    cleanup?;
    assert!(!status.is_success());
    assert!(!error.to_string().contains("private-query"));
    assert_eq!(proxy.count(), 4);
    assert_eq!(
        proxy.state.requests.lock().unwrap().last().unwrap().0,
        "subscription.invalid:443"
    );
    Ok(())
}

#[tokio::test]
async fn malformed_and_failed_system_proxy_never_fall_back_and_keep_catalog_unchanged() -> Result<()> {
    let origin = Fixture::new().await?;
    for endpoint in [
        "http://private-user:private-password@:7890",
        "socks5://private-user:private-password@proxy:1080",
        "http://127.0.0.1:1",
    ] {
        let dir = Directory::new()?;
        let mut service = Service::start(&dir, &environment(&[("HTTP_PROXY", endpoint.into())]), false).await?;
        let result = async {
            let before = service.api(json!({"command":"profiles"})).await?;
            let (status, error) = service
                .request(import(
                    &format!("{}/ok?token=subscription-secret", origin.base),
                    json!({"with_proxy":true,"timeout_seconds":1}),
                ))
                .await?;
            assert!(!status.is_success());
            for secret in ["private-user", "private-password", "subscription-secret"] {
                assert!(!error.to_string().contains(secret));
            }
            assert_eq!(service.api(json!({"command":"profiles"})).await?, before);
            assert!(
                !service
                    .request(import(
                        &format!("{}/ok", origin.base),
                        json!({"with_proxy":true,"self_proxy":true})
                    ))
                    .await?
                    .0
                    .is_success()
            );
            let count = origin.count();
            service
                .api(import(&format!("{}/ok", origin.base), json!({"with_proxy":false})))
                .await?;
            assert_eq!(origin.count(), count + 1);
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = service.shutdown().await;
        result?;
        cleanup?;
    }
    assert_eq!(origin.count(), 3);
    Ok(())
}

#[tokio::test]
async fn system_download_bounds_admission_lifecycle_and_shutdown_are_preserved() -> Result<()> {
    let dir = Directory::new()?;
    let proxy = Fixture::new().await?;
    let mut service = Service::start(&dir, &environment(&[("HTTP_PROXY", proxy.base.clone())]), false).await?;
    let result = async {
        let before = service.api(json!({"command":"profiles"})).await?;
        for path in ["large", "error"] {
            assert!(
                !service
                    .request(import(
                        &format!("http://subscription.invalid/{path}"),
                        json!({"with_proxy":true})
                    ))
                    .await?
                    .0
                    .is_success()
            );
            assert_eq!(service.api(json!({"command":"profiles"})).await?, before);
        }
        proxy.state.hold.store(true, Ordering::SeqCst);
        let (status, error) = service
            .request(import(
                "http://subscription.invalid/timeout?token=private-query",
                json!({"with_proxy":true,"timeout_seconds":1}),
            ))
            .await?;
        assert!(!status.is_success());
        assert!(!error.to_string().contains("private-query"));
        let count = proxy.count();
        let mut requests = JoinSet::new();
        for _ in 0..6 {
            let (client, base, token) = (service.client.clone(), service.base.clone(), service.token.clone());
            requests.spawn(async move {
                send(
                    &client,
                    &base,
                    &token,
                    import("http://subscription.invalid/held", json!({"with_proxy":true})),
                )
                .await
            });
        }
        proxy.wait(count + 4).await?;
        sleep(Duration::from_millis(50)).await;
        assert_eq!(proxy.count(), count + 4);
        service.api(json!({"command":"stop"})).await?;
        assert_eq!(proxy.count(), count + 4);
        Ok::<_, anyhow::Error>(requests)
    }
    .await;
    let cleanup = service.shutdown().await;
    let mut requests = result?;
    cleanup?;
    while let Some(result) = requests.join_next().await {
        if let Ok((status, _)) = result? {
            assert!(!status.is_success());
        }
    }
    assert_eq!(
        headless_core::config::profile_store::ProfileStore::open(&dir.0)?
            .snapshot()
            .items
            .unwrap()
            .len(),
        2
    );
    Ok(())
}
