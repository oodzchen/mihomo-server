#![cfg(target_os = "linux")]
#[path = "fixtures/dat.rs"]
mod fixtures;
use anyhow::{Context as _, Result};
use mihomo_server::core_manager::{CoreManager, CoreOptions};
use std::{fs, path::PathBuf, time::Duration};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Servers(Vec<tokio::task::JoinHandle<()>>);
impl Drop for Servers {
    fn drop(&mut self) {
        for server in &self.0 {
            server.abort();
        }
    }
}

#[tokio::test]
#[ignore = "requires real Mihomo; isolated DAT rules and loopback HTTP proxies"]
async fn dat_validation_preserves_running_core_and_fixtures_match_both_loaders_and_matchers() -> Result<()> {
    let dir = Directory(std::env::temp_dir().join(format!(
            "ms-dat-core-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        )));
    fs::create_dir(&dir.0)?;
    let geoip = fixtures::geoip();
    let geosite = fixtures::geosite();
    fs::write(dir.0.join("geoip.dat"), &geoip)?;
    fs::write(dir.0.join("geosite.dat"), &geosite)?;
    let mut servers = Servers(Vec::new());
    let mut proxies = String::new();
    for name in ["Sites", "IPs", "Other"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        servers.0.push(tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    async fn header(socket: &mut tokio::net::TcpStream) -> Result<Vec<u8>> {
                        let mut bytes = Vec::new();
                        while !bytes.ends_with(b"\r\n\r\n") {
                            anyhow::ensure!(bytes.len() < 8192, "proxy fixture header exceeds bounds");
                            bytes.push(socket.read_u8().await?);
                        }
                        Ok(bytes)
                    }
                    let result = async {
                        let first = header(&mut socket).await?;
                        if first.starts_with(b"CONNECT ") {
                            socket.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
                            header(&mut socket).await?;
                        }
                        socket
                            .write_all(
                                format!(
                                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{name}",
                                    name.len()
                                )
                                .as_bytes(),
                            )
                            .await?;
                        Ok::<_, anyhow::Error>(())
                    };
                    let _ = tokio::time::timeout(Duration::from_secs(5), result).await;
                });
            }
        }));
        proxies.push_str(&format!(
            "  - {{name: {name}, type: http, server: 127.0.0.1, port: {port}}}\n"
        ));
    }
    let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = reservation.local_addr()?.port();
    drop(reservation);
    let raw = format!(
        "mixed-port: {port}\nmode: rule\nipv6: true\ngeodata-mode: true\ngeodata-loader: standard\ngeosite-matcher: mph\ngeo-auto-update: false\ngeox-url: {{geoip: 'http://127.0.0.1:1/disabled', geosite: 'http://127.0.0.1:1/disabled'}}\ndns: {{enable: false}}\ntun: {{enable: false}}\nproxies:\n{proxies}rules: ['GEOSITE,ms-dat,Sites', 'GEOIP,ms-dat,IPs,no-resolve', 'MATCH,Other']\n"
    );
    let bootstrap = dir.0.join("bootstrap.yaml");
    fs::write(&bootstrap, &raw)?;
    let binary = std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?;
    let mut options = CoreOptions::new(binary.into(), dir.0.clone(), bootstrap);
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let uid = manager.import_profile_yaml(raw.clone(), "DAT rules fixture".into()).await?.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?; manager.start().await?;
        let client = reqwest::Client::builder().no_proxy().proxy(reqwest::Proxy::http(format!("http://127.0.0.1:{port}"))?).timeout(Duration::from_secs(3)).build()?;
        for loader in ["standard", "memconservative"] {
            for matcher in ["mph", "succinct"] {
                manager.set_settings(serde_yaml_ng::from_str(&format!("geodata-mode: true\ngeodata-loader: {loader}\ngeosite-matcher: {matcher}\ngeo-auto-update: false"))?).await?;
                let before = manager.status();
                for name in ["geoip.dat", "geosite.dat"] {
                    let report = manager.validate_geo(name.into()).await?;
                    assert!(report.verified && report.dat.is_some()); assert_eq!(report.warning, Some("dat_core_matching_unverified"));
                    assert_eq!(manager.status().pid, before.pid); assert_eq!(manager.status().generation, before.generation); assert_eq!(manager.status().config_revision, before.config_revision);
                }
                for (host, expected) in [("exact.dat.test", "Sites"), ("sub.suffix.dat.test", "Sites"), ("regex.dat.test", "Sites"), ("somekeyword.dat.test", "Sites"), ("192.0.2.55", "IPs"), ("[2001:db8::1]", "IPs"), ("unmatched.dat.test", "Other"), ("198.51.100.1", "Other")] {
                    let response = client.get(format!("http://{host}/fixture")).send().await?;
                    assert!(response.status().is_success()); assert_eq!(response.text().await?, expected, "{loader}/{matcher}: {host}");
                }
            }
        }
        // A malformed resource check is read-only and does not reload a running core.
        let before = manager.status();
        fs::write(dir.0.join("geoip.dat"), [0x0a, 0xff])?;
        assert!(manager.validate_geo("geoip.dat".into()).await.is_err());
        assert_eq!(fs::read(dir.0.join("geoip.dat"))?, [0x0a, 0xff]);
        assert_eq!(manager.status().pid, before.pid); assert_eq!(manager.status().config_revision, before.config_revision);
        fs::write(dir.0.join("geoip.dat"), &geoip)?;
        assert_eq!(manager.profile_raw(uid).await?.yaml, raw);
        manager.stop().await?;
        manager.validate_geo("geosite.dat".into()).await?;
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    result?;
    cleanup?;
    let restored = CoreManager::spawn(options)?;
    let result = async {
        restored.start().await?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .proxy(reqwest::Proxy::http(format!("http://127.0.0.1:{port}"))?)
            .timeout(Duration::from_secs(3))
            .build()?;
        assert_eq!(
            client.get("http://exact.dat.test/fixture").send().await?.text().await?,
            "Sites"
        );
        restored.validate_geo("geoip.dat".into()).await?;
        restored.validate_geo("geosite.dat".into()).await?;
        assert_eq!(fs::read(dir.0.join("geoip.dat"))?, geoip);
        assert_eq!(fs::read(dir.0.join("geosite.dat"))?, geosite);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result.and(cleanup)
}
