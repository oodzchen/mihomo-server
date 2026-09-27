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

#[tokio::test]
#[ignore = "requires real Mihomo; pinned DAT bundle and isolated compatibility probe"]
async fn stopped_pinned_dat_install_checks_core_and_keeps_runtime_recoverable() -> Result<()> {
    use mihomo_server::{
        geo_update::InstallRequest,
        resources::{Resources, TARGET},
    };
    use ring::digest::{SHA256, digest};
    use serde_json::json;
    let hash = |bytes: &[u8]| {
        digest(&SHA256, bytes)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let dir = Directory(std::env::temp_dir().join(format!(
            "ms-dat-install-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        )));
    let bundle = dir.0.join("bundle");
    fs::create_dir_all(bundle.join("core"))?;
    fs::create_dir_all(bundle.join("geo"))?;
    let core_source = std::env::var("MIHOMO_TEST_BINARY").unwrap_or_else(|_| "/usr/bin/verge-mihomo".into());
    let core_bytes = fs::read(&core_source)?;
    fs::write(bundle.join("core/verge-mihomo"), &core_bytes)?;
    fs::write(bundle.join("minimal.yaml"), "mode: rule\n")?;
    let ip = fixtures::geoip();
    let site = fixtures::geosite();
    fs::write(bundle.join("geo/geoip.dat"), &ip)?;
    fs::write(bundle.join("geo/geosite.dat"), &site)?;
    fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1, "target": TARGET,
            "core": {"version": "v1", "sha256": hash(&core_bytes)},
            "geo": {
                "geoip.dat": {"bytes": ip.len(), "sha256": hash(&ip)},
                "geosite.dat": {"bytes": site.len(), "sha256": hash(&site)}
            }
        }))?,
    )?;
    drop(core_bytes);
    let resources = Resources::open(&bundle)?;
    let mut options = CoreOptions::new(
        bundle.join("core/verge-mihomo"),
        dir.0.join("data"),
        dir.0.join("missing.yaml"),
    );
    options.resources = Some(resources);
    let manager = CoreManager::spawn(options)?;
    let result = async {
        for (name, bytes) in [("geoip.dat", &ip), ("geosite.dat", &site)] {
            let old = format!("old invalid {name}");
            fs::write(dir.0.join("data").join(name), old.as_bytes())?;
            let info = manager.geo_seed_info(name.into()).await?;
            assert_eq!(info.current_sha256.as_deref(), Some(hash(old.as_bytes()).as_str()));
            assert_eq!(info.seed_sha256, hash(bytes));
            let mut request = InstallRequest { name: name.into(), expected_current_sha256: info.current_sha256, expected_seed_sha256: info.seed_sha256, accept_metadata_only: false };
            let before = manager.status();
            request.expected_seed_sha256 = "0".repeat(64);
            assert!(manager.install_geo_seed(request.clone()).await.is_err());
            assert_eq!(fs::read(dir.0.join("data").join(name))?, old.as_bytes());
            request.expected_seed_sha256 = hash(bytes);
            request.accept_metadata_only = true;
            assert!(manager.install_geo_seed(request.clone()).await.is_err());
            request.accept_metadata_only = false;
            let receipt = manager.install_geo_seed(request.clone()).await?;
            assert!(receipt.changed && receipt.durable && receipt.validation.verified && !receipt.cleanup_pending);
            assert_eq!(receipt.core_load_verified, Some(true));
            assert_eq!(receipt.validation.sha256, hash(bytes));
            assert_eq!(receipt.validation.dat.as_ref().map(|d| d.has_cn_group), Some(true));
            assert_eq!(fs::read(dir.0.join("data").join(name))?, *bytes);
            assert_eq!(manager.status().generation, before.generation);
            assert_eq!(manager.status().config_revision, before.config_revision);
            assert_eq!(manager.status().pid, before.pid);
            assert!(!dir.0.join("data/.geo-seed").exists());
            assert!(manager.install_geo_seed(request.clone()).await.is_err());
            request.expected_current_sha256 = Some(hash(bytes));
            assert!(!manager.install_geo_seed(request).await?.changed);
        }
        let raw = "mode: rule\nmixed-port: 0\ngeodata-mode: true\ngeodata-loader: standard\ngeosite-matcher: mph\ngeo-auto-update: false\ngeox-url: {geoip: 'http://127.0.0.1:1/disabled', geosite: 'http://127.0.0.1:1/disabled'}\ndns: {enable: false}\ntun: {enable: false}\nrules: ['GEOSITE,ms-dat,DIRECT', 'GEOIP,ms-dat,DIRECT,no-resolve', 'MATCH,DIRECT']\n";
        let uid = manager.import_profile_yaml(raw.into(), "installed DAT".into()).await?.uid.unwrap().to_string();
        manager.select_profile(uid).await?;
        manager.start().await?;
        let running = manager.status();
        assert!(running.pid.is_some());
        let info = manager.geo_seed_info("geoip.dat".into()).await?;
        let request = InstallRequest { name: "geoip.dat".into(), expected_current_sha256: info.current_sha256, expected_seed_sha256: info.seed_sha256, accept_metadata_only: false };
        assert!(manager.install_geo_seed(request).await.is_err());
        assert_eq!(manager.status().pid, running.pid);
        manager.stop().await?;
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires real Mihomo; invalid regexp must fail isolated DAT installation"]
async fn dat_install_rejects_core_invalid_regex_without_replacing_old_resource() -> Result<()> {
    use mihomo_server::{
        geo_update::InstallRequest,
        resources::{Resources, TARGET},
    };
    use ring::digest::{SHA256, digest};
    use serde_json::json;
    let hash = |bytes: &[u8]| {
        digest(&SHA256, bytes)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let dir = Directory(std::env::temp_dir().join(format!(
            "ms-dat-bad-regex-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        )));
    let bundle = dir.0.join("bundle");
    fs::create_dir_all(bundle.join("core"))?;
    fs::create_dir_all(bundle.join("geo"))?;
    let core_bytes = fs::read(std::env::var("MIHOMO_TEST_BINARY").unwrap_or_else(|_| "/usr/bin/verge-mihomo".into()))?;
    fs::write(bundle.join("core/verge-mihomo"), &core_bytes)?;
    fs::write(bundle.join("minimal.yaml"), "mode: rule\n")?;
    let mut invalid = fixtures::group(b"ms-dat", &[fixtures::domain(1, b"[")]);
    invalid.extend(fixtures::group(b"CN", &[fixtures::domain(3, b"bootstrap.invalid")]));
    fs::write(bundle.join("geo/geosite.dat"), &invalid)?;
    fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1, "target": TARGET,
            "core": {"version": "v1", "sha256": hash(&core_bytes)},
            "geo": {"geosite.dat": {"bytes": invalid.len(), "sha256": hash(&invalid)}}
        }))?,
    )?;
    drop(core_bytes);
    let mut options = CoreOptions::new(
        bundle.join("core/verge-mihomo"),
        dir.0.join("data"),
        dir.0.join("missing.yaml"),
    );
    options.resources = Some(Resources::open(&bundle)?);
    let manager = CoreManager::spawn(options)?;
    let result = async {
        let old = b"old DAT retained";
        fs::write(dir.0.join("data/geosite.dat"), old)?;
        let info = manager.geo_seed_info("geosite.dat".into()).await?;
        let request = InstallRequest {
            name: "geosite.dat".into(),
            expected_current_sha256: info.current_sha256,
            expected_seed_sha256: info.seed_sha256,
            accept_metadata_only: false,
        };
        let before = manager.status();
        let failure = manager
            .install_geo_seed(request)
            .await
            .expect_err("invalid Go regexp must fail core probe");
        assert!(format!("{failure:#}").contains("compatibility probe failed"));
        assert_eq!(fs::read(dir.0.join("data/geosite.dat"))?, old);
        assert_eq!(manager.status().generation, before.generation);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(manager.status().pid, before.pid);
        assert!(!dir.0.join("data/.geo-seed").exists());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
