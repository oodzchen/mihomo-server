#![cfg(target_os = "linux")]
use anyhow::{Context as _, Result, ensure};
use headless_core::config::{
    profile_store::{ProfilePatch, RemoteOptionsPatch},
    runtime,
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
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};
const YAML: &str = "proxies: []\nmode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']";
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-self-proxy-{}-{stamp:x}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn options(&self) -> Result<CoreOptions> {
        let mut options = CoreOptions::new(
            std::env::var_os("MIHOMO_TEST_BINARY")
                .context("set MIHOMO_TEST_BINARY")?
                .into(),
            self.0.clone(),
            self.0.join("missing.yaml"),
        );
        options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
        Ok(options)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Provider {
    requests: Mutex<Vec<(String, String)>>,
    hold: AtomicBool,
    release: Notify,
}
struct Tunnel {
    port: u16,
    state: Arc<Provider>,
    task: JoinHandle<()>,
}
impl Tunnel {
    async fn new() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let state = Arc::new(Provider::default());
        let worker = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let mut children = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break };
                        let state = Arc::clone(&worker);
                        children.spawn(async move { let _ = serve(socket, state).await; });
                    }
                    _ = children.join_next(), if !children.is_empty() => {}
                }
            }
        });
        Ok(Self { port, state, task })
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
impl Drop for Tunnel {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn headers(socket: &mut TcpStream) -> Result<String> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        ensure!(bytes.len() < 16384, "oversize fixture request");
        bytes.push(socket.read_u8().await?);
    }
    Ok(String::from_utf8(bytes)?)
}
async fn serve(mut socket: TcpStream, state: Arc<Provider>) -> Result<()> {
    let first = headers(&mut socket).await?;
    ensure!(
        first.starts_with("CONNECT subscription.invalid:80 "),
        "expected upstream tunnel"
    );
    socket.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
    let request = headers(&mut socket).await?;
    let large = request.contains("/large");
    let error = request.contains("/error");
    state.requests.lock().unwrap().push((first, request));
    if state.hold.load(Ordering::SeqCst) {
        state.release.notified().await;
    }
    let length = if large {
        runtime::MAX_CONFIG_BYTES + 1
    } else {
        YAML.len()
    };
    let status = if error { "503 Unavailable" } else { "200 OK" };
    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {length}\r\nSubscription-Userinfo: upload=1; download=2; total=10\r\nConnection: close\r\n\r\n{YAML}").as_bytes()).await?;
    Ok(())
}
fn proxy_options() -> RemoteOptions {
    RemoteOptions {
        self_proxy: Some(true),
        timeout_seconds: Some(5),
        ..Default::default()
    }
}
fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}
fn config(tunnel: u16, port: u16, mixed: u16) -> Result<serde_yaml_ng::Mapping> {
    runtime::parse(&format!(
        "mode: rule\nmixed-port: {mixed}\nport: {port}\nauthentication: ['fixture:private:password']\ndns: {{enable: false}}\nproxies: [{{name: Upstream, type: http, server: 127.0.0.1, port: {tunnel}}}]\nproxy-groups: [{{name: Hop, type: select, proxies: [Upstream]}}]\nrules: ['MATCH,Hop']"
    ))
}
#[tokio::test]
#[ignore = "requires real Mihomo HTTP proxy and local sockets"]
async fn authenticated_self_proxy_uses_upstream_tunnel_live_ports_bounds_and_persisted_refresh() -> Result<()> {
    let dir = Directory::new()?;
    let tunnel = Tunnel::new().await?;
    let options = dir.options()?;
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        manager.apply_config(config(tunnel.port, 0, free_port()?)?).await?;
        manager.start().await?;
        let before = manager.status();
        // This hostname has no DNS entry. Only the controlled upstream tunnel can serve it.
        let item = manager
            .import_remote_profile(
                "http://subscription.invalid/ok?token=provider-secret".into(),
                Some("proxied".into()),
                proxy_options(),
            )
            .await?;
        let uid = item.uid.as_deref().unwrap().to_owned();
        assert_eq!(item.option.as_ref().unwrap().self_proxy, Some(true));
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(item.extra.as_ref().unwrap().download, 2);
        assert!(
            manager
                .import_remote_profile(
                    "http://subscription.invalid/ok".into(),
                    None,
                    RemoteOptions {
                        timeout_seconds: Some(1),
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(tunnel.state.requests.lock().unwrap().len(), 1);
        // HTTP listener fallback must resolve the new port, without cached Mixed routing.
        manager.apply_config(config(tunnel.port, free_port()?, 0)?).await?;
        manager.refresh_profile(uid.clone()).await?;
        for path in ["large", "error"] {
            let before = serde_json::to_value(manager.profiles())?;
            let error = manager
                .import_remote_profile(
                    format!("http://subscription.invalid/{path}?token=secret-query"),
                    None,
                    proxy_options(),
                )
                .await
                .unwrap_err();
            assert!(!format!("{error:#}").contains("secret-query"));
            assert_eq!(serde_json::to_value(manager.profiles())?, before);
        }
        tunnel.state.hold.store(true, Ordering::SeqCst);
        let before = serde_json::to_value(manager.profiles())?;
        let error = manager
            .import_remote_profile(
                "http://subscription.invalid/timeout?token=timeout-secret".into(),
                None,
                RemoteOptions {
                    timeout_seconds: Some(1),
                    ..proxy_options()
                },
            )
            .await
            .unwrap_err();
        assert!(!format!("{error:#}").contains("timeout-secret"));
        assert_eq!(serde_json::to_value(manager.profiles())?, before);
        tunnel.state.hold.store(false, Ordering::SeqCst);
        tunnel.state.release.notify_waiters();
        let requests = tunnel.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        for (first, inner) in requests.iter() {
            for header in ["authorization:", "proxy-authorization:", "private:password"] {
                assert!(!first.to_lowercase().contains(header));
                assert!(!inner.to_lowercase().contains(header));
            }
        }
        Ok::<_, anyhow::Error>(uid)
    }
    .await;
    let cleanup = manager.shutdown().await;
    let uid = result?;
    cleanup?;
    let restored = CoreManager::spawn(options)?;
    let result = async {
        restored.start().await?;
        assert_eq!(
            restored
                .profiles()
                .items
                .unwrap()
                .iter()
                .find(|p| p.uid.as_deref() == Some(&uid))
                .unwrap()
                .option
                .as_ref()
                .unwrap()
                .self_proxy,
            Some(true)
        );
        restored.refresh_profile(uid).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result.and(cleanup)
}
#[tokio::test]
#[ignore = "requires real Mihomo HTTP proxy and local sockets"]
async fn self_proxy_cancels_on_core_changes_and_rejects_stale_metadata() -> Result<()> {
    let dir = Directory::new()?;
    let tunnel = Tunnel::new().await?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        manager.apply_config(config(tunnel.port, 0, free_port()?)?).await?;
        manager.start().await?;
        let item = manager
            .import_remote_profile("http://subscription.invalid/ok".into(), None, proxy_options())
            .await?;
        let uid = item.uid.as_deref().unwrap().to_owned();
        tunnel.state.hold.store(true, Ordering::SeqCst);
        let mut refresh = Box::pin(manager.refresh_profile(uid.clone()));
        tokio::select! {
            result = &mut refresh => { result?; anyhow::bail!("unexpected early completion") },
            result = tunnel.wait_requests(2) => result?,
        }
        manager
            .edit_profile(
                uid.clone(),
                ProfilePatch {
                    options: Some(RemoteOptionsPatch {
                        self_proxy: Some(false),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        tunnel.state.release.notify_waiters();
        assert!(refresh.await.is_err());
        let saved = manager
            .profiles()
            .items
            .unwrap()
            .into_iter()
            .find(|p| p.uid == item.uid)
            .unwrap();
        assert_eq!(saved.file, item.file);
        assert_eq!(saved.option.unwrap().self_proxy, Some(false));
        for reload in [false, true] {
            let before = serde_json::to_value(manager.profiles())?;
            let mut download = Box::pin(manager.import_remote_profile(
                "http://subscription.invalid/held".into(),
                None,
                proxy_options(),
            ));
            let count = tunnel.state.requests.lock().unwrap().len() + 1;
            tokio::select! {
                result = &mut download => { result?; anyhow::bail!("unexpected early completion") },
                result = tunnel.wait_requests(count) => result?,
            }
            if reload {
                manager.apply_config(config(tunnel.port, free_port()?, 0)?).await?;
            } else {
                manager.stop().await?;
            }
            let error = timeout(Duration::from_secs(1), download).await?.unwrap_err();
            assert!(error.to_string().contains("managed proxy changed"));
            assert_eq!(serde_json::to_value(manager.profiles())?, before);
            if !reload {
                assert!(
                    manager
                        .import_remote_profile("http://subscription.invalid/held".into(), None, proxy_options())
                        .await
                        .is_err()
                );
                manager.start().await?;
            }
        }
        // No HTTP-compatible ingress: fail rather than silently download directly.
        manager.apply_config(runtime::parse(YAML)?).await?;
        assert!(
            manager
                .import_remote_profile("http://subscription.invalid/held".into(), None, proxy_options())
                .await
                .is_err()
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
