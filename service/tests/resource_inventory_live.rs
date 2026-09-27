#![cfg(target_os = "linux")]
//! Opt-in resource readback with isolated copies of actual subscription nodes.
use anyhow::{Context as _, Result, ensure};
use mihomo_server::core_manager::{CoreManager, CoreOptions};
use ring::digest::{SHA256, digest};
use serde_yaml_ng::{Mapping, Value};
use std::{fs, os::unix::fs::DirBuilderExt as _, path::PathBuf, time::Duration};

struct Directory(PathBuf);
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
    fs::write(
        directory.0.join("providers/rules.yaml"),
        "payload: ['DOMAIN,example.org']\n",
    )?;
    if let Some(data) = source.parent().and_then(|path| path.parent()) {
        if data.join("geoip.metadb").is_file() {
            fs::copy(data.join("geoip.metadb"), directory.0.join("geoip.metadb"))?;
        }
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let config = format!(
        "mixed-port: {port}\nallow-lan: false\nmode: rule\ndns: {{enable: false}}\ntun: {{enable: false}}\nproxy-providers:\n  nodes: {{type: file, path: providers/nodes.yaml}}\nrule-providers:\n  local: {{type: file, behavior: classical, path: providers/rules.yaml}}\nproxy-groups:\n  - {{name: verification, type: select, use: [nodes]}}\nrules: ['RULE-SET,local,verification', 'MATCH,verification']\n"
    );
    let config_path = directory.0.join("bootstrap.yaml");
    fs::write(&config_path, &config)?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let mut options = CoreOptions::new(binary, directory.0.clone(), config_path);
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options)?;
    let result = async {
        let profile = manager
            .import_profile_yaml(config, "isolated live providers".into())
            .await?;
        manager
            .select_profile(profile.uid.context("import did not assign UID")?.to_string())
            .await?;
        manager.start().await?;
        let inventory = manager.resource_inventory().await?;
        assert_eq!(inventory.config_revision, manager.status().config_revision);
        assert_eq!(inventory.providers.len(), 2);
        for provider in &inventory.providers {
            assert_eq!(provider.state, mihomo_server::resource_inventory::FileState::Available);
            assert!(!provider.conflict);
            assert!(provider.bytes.unwrap() > 0);
        }
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
        manager.restart().await?;
        assert_eq!(manager.resource_inventory().await?.providers.len(), 2);
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
    result.and(cleanup)
}
