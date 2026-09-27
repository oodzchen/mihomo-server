#![cfg(target_os = "linux")]
//! Opt-in resource readback with isolated copies of actual subscription nodes.
use anyhow::{Context as _, Result, ensure};
use mihomo_server::core_manager::{CoreManager, CoreOptions};
use ring::digest::{SHA256, digest};
use serde_yaml_ng::{Mapping, Value};
use std::{fs, os::unix::fs::DirBuilderExt as _, path::PathBuf, time::Duration};

struct Directory(PathBuf);
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
#[ignore = "requires real Mihomo, MIHOMO_REAL_PROFILE and live subscription nodes"]
async fn real_nodes_local_providers_inventory_and_https_proxy_remain_usable() -> Result<()> {
    let source = PathBuf::from(std::env::var_os("MIHOMO_REAL_PROFILE").context("set MIHOMO_REAL_PROFILE")?);
    let original = fs::read(&source)?;
    let fingerprint = digest(&SHA256, &original);
    let profile: Mapping = serde_yaml_ng::from_slice(&original)?;
    let proxies = profile
        .get("proxies")
        .and_then(Value::as_sequence)
        .context("profile needs actual inline nodes")?;
    let names: Vec<_> = proxies
        .iter()
        .filter_map(|node| node.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();
    ensure!(!names.is_empty(), "profile has no usable node names");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = Directory(std::env::temp_dir().join(format!("ms-live-resources-{}-{stamp:x}", std::process::id())));
    fs::DirBuilder::new().mode(0o700).create(&directory.0)?;
    fs::create_dir(directory.0.join("providers"))?;
    let mut provider = Mapping::new();
    provider.insert(Value::String("proxies".into()), Value::Sequence(proxies.clone()));
    fs::write(
        directory.0.join("providers/nodes.yaml"),
        serde_yaml_ng::to_string(&provider)?,
    )?;
    let provider_yaml = serde_yaml_ng::to_string(&provider)?;
    let app = axum::Router::new()
        .route(
            "/one",
            axum::routing::get(|axum::extract::State(yaml): axum::extract::State<String>| async move { yaml }),
        )
        .route(
            "/two",
            axum::routing::get(|axum::extract::State(yaml): axum::extract::State<String>| async move { yaml }),
        )
        .with_state(provider_yaml);
    let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let provider_url = format!("http://{}", provider_listener.local_addr()?);
    let _server = Server(tokio::spawn(async move {
        let _ = axum::serve(provider_listener, app).await;
    }));
    fs::write(
        directory.0.join("providers/rules.yaml"),
        "payload: ['DOMAIN,example.org']\n",
    )?;
    let geo_source = source
        .parent()
        .and_then(|path| path.parent())
        .context("profile must have a data root")?
        .join("geoip.metadb");
    let geo_bytes = fs::read(&geo_source).context("real-data test needs geoip.metadb beside profiles")?;
    let geo_fingerprint = digest(&SHA256, &geo_bytes);
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let config = format!(
        "mixed-port: {port}\nallow-lan: false\nmode: rule\ngeodata-mode: false\ngeo-auto-update: false\ngeox-url: {{mmdb: 'http://127.0.0.1:1/unavailable'}}\ndns: {{enable: false}}\ntun: {{enable: false}}\nproxy-providers:\n  nodes: {{type: file, path: providers/nodes.yaml}}\n  remote_one: {{type: http, path: ./providers/shared.yaml, url: '{provider_url}/one'}}\n  remote_two: {{type: http, path: providers/shared.yaml, url: '{provider_url}/two'}}\nrule-providers:\n  local: {{type: file, behavior: classical, path: providers/rules.yaml}}\nproxy-groups:\n  - {{name: verification, type: select, use: [nodes]}}\nrules: ['RULE-SET,local,verification', 'GEOIP,CN,REJECT,no-resolve', 'IP-CIDR,1.1.1.1/32,REJECT,no-resolve', 'MATCH,verification']\n"
    );
    let config_path = directory.0.join("bootstrap.yaml");
    fs::write(&config_path, &config)?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let bundle = directory.0.join("bundle");
    fs::create_dir_all(bundle.join("core"))?;
    fs::create_dir(bundle.join("geo"))?;
    fs::copy(&binary, bundle.join("core/verge-mihomo"))?;
    fs::copy(&geo_source, bundle.join("geo/geoip.metadb"))?;
    let hex = |bytes: &[u8]| {
        digest(&SHA256, bytes)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version":1,"target":mihomo_server::resources::TARGET,
            "core":{"version":"v1.19.31","sha256":hex(&fs::read(&binary)?)},
            "geo":{"geoip.metadb":{"bytes":geo_bytes.len(),"sha256":hex(&geo_bytes)}}
        }))?,
    )?;
    let resources = mihomo_server::resources::Resources::open(&bundle)?;
    let mut options = CoreOptions::new(binary, directory.0.clone(), config_path);
    options.resources = Some(resources.clone());
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options)?;
    let result = async {
        assert_eq!(fs::read(directory.0.join("geoip.metadb"))?, geo_bytes);
        assert!(!directory.0.join(".geo-seed").exists());
        let profile = manager
            .import_profile_yaml(config.clone(), "isolated live providers".into())
            .await?;
        let uid = profile.uid.context("import did not assign UID")?.to_string();
        manager.select_profile(uid.clone()).await?;
        manager.start().await?;
        let inventory = manager.resource_inventory().await?;
        assert_eq!(inventory.config_revision, manager.status().config_revision);
        assert_eq!(inventory.providers.len(), 4);
        for provider in &inventory.providers {
            assert_eq!(provider.state, mihomo_server::resource_inventory::FileState::Available);
            assert!(!provider.conflict);
            assert!(provider.bytes.unwrap() > 0);
        }
        let committed = manager.runtime_config().await?;
        let one = committed["proxy-providers"]["remote_one"]["path"].as_str().unwrap();
        let two = committed["proxy-providers"]["remote_two"]["path"].as_str().unwrap();
        assert_ne!(one, two);
        assert!(one.starts_with("provider-cache/v1/") && two.starts_with("provider-cache/v1/"));
        assert!(directory.0.join(one).is_file() && directory.0.join(two).is_file());
        assert!(!directory.0.join("providers/shared.yaml").exists());
        assert_eq!(manager.profile_raw(uid.clone()).await?.yaml, config);
        let providers = manager.client().get_proxy_providers().await?;
        assert_eq!(providers.providers["remote_one"].proxies.len(), names.len());
        assert_eq!(providers.providers["remote_two"].proxies.len(), names.len());
        let before = manager.status();
        assert!(
            manager
                .apply_overlay(serde_yaml_ng::from_str(
                    "proxy-providers: {remote_one: {path: ../outside.yaml}}"
                )?)
                .await
                .is_err()
        );
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.profile_raw(uid).await?.yaml, config);
        if directory.0.join("geoip.metadb").is_file() {
            let geo = inventory
                .geo
                .iter()
                .find(|resource| resource.name == "geoip.metadb")
                .unwrap();
            assert_eq!(geo.state, mihomo_server::resource_inventory::FileState::Available);
            assert_eq!(geo.bytes, Some(fs::metadata(directory.0.join("geoip.metadb"))?.len()));
        }
        ensure!(
            manager
                .client()
                .get_proxy_providers()
                .await?
                .providers
                .contains_key("nodes"),
            "core did not load node provider"
        );
        ensure!(
            manager
                .client()
                .get_rule_providers()
                .await?
                .providers
                .contains_key("local"),
            "core did not load rule provider"
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .proxy(reqwest::Proxy::http(format!("http://127.0.0.1:{port}"))?)
            .timeout(Duration::from_secs(12))
            .build()?;
        // IP-only request triggers the Geo matcher; both possible rules reject it
        // before any outbound connection, so this check needs no network access.
        let _ = client.get("http://1.1.1.1/").send().await;
        ensure!(
            manager
                .logs()
                .iter()
                .any(|log| log.message.contains("Load MMDB file:") && log.message.contains("geoip.metadb")),
            "core did not exercise the seeded MMDB loader"
        );
        let mut successful = false;
        for name in names.iter().take(5) {
            manager.select_node("verification".into(), name.clone()).await?;
            if client
                .get("https://cp.cloudflare.com/generate_204")
                .send()
                .await
                .is_ok_and(|response| response.status().as_u16() == 204)
            {
                successful = true;
                break;
            }
        }
        ensure!(successful, "no HTTPS 204 response through the tested live nodes");
        let geo_runtime: headless_core::config::settings::RuntimeSettings = serde_yaml_ng::from_str("tcp-concurrent: true\nfind-process-mode: off\nkeep-alive-interval: 15\nkeep-alive-idle: 30\ndisable-keep-alive: false\ngeodata-mode: false\ngeodata-loader: standard\ngeosite-matcher: mph\ngeo-auto-update: false\ngeo-update-interval: 48\ngeox-url: {geoip: 'http://127.0.0.1:1/geoip', geosite: 'http://127.0.0.1:1/geosite', mmdb: 'http://127.0.0.1:1/mmdb', asn: 'http://127.0.0.1:1/asn'}")?;
        manager.set_settings(geo_runtime.clone()).await?;
        let geo_readback = manager.geo_settings().await?;
        ensure!(geo_readback.running && geo_readback.error.is_none(), "Geo actual readback unavailable");
        for field in &geo_readback.fields {
            assert!(!field.setting.is_null() && !field.configured.is_null() && !field.actual.is_null(), "{} missing Geo readback", field.key);
            assert_eq!(field.configured, field.actual, "{} differs in actual core", field.key);
            assert!(!field.mismatch);
        }
        assert_eq!(manager.settings().await?.runtime, geo_runtime);
        let connection = manager.connection_settings().await?;
        assert!(connection.running && connection.error.is_none());
        assert!(connection.fields.iter().all(|f| f.setting == f.configured && f.configured == f.actual && !f.mismatch));
        let before_check = manager.status();
        let validation = manager.validate_geo("geoip.metadb".into()).await?;
        assert_eq!(validation.format, "mmdb");
        assert_eq!(validation.verified, validation.warning.is_none());
        if let Some(warning) = validation.warning {
            assert_eq!(warning, "empty_description_structure_unverified");
        }
        assert_eq!(validation.bytes, fs::metadata(directory.0.join("geoip.metadb"))?.len());
        assert_eq!(validation.sha256, hex(&fs::read(directory.0.join("geoip.metadb"))?));
        assert_eq!(manager.status().pid, before_check.pid);
        assert_eq!(manager.status().config_revision, before_check.config_revision);
        let seed_info = manager.geo_seed_info("geoip.metadb".into()).await?;
        let mut update = mihomo_server::geo_update::InstallRequest {
            name: "geoip.metadb".into(),
            expected_current_sha256: seed_info.current_sha256,
            expected_seed_sha256: seed_info.seed_sha256,
            accept_metadata_only: false,
        };
        assert!(manager.install_geo_seed(update.clone()).await.is_err()); // Never replace under a running core.
        assert_eq!(manager.status().pid, before_check.pid);
        manager.stop().await?;
        fs::write(bundle.join("geo/geoip.metadb"), "changed bundle candidate")?;
        assert!(manager.install_geo_seed(update.clone()).await.is_err());
        assert_eq!(fs::read(directory.0.join("geoip.metadb"))?, geo_bytes);
        fs::write(bundle.join("geo/geoip.metadb"), &geo_bytes)?;
        fs::write(directory.0.join("geoip.metadb"), "damaged isolated Geo")?;
        assert!(manager.install_geo_seed(update.clone()).await.is_err()); // Reject stale current-file digest.
        update.expected_current_sha256 = manager.geo_seed_info("geoip.metadb".into()).await?.current_sha256;
        if !validation.verified {
            assert!(manager.install_geo_seed(update.clone()).await.is_err());
        }
        update.accept_metadata_only = true;
        let receipt = manager.install_geo_seed(update).await?;
        assert!(receipt.changed && receipt.durable && !receipt.cleanup_pending);
        assert_eq!(receipt.validation.sha256, hex(&geo_bytes));
        assert_eq!(receipt.validation.verified, validation.verified);
        assert_eq!(manager.status().phase, mihomo_server::core_manager::CorePhase::Stopped);
        assert_eq!(manager.status().config_revision, before_check.config_revision);
        assert!(!directory.0.join(".geo-seed").exists());
        let installed = fs::read(directory.0.join("geoip.metadb"))?;
        fs::write(bundle.join("geo/geoip.metadb"), "bundle changed after initialization")?;
        assert!(resources.initialize_geo(&directory.0)?.is_empty());
        assert_eq!(fs::read(directory.0.join("geoip.metadb"))?, installed);
        manager.start().await?;
        assert_eq!(manager.resource_inventory().await?.providers.len(), 4);
        assert_eq!(
            manager.runtime_config().await?["proxy-providers"]["remote_one"]["path"],
            one
        );
        assert_eq!(
            manager.runtime_config().await?["proxy-providers"]["remote_two"]["path"],
            two
        );
        let restored_geo = manager.geo_settings().await?;
        ensure!(restored_geo.running && restored_geo.error.is_none(), "Geo readback unavailable after restart");
        assert!(restored_geo.fields.iter().all(|field| !field.mismatch && field.setting == field.configured && field.configured == field.actual));
        ensure!(
            client
                .get("https://cp.cloudflare.com/generate_204")
                .send()
                .await
                .is_ok_and(|response| response.status().as_u16() == 204),
            "HTTPS proxy unavailable after restart"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    assert_eq!(digest(&SHA256, &fs::read(source)?).as_ref(), fingerprint.as_ref());
    assert_eq!(
        digest(&SHA256, &fs::read(geo_source)?).as_ref(),
        geo_fingerprint.as_ref()
    );
    result.and(cleanup)
}
