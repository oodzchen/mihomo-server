#![cfg(target_os = "linux")]
//! Real-core ownership transitions; all resources are private disposable fixtures.
use anyhow::{Context as _, Result};
use mihomo_server::core_manager::{CoreManager, CoreOptions};
use std::{fs, path::PathBuf};

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn wait_rules(manager: &CoreManager) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if manager.client().get_rule_providers().await?.providers["remote_rules"].rule_count == 1 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .context("HTTP rule provider did not finish initialization")?
}

#[tokio::test]
#[ignore = "requires MIHOMO_TEST_BINARY and real Mihomo"]
async fn single_provider_source_changes_do_not_reuse_old_nodes_and_restart_reuses_owned_cache() -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = Directory(std::env::temp_dir().join(format!("ms-provider-owned-{}-{stamp:x}", std::process::id())));
    fs::create_dir(&directory.0)?;
    let app = axum::Router::new()
        .route(
            "/one",
            axum::routing::get(|| async { "proxies: [{name: Alpha, type: direct}]\n" }),
        )
        .route(
            "/two",
            axum::routing::get(|| async { "proxies: [{name: Beta, type: direct}]\n" }),
        )
        .route(
            "/rules",
            axum::routing::get(|| async { "payload:\n  - DOMAIN,example.org\n" }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let mut server = Server(tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    }));
    let config = |endpoint| {
        format!(
            "mode: rule\nmixed-port: 0\ngeodata-mode: false\ngeo-auto-update: false\ndns: {{enable: false}}\ntun: {{enable: false}}\nproxy-providers:\n  remote: {{type: http, path: providers/shared.yaml, url: '{url}/{endpoint}', interval: 86400}}\nrule-providers:\n  remote_rules: {{type: http, url: '{url}/rules', behavior: classical, interval: 86400}}\nproxy-groups: [{{name: Main, type: select, use: [remote]}}]\nrules: ['RULE-SET,remote_rules,Main', 'MATCH,Main']\n"
        )
    };
    fs::write(directory.0.join("bootstrap.yaml"), config("one"))?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let mut options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("bootstrap.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let one = manager
            .import_profile_yaml(config("one"), "one".into())
            .await?
            .uid
            .unwrap()
            .to_string();
        let two = manager
            .import_profile_yaml(config("two"), "two".into())
            .await?
            .uid
            .unwrap()
            .to_string();
        manager.select_profile(one.clone()).await?;
        manager.start().await?;
        let rules = manager.runtime_config().await?["rule-providers"]["remote_rules"]["path"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(rules.starts_with("provider-cache/v1/"));
        let rule_bytes = fs::read_to_string(directory.0.join(&rules))?;
        anyhow::ensure!(
            rule_bytes.contains("example.org"),
            "unexpected rule cache: {rule_bytes}"
        );
        wait_rules(&manager).await?;
        assert_eq!(
            manager.client().get_proxy_providers().await?.providers["remote"].proxies[0].name,
            "Alpha"
        );
        let a = manager.runtime_config().await?["proxy-providers"]["remote"]["path"]
            .as_str()
            .unwrap()
            .to_owned();
        let a_bytes = fs::read(directory.0.join(&a))?;
        manager.select_profile(two.clone()).await?;
        let b = manager.runtime_config().await?["proxy-providers"]["remote"]["path"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(a, b);
        assert_eq!(
            manager.client().get_proxy_providers().await?.providers["remote"].proxies[0].name,
            "Beta"
        );
        assert_eq!(fs::read(directory.0.join(&a))?, a_bytes);
        let b_bytes = fs::read(directory.0.join(&b))?;
        assert_ne!(a_bytes, b_bytes);
        assert!(!directory.0.join("providers/shared.yaml").exists());
        // A failed candidate with another source cannot overwrite either cache.
        let before = manager.status();
        let mut invalid = manager.runtime_config().await?;
        invalid["proxy-providers"]["remote"]["url"] = format!("{url}/missing").into();
        invalid.insert("rules".into(), serde_yaml_ng::to_value(["INVALID,DIRECT"])?);
        assert!(manager.edit_config(invalid).await.is_err());
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(fs::read(directory.0.join(&a))?, a_bytes);
        assert_eq!(fs::read(directory.0.join(&b))?, b_bytes);
        assert_eq!(manager.profile_raw(one.clone()).await?.yaml, config("one"));
        assert_eq!(manager.profile_raw(two.clone()).await?.yaml, config("two"));
        Ok::<_, anyhow::Error>((one, two, a, b, a_bytes, b_bytes))
    }
    .await;
    let cleanup = manager.shutdown().await;
    let (one, two, a, b, a_bytes, b_bytes) = result?;
    cleanup?;
    server.0.abort();
    // Ensure the listener is closed before verifying offline restart/reselection.
    let _ = (&mut server.0).await;
    drop(server);
    let manager = CoreManager::spawn(options)?;
    let result = async {
        manager.start().await?;
        wait_rules(&manager).await?;
        assert_eq!(manager.runtime_config().await?["proxy-providers"]["remote"]["path"], b);
        assert_eq!(
            manager.client().get_proxy_providers().await?.providers["remote"].proxies[0].name,
            "Beta"
        );
        manager.select_profile(one).await?;
        assert_eq!(manager.runtime_config().await?["proxy-providers"]["remote"]["path"], a);
        assert_eq!(
            manager.client().get_proxy_providers().await?.providers["remote"].proxies[0].name,
            "Alpha"
        );
        manager.select_profile(two).await?;
        assert_eq!(manager.runtime_config().await?["proxy-providers"]["remote"]["path"], b);
        assert_eq!(fs::read(directory.0.join(a))?, a_bytes);
        assert_eq!(fs::read(directory.0.join(b))?, b_bytes);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
