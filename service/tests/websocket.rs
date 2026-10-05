#![cfg(unix)]

use anyhow::{Context as _, Result, ensure};
use futures_util::{SinkExt as _, StreamExt as _};
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions},
    management::{
        Management,
        auth::Authentication,
        http::{HttpState, MAX_WEBSOCKETS, router},
    },
};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Error, Message, client::IntoClientRequest as _},
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Server {
    directory: Directory,
    manager: CoreManager,
    state: HttpState,
    stop: watch::Sender<bool>,
    task: JoinHandle<std::io::Result<()>>,
    address: std::net::SocketAddr,
    token: String,
}
impl Server {
    async fn new(real: bool) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt as _;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = Directory(std::env::temp_dir().join(format!("ms-ws-{}-{stamp:x}", std::process::id())));
        std::fs::create_dir(&directory.0)?;
        let binary = if real {
            std::env::var_os("MIHOMO_TEST_BINARY")
                .context("set MIHOMO_TEST_BINARY")?
                .into()
        } else {
            let path = directory.0.join("validator.py");
            std::fs::write(
                &path,
                "#!/usr/bin/python3\nimport sys\nsys.exit(0 if '-t' in sys.argv else 1)\n",
            )?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            path
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let manager = CoreManager::spawn(CoreOptions::new(
            binary,
            directory.0.clone(),
            directory.0.join("missing.yaml"),
        ))?;
        let auth = Authentication::load_or_create(&directory.0.join("management-token"), address, None)?;
        let token = std::fs::read_to_string(directory.0.join("management-token"))?
            .trim()
            .to_owned();
        let state = HttpState::new(Management::new(manager.clone(), auth));
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(
            axum::serve(listener, router(state.clone()))
                .with_graceful_shutdown(async move {
                    if !*stopped.borrow_and_update() {
                        let _ = stopped.changed().await;
                    }
                })
                .into_future(),
        );
        Ok(Self {
            directory,
            manager,
            state,
            stop,
            task,
            address,
            token,
        })
    }
    async fn socket(&self, path: &str) -> Result<Socket> {
        let mut request = format!("ws://{}{path}", self.address).into_client_request()?;
        request
            .headers_mut()
            .insert("Origin", format!("http://{}", self.address).parse()?);
        Ok(connect_async(request).await?.0)
    }
    async fn authenticated(&self, path: &str) -> Result<Socket> {
        let mut socket = self.socket(path).await?;
        socket
            .send(Message::Text(
                json!({"type":"authenticate","token":self.token}).to_string().into(),
            ))
            .await?;
        assert_eq!(receive(&mut socket).await?["type"], "ready");
        Ok(socket)
    }
    async fn shutdown(self) -> Result<()> {
        self.state.close();
        self.stop.send_replace(true);
        let core = self.manager.shutdown().await;
        timeout(Duration::from_secs(5), self.state.drain_websockets()).await?;
        timeout(Duration::from_secs(5), self.task).await???;
        core
    }
}

