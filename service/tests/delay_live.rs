#![cfg(target_os = "linux")]
//! Live core proxy providers and delay testing integration tests.
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

async fn wait_proxy_providers(manager: &CoreManager) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(providers) = manager.proxy_providers().await
                && let Some(provider) = providers.providers.get("remote_proxies")
                && !provider.proxies.is_empty()
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .context("HTTP proxy provider did not finish initialization")?
}

#[tokio::test]
#[ignore = "requires MIHOMO_TEST_BINARY and real Mihomo"]
async fn proxy_providers_and_delay_testing_with_live_core() -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = Directory(std::env::temp_dir().join(format!("ms-delay-live-{}-{stamp:x}", std::process::id())));
    fs::create_dir_all(&directory.0)?;

    let app = axum::Router::new()
        .route(
            "/proxies",
            axum::routing::get(|| async {
                "proxies:\n  - name: ProviderNode\n    type: socks5\n    server: 127.0.0.1\n    port: 1080\n"
            }),
        )
        .route("/delay", axum::routing::get(|| async { StatusCode::NO_CONTENT }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let server_url = format!("http://{}", listener.local_addr()?);
    let server = Server(tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    }));

    let config = format!(
        "mode: rule\nmixed-port: 0\ngeodata-mode: false\ngeo-auto-update: false\ndns: {{enable: false}}\ntun: {{enable: false}}\nproxy-providers:\n  remote_proxies:\n    type: http\n    url: '{server_url}/proxies'\n    interval: 86400\n    path: providers/remote.yaml\n    health-check:\n      enable: true\n      url: '{server_url}/delay'\n      interval: 300\nproxies:\n  - name: StaticNode\n    type: socks5\n    server: 127.0.0.1\n    port: 1081\nproxy-groups:\n  - name: MainGroup\n    type: select\n    use:\n      - remote_proxies\n    proxies:\n      - StaticNode\n      - DIRECT\nrules:\n  - 'MATCH,DIRECT'\n"
    );
    fs::write(directory.0.join("bootstrap.yaml"), &config)?;

    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let mut options = CoreOptions::new(binary, directory.0.clone(), directory.0.join("bootstrap.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    let manager = CoreManager::spawn(options)?;

    let auth = directory.authentication()?;
    let token = directory.token()?;
    let http_app = router(HttpState::new(Management::new(manager.clone(), auth)));

    let test_url = format!("{server_url}/delay");

    let result = async {
        manager.start().await?;
        wait_proxy_providers(&manager).await?;

        // 1. Direct CoreManager proxy_providers query
        let providers = manager.proxy_providers().await?;
        assert!(providers.providers.contains_key("remote_proxies"));
        let remote = &providers.providers["remote_proxies"];
        assert_eq!(remote.name, "remote_proxies");
        assert_eq!(remote.vehicle_type, mihomo_client::models::VehicleType::HTTP);

        // 2. Direct CoreManager update and healthcheck
        manager.update_proxy_provider("remote_proxies").await?;
        manager.healthcheck_proxy_provider("remote_proxies").await?;

        // 3. Direct CoreManager delay testing
        let direct_delay = manager.delay_proxy("DIRECT", Some(&test_url), Some(2000)).await?;
        // DIRECT against local HTTP fixture should succeed
        assert!(direct_delay.delay < 2000);

        let group_delay = manager.delay_group("MainGroup", Some(&test_url), Some(2000)).await?;
        assert!(group_delay.contains_key("DIRECT"));

        // 4. HTTP management API query for proxy_providers
        let req = Request::builder()
            .method("POST")
            .uri("/api/commands")
            .header(header::HOST, "127.0.0.1:9090")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"command": "proxy_providers"}).to_string()))?;
        let res = http_app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(&to_bytes(res.into_body(), usize::MAX).await?)?;
        assert!(body["providers"]["remote_proxies"].is_object());

        // 5. HTTP management API update_proxy_provider
        let req = Request::builder()
            .method("POST")
            .uri("/api/commands")
            .header(header::HOST, "127.0.0.1:9090")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"command": "update_proxy_provider", "name": "remote_proxies"}).to_string(),
            ))?;
        let res = http_app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::OK);
        let receipt: Value = serde_json::from_slice(&to_bytes(res.into_body(), usize::MAX).await?)?;
        assert_eq!(receipt["action"], "update");
        assert_eq!(receipt["name"], "remote_proxies");
        assert_eq!(receipt["success"], true);

        // 6. HTTP management API healthcheck_proxy_provider
        let req = Request::builder()
            .method("POST")
            .uri("/api/commands")
            .header(header::HOST, "127.0.0.1:9090")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"command": "healthcheck_proxy_provider", "name": "remote_proxies"}).to_string(),
            ))?;
        let res = http_app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::OK);
        let receipt: Value = serde_json::from_slice(&to_bytes(res.into_body(), usize::MAX).await?)?;
        assert_eq!(receipt["action"], "healthcheck");
        assert_eq!(receipt["name"], "remote_proxies");
        assert_eq!(receipt["success"], true);

        // 7. HTTP management API delay_proxy
        let req = Request::builder()
            .method("POST")
            .uri("/api/commands")
            .header(header::HOST, "127.0.0.1:9090")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "command": "delay_proxy",
                    "name": "DIRECT",
                    "url": test_url,
                    "timeout": 2000
                })
                .to_string(),
            ))?;
        let res = http_app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::OK);
        let delay_res: Value = serde_json::from_slice(&to_bytes(res.into_body(), usize::MAX).await?)?;
        assert!(delay_res["delay"].is_number());

        // 8. HTTP management API delay_group
        let req = Request::builder()
            .method("POST")
            .uri("/api/commands")
            .header(header::HOST, "127.0.0.1:9090")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "command": "delay_group",
                    "group": "MainGroup",
                    "url": test_url,
                    "timeout": 2000
                })
                .to_string(),
            ))?;
        let res = http_app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::OK);
        let group_res: Value = serde_json::from_slice(&to_bytes(res.into_body(), usize::MAX).await?)?;
        assert!(group_res.is_object());
        assert!(group_res["DIRECT"].is_number());

        // 9. Non-existent provider returns error
        let req = Request::builder()
            .method("POST")
            .uri("/api/commands")
            .header(header::HOST, "127.0.0.1:9090")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"command": "update_proxy_provider", "name": "non_existent"}).to_string(),
            ))?;
        let res = http_app.oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

        Ok::<(), anyhow::Error>(())
    }
    .await;

    drop(server);
    let shutdown = manager.shutdown().await;
    result?;
    shutdown?;
    Ok(())
}
