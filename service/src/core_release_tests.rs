#![cfg(all(unix, target_arch = "x86_64"))]
use super::*;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Request, State},
    http::StatusCode,
    response::Response,
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::{PermissionsExt as _, symlink},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU16, Ordering},
    },
};
use tokio::{sync::Semaphore, task::JoinHandle};
// A deterministic gzip stream containing ordinary fixture text, never executable.
const PACKAGE: &[u8] = &[
    31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 75, 203, 172, 40, 41, 45, 74, 85, 72, 206, 47, 74, 5, 0, 199, 69, 132, 147, 12,
    0, 0, 0,
];
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).unwrap();
        let path = std::env::temp_dir().join(format!("ms-core-release-{}", hex(&bytes)));
        fs::create_dir(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        fs::write(path.join("verge-mihomo"), b"unchanged live core")?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Provider {
    metadata: Mutex<Value>,
    package: Mutex<Vec<u8>>,
    status: AtomicU16,
    hold_metadata: AtomicBool,
    hold_package: AtomicBool,
    stream: AtomicBool,
    redirect: Mutex<Option<String>>,
    requests: Mutex<Vec<String>>,
    release: Semaphore,
}
struct Fixture {
    repository: Repository,
    state: Arc<Provider>,
    task: JoinHandle<std::io::Result<()>>,
}
impl Fixture {
    async fn new() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}/", listener.local_addr()?);
        let repository = Repository {
            api: format!("{base}api/").parse()?,
            packages: format!("{base}download/").parse()?,
        };
        let metadata = json!({"tag_name":"v1.19.31","draft":false,"prerelease":false,"assets":[{"name":asset_name("v1.19.31")?,"state":"uploaded","size":PACKAGE.len(),"digest":format!("sha256:{}",hash(PACKAGE)),"browser_download_url":repository.package_url("v1.19.31")?}]});
        let state = Arc::new(Provider {
            metadata: Mutex::new(metadata),
            package: Mutex::new(PACKAGE.to_vec()),
            status: AtomicU16::new(200),
            hold_metadata: AtomicBool::new(false),
            hold_package: AtomicBool::new(false),
            stream: AtomicBool::new(false),
            redirect: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            release: Semaphore::new(0),
        });
        let task = tokio::spawn(
            axum::serve(listener, Router::new().fallback(serve).with_state(Arc::clone(&state))).into_future(),
        );
        Ok(Self {
            repository,
            state,
            task,
        })
    }
    fn downloads(&self, dir: &Directory) -> Result<CoreDownloads> {
        let mut downloads = CoreDownloads::new(&dir.0)?;
        downloads.repository = self.repository.clone();
        Ok(downloads)
    }
    async fn wait(&self, count: usize) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.state.requests.lock().unwrap().len() < count {
                tokio::time::sleep(Duration::from_millis(10)).await;
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
    let metadata = request.uri().path().starts_with("/api/");
    if (metadata && state.hold_metadata.load(Ordering::SeqCst))
        || (!metadata && state.hold_package.load(Ordering::SeqCst))
    {
        state.release.acquire().await.unwrap().forget();
    }
    if !metadata
        && request.uri().path().ends_with(".gz")
        && let Some(url) = state.redirect.lock().unwrap().clone()
    {
        return Response::builder()
            .status(StatusCode::FOUND)
            .header("Location", url)
            .body(Body::empty())
            .unwrap();
    }
    let bytes = if metadata {
        serde_json::to_vec(&*state.metadata.lock().unwrap()).unwrap()
    } else {
        state.package.lock().unwrap().clone()
    };
    let body = if state.stream.load(Ordering::SeqCst) {
        Body::from_stream(futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from(
            bytes,
        ))]))
    } else {
        Body::from(bytes)
    };
    Response::builder()
        .status(if metadata {
            200
        } else {
            state.status.load(Ordering::SeqCst)
        })
        .body(body)
        .unwrap()
}
fn hash(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&SHA256, bytes).as_ref())
}
#[tokio::test]
async fn latest_and_pinned_tags_resolve_the_unique_platform_asset_with_public_sha256() -> Result<()> {
    let fixture = Fixture::new().await?;
    let client = fixture.repository.client()?;
    let latest = fixture.repository.discover(&client, None).await?;
    assert_eq!(latest.version, "v1.19.31");
    assert_eq!(latest.sha256, hash(PACKAGE));
    assert_eq!(latest.target, TARGET);
    assert_eq!(fixture.repository.discover(&client, Some("v1.19.31")).await?, latest);
    assert_eq!(
        &*fixture.state.requests.lock().unwrap(),
        &["/api/latest", "/api/tags/v1.19.31"]
    );
    for version in [
        "../secret",
        "v1.2.3/secret",
        "alpha-123",
        "v1.2",
        "<html>private</html>",
    ] {
        assert!(fixture.repository.discover(&client, Some(version)).await.is_err());
    }
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
    Ok(())
}
#[tokio::test]
async fn metadata_requires_published_stable_uploaded_exact_asset_digest_size_and_url() -> Result<()> {
    let fixture = Fixture::new().await?;
    let client = fixture.repository.client()?;
    let original = fixture.state.metadata.lock().unwrap().clone();
    let mut cases = Vec::new();
    for (field, value) in [
        ("draft", json!(true)),
        ("prerelease", json!(true)),
        ("tag_name", json!("alpha-secret")),
    ] {
        let mut j = original.clone();
        j[field] = value;
        cases.push(j);
    }
    for (field, value) in [
        ("name", json!("wrong-platform")),
        ("state", json!("new")),
        ("size", json!(0)),
        ("size", json!(MAX_PACKAGE + 1)),
        ("digest", Value::Null),
        ("digest", json!("md5:private")),
        ("digest", json!("sha256:short")),
        ("browser_download_url", json!("https://evil.invalid/?token=private")),
    ] {
        let mut j = original.clone();
        j["assets"][0][field] = value;
        cases.push(j);
    }
    let mut duplicate = original.clone();
    duplicate["assets"]
        .as_array_mut()
        .unwrap()
        .push(original["assets"][0].clone());
    cases.push(duplicate);
    for value in cases {
        *fixture.state.metadata.lock().unwrap() = value;
        let error = fixture.repository.discover(&client, None).await.unwrap_err();
        assert!(!format!("{error:#}").contains("token=private"));
    }
    *fixture.state.metadata.lock().unwrap() = original;
    assert!(fixture.repository.discover(&client, Some("v1.2.3")).await.is_err());
    fixture.state.metadata.lock().unwrap()["padding"] = json!("x".repeat(MAX_METADATA));
    assert!(fixture.repository.discover(&client, None).await.is_err());
    Ok(())
}
#[tokio::test]
async fn resolved_preparation_pins_metadata_across_a_moving_latest_and_cancels_before_publication() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    let downloads = fixture.downloads(&dir)?;
    let client = fixture.repository.client()?;
    let release = fixture.repository.discover(&client, None).await?;
    fixture.state.metadata.lock().unwrap()["tag_name"] = json!("v9.9.9");
    let (closing, rx) = watch::channel(false);
    let prepared = downloads.prepare_resolved(release.clone(), &rx).await?;
    assert_eq!(prepared.release, release);
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 2);
    closing.send_replace(true);
    assert!(downloads.prepare_resolved(release, &rx).await.is_err());
    assert_eq!(fs::read_dir(&downloads.root)?.count(), 1);
    Ok(())
}
#[tokio::test]
async fn verified_preparation_is_private_atomic_reusable_and_rechecked_after_restart() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    let downloads = fixture.downloads(&dir)?;
    let (_closing, rx) = watch::channel(false);
    let prepared = downloads.prepare(None, &rx).await?;
    assert_eq!(fs::read(dir.0.join("verge-mihomo"))?, b"unchanged live core");
    let path = downloads.root.join(&prepared.id);
    assert_eq!(fs::read(path.join("package.gz"))?, PACKAGE);
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(path.join("package.gz"))?.permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read_dir(&downloads.root)?.count(), 1);
    assert_eq!(downloads.prepare(Some("v1.19.31"), &rx).await?.id, prepared.id);
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 3); // Cached package is not fetched again.
    let restored = fixture.downloads(&dir)?;
    assert_eq!(restored.inspect(&prepared.id)?.release, prepared.release);
    fs::write(path.join("package.gz"), b"changed")?;
    assert!(restored.inspect(&prepared.id).is_err());
    assert!(restored.prepare(None, &rx).await.is_err());
    Ok(())
}
#[tokio::test]
async fn partial_oversized_wrong_hash_wrong_magic_and_http_failure_leave_no_candidate() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    let downloads = fixture.downloads(&dir)?;
    let (_closing, rx) = watch::channel(false);
    fixture.state.stream.store(true, Ordering::SeqCst);
    for bytes in [
        PACKAGE[..4].to_vec(),
        [PACKAGE, &[1_u8]].concat(),
        vec![0; PACKAGE.len()],
    ] {
        *fixture.state.package.lock().unwrap() = bytes;
        assert!(downloads.prepare(None, &rx).await.is_err());
        assert_eq!(fs::read_dir(&downloads.root)?.count(), 0);
    }
    let invalid = vec![0; PACKAGE.len()];
    fixture.state.metadata.lock().unwrap()["assets"][0]["digest"] = json!(format!("sha256:{}", hash(&invalid)));
    assert!(downloads.prepare(None, &rx).await.is_err());
    assert_eq!(fs::read_dir(&downloads.root)?.count(), 0);
    fixture.state.status.store(503, Ordering::SeqCst);
    assert!(downloads.prepare(None, &rx).await.is_err());
    assert_eq!(fs::read_dir(&downloads.root)?.count(), 0);
    assert_eq!(fs::read(dir.0.join("verge-mihomo"))?, b"unchanged live core");
    Ok(())
}
#[tokio::test]
async fn redirects_keep_integrity_and_reject_other_origins_without_echoing_signed_urls() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    let downloads = fixture.downloads(&dir)?;
    let (_closing, rx) = watch::channel(false);
    *fixture.state.redirect.lock().unwrap() = Some("http://untrusted.invalid/?token=signed-secret".into());
    let error = downloads.prepare(None, &rx).await.unwrap_err();
    assert!(!format!("{error:#}").contains("signed-secret"));
    assert_eq!(fs::read_dir(&downloads.root)?.count(), 0);
    *fixture.state.redirect.lock().unwrap() = Some(fixture.repository.api.join("../binary")?.to_string());
    downloads.prepare(None, &rx).await?;
    Ok(())
}
#[tokio::test]
async fn cancellation_and_total_package_deadline_remove_partial_staging() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    fixture.state.hold_package.store(true, Ordering::SeqCst);
    let downloads = Arc::new(fixture.downloads(&dir)?);
    let (cancel, rx) = watch::channel(false);
    let own = Arc::clone(&downloads);
    let mut stop = rx.clone();
    let job = tokio::spawn(async move {
        tokio::select! {biased; _=stop.changed()=>anyhow::bail!("cancelled"),result=own.prepare(None,&rx)=>result,}
    });
    fixture.wait(2).await?;
    cancel.send_replace(true);
    assert!(job.await?.is_err());
    assert_eq!(fs::read_dir(&downloads.root)?.count(), 0);
    let (_closing, rx) = watch::channel(false);
    let own = Arc::clone(&downloads);
    let job = tokio::spawn(async move { own.prepare(None, &rx).await });
    fixture.wait(4).await?;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(301)).await;
    tokio::time::resume();
    let error = job.await?.unwrap_err();
    assert!(format!("{error:#}").contains("timed out"));
    assert_eq!(fs::read_dir(&downloads.root)?.count(), 0);
    Ok(())
}
#[tokio::test]
async fn cache_rejects_traversal_links_and_tampered_manifests_and_cleans_only_owned_pending() -> Result<()> {
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    let downloads = fixture.downloads(&dir)?;
    let (_closing, rx) = watch::channel(false);
    for id in ["../verge-mihomo", "v1.2.3-../../private", "v1.2.3-a"] {
        assert!(downloads.inspect(id).is_err());
    }
    let prepared = downloads.prepare(None, &rx).await?;
    let package = downloads.root.join(&prepared.id).join("package.gz");
    fs::remove_file(&package)?;
    symlink(dir.0.join("verge-mihomo"), &package)?;
    assert!(downloads.inspect(&prepared.id).is_err());
    fs::remove_file(&package)?;
    fs::write(&package, PACKAGE)?;
    fs::set_permissions(&package, fs::Permissions::from_mode(0o600))?;
    let manifest = downloads.root.join(&prepared.id).join("release.json");
    fs::write(manifest, b"{\"schema_version\":999}")?;
    assert!(downloads.inspect(&prepared.id).is_err());
    let pending = downloads.root.join(format!(".pending-{}", "a".repeat(32)));
    fs::create_dir(&pending)?;
    fs::write(pending.join("partial"), "bytes")?;
    let link = downloads.root.join(format!(".pending-{}", "b".repeat(32)));
    symlink(&dir.0, &link)?;
    let unknown = downloads.root.join(".pending-user");
    fs::create_dir(&unknown)?;
    let _ = CoreDownloads::new(&dir.0)?;
    assert!(!pending.exists());
    assert!(fs::symlink_metadata(link)?.file_type().is_symlink());
    assert!(unknown.exists());
    assert!(downloads.root.join(&prepared.id).exists());
    Ok(())
}
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY and gzip; no external downloads"]
async fn real_mihomo_package_is_staged_and_read_back_without_changing_the_existing_core() -> Result<()> {
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let dir = Directory::new()?;
    let fixture = Fixture::new().await?;
    let output = std::process::Command::new(&binary).arg("-v").output()?;
    ensure!(output.status.success(), "real core version probe failed");
    let version = String::from_utf8(output.stdout)?
        .split_whitespace()
        .nth(2)
        .context("core version missing")?
        .to_owned();
    ensure!(stable_version(&version), "real stable version required");
    let compressed = std::process::Command::new("gzip")
        .args(["-n", "-c"])
        .arg(&binary)
        .output()?;
    ensure!(compressed.status.success(), "gzip failed");
    *fixture.state.package.lock().unwrap() = compressed.stdout.clone();
    *fixture.state.metadata.lock().unwrap() = json!({"tag_name":version,"draft":false,"prerelease":false,"assets":[{"name":asset_name(&version)?,"state":"uploaded","size":compressed.stdout.len(),"digest":format!("sha256:{}",hash(&compressed.stdout)),"browser_download_url":fixture.repository.package_url(&version)?}]});
    fs::copy(&binary, dir.0.join("verge-mihomo"))?;
    let before = hash(&fs::read(dir.0.join("verge-mihomo"))?);
    let downloads = fixture.downloads(&dir)?;
    let (_closing, rx) = watch::channel(false);
    let prepared = downloads.prepare(Some(&version), &rx).await?;
    assert_eq!(downloads.inspect(&prepared.id)?.release.version, version);
    let decoded = std::process::Command::new("gzip")
        .arg("-dc")
        .arg(downloads.root.join(prepared.id).join("package.gz"))
        .output()?;
    assert!(decoded.status.success());
    assert_eq!(hash(&decoded.stdout), before);
    assert_eq!(hash(&fs::read(dir.0.join("verge-mihomo"))?), before);
    Ok(())
}