async fn receive(socket: &mut Socket) -> Result<Value> {
    timeout(Duration::from_secs(6), async {
        loop {
            match socket.next().await.context("WebSocket ended")?? {
                Message::Text(text) => return Ok::<_, anyhow::Error>(serde_json::from_str(&text)?),
                Message::Ping(_) => socket.flush().await?,
                Message::Close(_) => anyhow::bail!("WebSocket closed before expected data"),
                _ => {}
            }
        }
    })
    .await?
}
async fn until(socket: &mut Socket, predicate: impl Fn(&Value) -> bool) -> Result<Value> {
    timeout(Duration::from_secs(8), async {
        loop {
            let value = receive(socket).await?;
            if predicate(&value) {
                break Ok::<_, anyhow::Error>(value);
            }
        }
    })
    .await?
}
async fn closed(socket: &mut Socket) -> Result<()> {
    timeout(Duration::from_secs(4), async {
        loop {
            match socket.next().await {
                None | Some(Ok(Message::Close(_))) | Some(Err(_)) => break Ok::<_, anyhow::Error>(()),
                Some(Ok(Message::Ping(_))) => {
                    let _ = socket.flush().await;
                }
                Some(Ok(Message::Text(text))) => {
                    let value: Value = serde_json::from_str(&text)?;
                    ensure!(value["type"] == "error", "unauthenticated session leaked data");
                }
                _ => {}
            }
        }
    })
    .await?
}
async fn connection_count(manager: &CoreManager, count: usize) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        loop {
            if manager.client().connection_manager.0.len() == count {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires local TCP socket binding permissions"]
async fn socket_authentication_limits_and_shutdown_do_not_leak_state() -> Result<()> {
    let server = Server::new(false).await?;
    let result = async {
        for (header, value) in [
            ("Origin", "http://evil.test"),
            ("Host", "evil.test"),
            ("Authorization", "Bearer invalid"),
        ] {
            let mut request = format!("ws://{}/api/events", server.address).into_client_request()?;
            request.headers_mut().insert(header, value.parse()?);
            assert!(
                matches!(connect_async(request).await, Err(Error::Http(response)) if response.status().as_u16() == 401)
            );
        }
        assert!(
            matches!(connect_async(format!("ws://{}/api/events?token=bad", server.address)).await,
            Err(Error::Http(response)) if response.status().as_u16() == 400)
        );
        assert!(
            matches!(connect_async(format!("ws://{}/api/streams/unknown", server.address)).await,
            Err(Error::Http(response)) if response.status().as_u16() == 404)
        );
        for frame in [
            json!({"type":"authenticate","token":"invalid"}),
            json!({"type":"authenticate","token":server.token,"extra":true}),
            json!({"type":"status"}),
        ] {
            let mut socket = server.socket("/api/events").await?;
            socket.send(Message::Text(frame.to_string().into())).await?;
            closed(&mut socket).await?;
        }
        let mut large = server.socket("/api/events").await?;
        large.send(Message::Text("x".repeat(5000).into())).await?;
        closed(&mut large).await?;
        let mut idle = server.socket("/api/streams/traffic").await?;
        assert!(timeout(Duration::from_millis(150), idle.next()).await.is_err());
        assert!(server.manager.client().connection_manager.0.is_empty());
        let timeout_error = receive(&mut idle).await?;
        assert_eq!(timeout_error["code"], "unauthorized");
        closed(&mut idle).await?;
        let mut sockets = Vec::new();
        for _ in 0..MAX_WEBSOCKETS {
            sockets.push(server.socket("/api/events").await?);
        }
        assert!(
            matches!(connect_async(format!("ws://{}/api/events", server.address)).await,
            Err(Error::Http(response)) if response.status().as_u16() == 503)
        );
        server.state.close();
        timeout(Duration::from_secs(3), server.state.drain_websockets()).await?;
        for socket in &mut sockets {
            closed(socket).await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = server.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires local TCP socket binding permissions"]
async fn events_reconnect_with_current_failed_core_and_profile_snapshots() -> Result<()> {
    let server = Server::new(false).await?;
    let result = async {
        let mut events = server.authenticated("/api/events").await?;
        let snapshot = receive(&mut events).await?;
        assert_eq!(snapshot["type"], "snapshot");
        assert_eq!(snapshot["status"]["phase"], "stopped");
        assert!(server.manager.start().await.is_err());
        let failed = until(&mut events, |value| {
            value["type"] == "status" && value["data"]["phase"] == "failed"
        })
        .await?;
        assert!(failed["data"]["error"].is_string());
        let item = server
            .manager
            .import_profile_yaml("mode: rule\n".into(), "Events import".into())
            .await?;
        let uid = item.uid.context("missing UID")?;
        let profiles = until(&mut events, |value| value["type"] == "profiles").await?;
        assert_eq!(
            profiles["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["uid"] == uid.as_str())
                .unwrap()["uid"],
            uid.as_str()
        );
        events.close(None).await?;
        server.manager.select_profile(uid.to_string()).await?;
        let mut reconnected = server.authenticated("/api/events").await?;
        let latest = receive(&mut reconnected).await?;
        assert_eq!(latest["profiles"]["current"], uid.as_str());
        assert_eq!(latest["status"]["active_profile"], uid.as_str());
        assert!(latest["status"]["config_revision"].is_string());
        reconnected
            .send(Message::Text(json!({"type":"start"}).to_string().into()))
            .await?;
        // The event socket cannot execute mutations; protocol misuse closes it.
        timeout(Duration::from_secs(3), async {
            loop {
                if matches!(
                    reconnected.next().await,
                    None | Some(Ok(Message::Close(_))) | Some(Err(_))
                ) {
                    break;
                }
            }
        })
        .await?;
        assert!(server.manager.status().pid.is_none());
        assert!(server.directory.0.join("profiles.yaml").exists());
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = server.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires a real core and TCP/Unix socket binding permissions"]
async fn live_streams_forward_and_cancel_independently_then_reconnect_after_core_restart() -> Result<()> {
    let server = Server::new(true).await?;
    let result = async {
        let mut events = server.authenticated("/api/events").await?;
        receive(&mut events).await?;
        let yaml = "mixed-port: 0\nmode: rule\nlog-level: debug\nexternal-controller: ''\ndns: {enable: false}\ntun: {enable: false}\nproxy-groups:\n  - {name: Main, type: select, proxies: [DIRECT, REJECT]}\nrules: ['MATCH,Main']\n";
        let item = server.manager.import_profile_yaml(yaml.into(), "Realtime".into()).await?;
        server.manager.select_profile(item.uid.context("missing UID")?.to_string()).await?;
        let started = server.manager.start().await?;
        let old_pid = started.pid.context("core PID missing")?;
        let mut saw_log = false;
        let mut saw_running = false;
        timeout(Duration::from_secs(5), async {
            while !saw_log || !saw_running {
                let value = receive(&mut events).await?;
                saw_log |= value["type"] == "log";
                saw_running |= value["type"] == "status" && value["data"]["phase"] == "running";
            }
            Ok::<(), anyhow::Error>(())
        }).await??;
        let mut sockets = Vec::new();
        for name in ["traffic", "memory", "connections", "connections_count", "logs", "traffic"] {
            let mut socket = server.authenticated(&format!("/api/streams/{name}")).await?;
            assert_eq!(receive(&mut socket).await?["type"], "core_state");
            sockets.push(socket);
        }
        connection_count(&server.manager, 6).await?;
        server.manager.apply_overlay(headless_core::config::runtime::parse("mode: direct")?).await?;
        for (index, socket) in sockets.iter_mut().enumerate() {
            let value = until(socket, |value| value["type"] == "data").await?;
            assert!(value["data"].is_object());
            match index {
                0 | 5 => assert!(value["data"]["up"].is_number()),
                1 => assert!(value["data"]["inuse"].is_number()),
                2 => assert!(value["data"]["downloadTotal"].is_number()),
                3 => assert!(value["data"]["count"].is_number()),
                4 => assert!(value["data"]["payload"].is_string()),
                _ => unreachable!(),
            }
        }
        sockets.pop().context("missing peer")?.close(None).await?;
        connection_count(&server.manager, 5).await?;
        until(&mut sockets[0], |value| value["type"] == "data").await?;
        let client = server.manager.client();
        let old_ids = client.connection_manager.0.iter().map(|entry| *entry.key()).collect::<Vec<_>>();
        // Simulate an internal stream closure: the service retries independently.
        let id = *old_ids.first().context("missing subscription ID")?;
        client.cancel_ws_connection(id);
        timeout(Duration::from_secs(5), async {
            while client.connection_manager.0.contains_key(&id) || client.connection_manager.0.len() != 5 {
                sleep(Duration::from_millis(20)).await;
            }
        }).await?;
        server.manager.stop().await?;
        connection_count(&server.manager, 0).await?;
        until(&mut sockets[0], |value| value["type"] == "core_state" && value["data"]["phase"] == "stopped").await?;
        let restarted = server.manager.start().await?;
        assert_ne!(restarted.pid, Some(old_pid));
        assert!(restarted.generation > started.generation);
        connection_count(&server.manager, 5).await?;
        until(&mut sockets[0], |value| value["type"] == "core_state" && value["data"]["phase"] == "running").await?;
        until(&mut sockets[0], |value| value["type"] == "data").await?;
        assert!(old_ids.iter().all(|id| !client.connection_manager.0.contains_key(id)));
        server.state.close();
        timeout(Duration::from_secs(4), server.state.drain_websockets()).await?;
        connection_count(&server.manager, 0).await?;
        Ok::<(), anyhow::Error>(())
    }.await;
    let cleanup = server.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn interface_preferences_are_pushed_to_event_and_preference_feeds() -> Result<()> {
    let server = Server::new(false).await?;
    let mut events = server.authenticated("/api/events").await?;
    let snapshot = until(&mut events, |value| value["type"] == "snapshot").await?;
    assert_eq!(snapshot["preferences"], json!({"language": null}));
    let mut feed = server.authenticated("/api/streams/preferences").await?;
    assert_eq!(
        receive(&mut feed).await?,
        json!({"type":"preferences","data":{"language":null}})
    );

    let set = reqwest::Client::new()
        .post(format!("http://{}/api/commands", server.address))
        .bearer_auth(&server.token)
        .json(&json!({"command":"set_language","language":"zhtw"}))
        .send()
        .await?;
    ensure!(set.status().is_success(), "set_language failed: {}", set.status());
    let expected = json!({"type":"preferences","data":{"language":"zhtw"}});
    assert_eq!(receive(&mut feed).await?, expected);
    assert_eq!(
        until(&mut events, |value| value["type"] == "preferences").await?,
        expected
    );

    // The shared client (used by the desktop client) authenticates and reads the same feed.
    let endpoint = management_client::Endpoint::new(server.address, None, server.directory.0.join("management-token"))?;
    let mut client = management_client::events::Feed::connect(&endpoint, &server.token, Some("preferences")).await?;
    assert_eq!(
        client.next().await?,
        Some(json!({"type":"preferences","data":{"language":"zhtw"}}))
    );
    let wrong = management_client::events::Feed::connect(&endpoint, &"0".repeat(64), Some("preferences")).await;
    assert!(wrong.is_err(), "a wrong token is refused");
    drop((events, feed, client));
    server.shutdown().await
}
