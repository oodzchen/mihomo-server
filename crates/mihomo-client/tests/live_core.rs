use std::{net::TcpListener, path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context as _, Result, ensure};
use mihomo_client::{
    Builder, Mihomo,
    models::{ClashMode, Connections, Log, LogLevel, Memory, Protocol, Traffic, WsConnectionId},
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::{process::Command, sync::mpsc, time::timeout};

const SECRET: &str = "client-test/secret+?";
const GROUP: &str = "MVP /测试?";

struct TestDirectory(PathBuf);

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct TestCore {
    child: tokio::process::Child,
    directory: TestDirectory,
    client: Mihomo,
    config: Value,
    port: Option<u16>,
}

impl TestCore {
    async fn start(protocol: Protocol) -> Result<Self> {
        let binary =
            std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY to a real Mihomo binary")?;
        let directory =
            TestDirectory(std::env::temp_dir().join(format!("mc-{}-{}", std::process::id(), uuid::Uuid::new_v4())));
        tokio::fs::create_dir_all(&directory.0).await?;
        let mut config = json!({
            "mixed-port": 0,
            "mode": "rule",
            "log-level": "debug",
            "ipv6": false,
            "dns": {"enable": false},
            "geo-auto-update": false,
            "secret": SECRET,
            "proxy-groups": [{"name": GROUP, "type": "select", "proxies": ["DIRECT", "REJECT"]}],
            "rules": ["MATCH,DIRECT"]
        });
        let mut builder = Builder::new()
            .protocol(protocol)
            .request_timeout(Duration::from_secs(2));
        let port = match protocol {
            Protocol::Http => {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                let port = listener.local_addr()?.port();
                config["external-controller"] = json!(format!("127.0.0.1:{port}"));
                builder = builder.external_port(port).secret(SECRET);
                Some(port)
            }
            Protocol::LocalSocket => {
                let socket = directory.0.join("core.sock");
                let socket = socket.to_str().context("socket path must be UTF-8")?;
                config["external-controller"] = json!("");
                config["external-controller-unix"] = json!(socket);
                builder = builder.socket_path(socket);
                None
            }
        };
        let config_path = directory.0.join("config.yaml");
        tokio::fs::write(&config_path, serde_json::to_vec(&config)?).await?;
        let validation = timeout(
            Duration::from_secs(10),
            Command::new(&binary)
                .arg("-t")
                .arg("-d")
                .arg(&directory.0)
                .arg("-f")
                .arg(&config_path)
                .output(),
        )
        .await??;
        ensure!(
            validation.status.success(),
            "Mihomo validation failed: {} {}",
            String::from_utf8_lossy(&validation.stdout),
            String::from_utf8_lossy(&validation.stderr)
        );
        let output = std::fs::File::create(directory.0.join("core.log"))?;
        let child = Command::new(binary)
            .arg("-d")
            .arg(&directory.0)
            .arg("-f")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(output)
            .kill_on_drop(true)
            .spawn()?;
        Ok(Self {
            child,
            directory,
            client: builder.build()?,
            config,
            port,
        })
    }

    async fn verify(&mut self) -> Result<()> {
        timeout(Duration::from_secs(10), async {
            loop {
                if let Some(status) = self.child.try_wait()? {
                    anyhow::bail!(
                        "Mihomo exited {status}: {}",
                        tokio::fs::read_to_string(self.directory.0.join("core.log")).await?
                    );
                }
                // Mihomo exposes its controller before proxy/rule initialization finishes.
                if self.client.get_version().await.is_ok()
                    && self
                        .client
                        .get_proxies()
                        .await
                        .is_ok_and(|snapshot| snapshot.proxies.contains_key(GROUP))
                    && self
                        .client
                        .get_rules()
                        .await
                        .is_ok_and(|snapshot| !snapshot.rules.is_empty())
                {
                    return Ok::<(), anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await??;
        ensure!(!self.client.get_version().await?.version.is_empty());
        assert_eq!(self.client.get_base_config().await?.mode, ClashMode::Rule);
        ensure!(self.client.get_proxies().await?.proxies.contains_key(GROUP));
        ensure!(!self.client.get_groups().await?.proxies.is_empty());
        ensure!(!self.client.get_rules().await?.rules.is_empty());
        self.client.get_proxy_providers().await?;
        self.client.get_rule_providers().await?;
        self.client.get_connections().await?;
        self.client.close_all_connections().await?;
        self.client.flush_fakeip().await?;
        self.client.flush_dns().await?;
        self.client.select_node_for_group(GROUP, "REJECT").await?;
        assert_eq!(
            self.client.get_proxy_by_name(GROUP).await?.now.as_deref(),
            Some("REJECT")
        );
        ensure!(matches!(
            self.client.get_proxy_by_name("missing-proxy").await,
            Err(mihomo_client::Error::FailedResponse(_))
        ));
        self.client.patch_base_config(&json!({"mode": "global"})).await?;
        assert_eq!(self.client.get_base_config().await?.mode, ClashMode::Global);

        if let Some(port) = self.port {
            let unauthorized = Builder::new().external_port(port).secret("incorrect-secret").build()?;
            ensure!(matches!(
                unauthorized.get_version().await,
                Err(mihomo_client::Error::FailedResponse(_))
            ));
            ensure!(unauthorized.ws_traffic(|_| {}).await.is_err());
        }

        let (sender, mut traffic) = mpsc::unbounded_channel();
        let traffic_id = self
            .client
            .ws_traffic_checked(move |body| sender.send(body.into_bytes()).is_ok())
            .await?;
        let (sender, mut memory) = mpsc::unbounded_channel();
        let memory_id = self
            .client
            .ws_memory_checked(move |body| sender.send(body.into_bytes()).is_ok())
            .await?;
        let (sender, mut connections) = mpsc::unbounded_channel();
        self.client
            .ws_connections_checked(move |body| sender.send(body.into_bytes()).is_ok())
            .await?;
        let (sender, mut count) = mpsc::unbounded_channel();
        self.client
            .ws_connections_count_checked(move |body| sender.send(body.into_bytes()).is_ok())
            .await?;
        let (sender, mut logs) = mpsc::unbounded_channel();
        self.client
            .ws_logs_checked(LogLevel::DEBUG, move |body| sender.send(body.into_bytes()).is_ok())
            .await?;
        receive::<Traffic>(&mut traffic).await?;
        receive::<Memory>(&mut memory).await?;
        receive::<Connections>(&mut connections).await?;
        ensure!(receive::<Value>(&mut count).await?["count"].is_number());

        self.config["mode"] = json!("direct");
        let path = self.directory.0.join("config.yaml");
        tokio::fs::write(&path, serde_json::to_vec(&self.config)?).await?;
        self.client
            .reload_config(true, path.to_str().context("config path must be UTF-8")?)
            .await?;
        assert_eq!(self.client.get_base_config().await?.mode, ClashMode::Direct);
        receive::<Log>(&mut logs).await?;

        self.client.disconnect(traffic_id, Some(0)).await?;
        wait_disconnected(&self.client, traffic_id).await?;
        drop(memory);
        wait_disconnected(&self.client, memory_id).await?;
        let (sender, mut reconnected) = mpsc::unbounded_channel();
        self.client
            .ws_traffic_checked(move |body| sender.send(body.into_bytes()).is_ok())
            .await?;
        receive::<Traffic>(&mut reconnected).await?;
        self.client.clear_all_ws_connections()?;
        ensure!(self.client.connection_manager.0.is_empty());
        while timeout(Duration::from_secs(2), reconnected.recv()).await?.is_some() {}
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.client.clear_all_ws_connections()?;
        if self.child.try_wait()?.is_none() {
            self.child.start_kill()?;
        }
        timeout(Duration::from_secs(5), self.child.wait()).await??;
        Ok(())
    }
}

async fn receive<T: DeserializeOwned>(receiver: &mut mpsc::UnboundedReceiver<Vec<u8>>) -> Result<T> {
    let frame = timeout(Duration::from_secs(5), receiver.recv())
        .await?
        .context("subscription closed before a frame arrived")?;
    Ok(serde_json::from_slice(&frame)?)
}

async fn wait_disconnected(client: &Mihomo, id: WsConnectionId) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        while client.connection_manager.0.contains_key(&id) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

async fn exercise(protocol: Protocol) -> Result<()> {
    let mut core = TestCore::start(protocol).await?;
    let result = core.verify().await;
    if let Err(error) = &result {
        eprintln!(
            "{error:#}\n{}",
            tokio::fs::read_to_string(core.directory.0.join("core.log"))
                .await
                .unwrap_or_default()
        );
    }
    let stopped = core.stop().await;
    result.and(stopped)
}

#[tokio::test]
#[ignore = "requires MIHOMO_TEST_BINARY and permission to bind a local controller"]
async fn live_http_queries_reload_and_realtime() -> Result<()> {
    exercise(Protocol::Http).await
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires MIHOMO_TEST_BINARY and permission to bind a Unix socket"]
async fn live_unix_queries_reload_and_realtime() -> Result<()> {
    exercise(Protocol::LocalSocket).await
}
