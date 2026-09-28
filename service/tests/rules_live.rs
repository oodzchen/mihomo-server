#![cfg(target_os = "linux")]
//! Live core rules and rule-provider integration tests.
use anyhow::{Context as _, Result};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions},
    management::{
        Management,
        auth::Authentication,
        http::{HttpState, router},
    },
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};
use tower::ServiceExt as _;

struct Directory(PathBuf);
impl Directory {
    fn authentication(&self) -> Result<Authentication> {
        Authentication::load_or_create(&self.0.join("management-token"), "127.0.0.1:9090".parse()?, None)
    }
    fn token(&self) -> Result<String> {
        Ok(fs::read_to_string(self.0.join("management-token"))?.trim().to_owned())
    }
}
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
            if let Ok(providers) = manager.rule_providers().await
                && let Some(provider) = providers.providers.get("remote_rules")
                && provider.rule_count == 1
            {
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
async fn rules_and_rule_providers_readback_and_update_with_live_core() -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = Directory(std::env::temp_dir().join(format!("ms-rules-live-{}-{stamp:x}", std::process::id())));
    fs::create_dir_all(&directory.0)?;

    let rule_content = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1));
    let rule_content_clone = rule_content.clone();

    let app = axum::Router::new().route(
        "/rules",
        axum::routing::get(move || {
            let count = rule_content_clone.load(std::sync::atomic::Ordering::SeqCst);
            async move {
                if count == 1 {
                    "payload:\n  - DOMAIN,initial.example.org\n"
                } else {
                    "payload:\n  - DOMAIN,updated.example.org\n  - DOMAIN-SUFFIX,example.com\n"
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server_url = format!("http://{}", listener.local_addr()?);
    let mut server = Server(tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    }));

    let config = format!(
        "mode: rule\nmixed-port: 0\ngeodata-mode: false\ngeo-auto-update: false\ndns: {{enable: false}}\ntun: {{enable: false}}\nrule-providers:\n  remote_rules: {{type: http, url: '{server_url}/rules', behavior: classical, interval: 86400, path: rules/remote.yaml}}\nrules:\n  - 'DOMAIN,direct.example.com,DIRECT'\n  - 'RULE-SET,remote_rules,DIRECT'\n  - 'MATCH,DIRECT'\n"
    );
    fs::write(directory.0.join("bootstrap.yaml"), &config)?;

    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let mut options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("bootstrap.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options)?;

    let auth = directory.authentication()?;
    let token = directory.token()?;
    let http_app = router(HttpState::new(Management::new(manager.clone(), auth)));

    let result = async {
        manager.start().await?;
        wait_rules(&manager).await?;

        // 1. Direct CoreManager API checks
        let rules = manager.rules().await?;
        assert!(rules.rules.len() >= 3);
        assert_eq!(rules.rules[0].payload, "direct.example.com");
        assert_eq!(rules.rules[0].proxy, "DIRECT");

        let providers = manager.rule_providers().await?;
        assert!(providers.providers.contains_key("remote_rules"));
        let p = &providers.providers["remote_rules"];
        assert_eq!(p.rule_count, 1);
        assert_eq!(p.behavior, mihomo_client::models::RuleBehavior::Classical);

        // 2. HTTP management API commands checks
        let req_rules = Request::builder()
            .method("POST")
            .uri("/api/commands")
            .header(header::HOST, "127.0.0.1:9090")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"command": "rules"}).to_string()))?;
        let res_rules = http_app.clone().oneshot(req_rules).await?;
        assert_eq!(res_rules.status(), StatusCode::OK);
        let rules_body: Value = serde_json::from_slice(&to_bytes(res_rules.into_body(), 1024 * 1024).await?)?;
        assert!(rules_body["rules"].as_array().unwrap().len() >= 3);

        let req_prov = Request::builder()
            .method("POST")
            .uri("/api/commands")
            .header(header::HOST, "127.0.0.1:9090")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"command": "rule_providers"}).to_string()))?;
        let res_prov = http_app.clone().oneshot(req_prov).await?;
        assert_eq!(res_prov.status(), StatusCode::OK);
        let prov_body: Value = serde_json::from_slice(&to_bytes(res_prov.into_body(), 1024 * 1024).await?)?;
        assert_eq!(prov_body["providers"]["remote_rules"]["ruleCount"], 1);

        // 3. Update provider through management API
        rule_content.store(2, std::sync::atomic::Ordering::SeqCst);
        let req_update = Request::builder()
            .method("POST")
            .uri("/api/commands")
            .header(header::HOST, "127.0.0.1:9090")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"command": "update_rule_provider", "name": "remote_rules"}).to_string(),
            ))?;
        let res_update = http_app.clone().oneshot(req_update).await?;
        assert_eq!(res_update.status(), StatusCode::OK);
        let update_body: Value = serde_json::from_slice(&to_bytes(res_update.into_body(), 1024 * 1024).await?)?;
        assert_eq!(update_body["updated"], true);

        // Verify updated count in rule providers
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(providers) = manager.rule_providers().await
                    && let Some(provider) = providers.providers.get("remote_rules")
                    && provider.rule_count == 2
                {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await??;

        // 4. Update non-existent provider returns error
        let req_bad = Request::builder()
            .method("POST")
            .uri("/api/commands")
            .header(header::HOST, "127.0.0.1:9090")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"command": "update_rule_provider", "name": "non_existent"}).to_string(),
            ))?;
        let res_bad = http_app.clone().oneshot(req_bad).await?;
        assert_eq!(res_bad.status(), StatusCode::UNPROCESSABLE_ENTITY);

        Ok::<_, anyhow::Error>(())
    }
    .await;

    let cleanup = manager.shutdown().await;
    server.0.abort();
    let _ = (&mut server.0).await;
    result.and(cleanup)
}
