#![cfg(unix)]

use anyhow::Result;
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
use std::{os::unix::fs::symlink, path::PathBuf};
use tower::ServiceExt as _;

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn directory() -> Result<Directory> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = Directory(std::env::temp_dir().join(format!("ms-assets-{}-{stamp:x}", std::process::id())));
    std::fs::create_dir_all(directory.0.join("web/assets"))?;
    std::fs::write(
        directory.0.join("web/index.html"),
        "<!doctype html><title>Test UI</title>",
    )?;
    std::fs::write(directory.0.join("web/assets/app.js"), "console.log('test')")?;
    Ok(directory)
}
fn manager(directory: &Directory) -> Result<(CoreManager, HttpState, String)> {
    let manager = CoreManager::spawn(CoreOptions::new(
        "missing-mihomo".into(),
        directory.0.join("data"),
        directory.0.join("missing.yaml"),
    ))?;
    let auth = Authentication::load_or_create(
        &directory.0.join("data/management-token"),
        "127.0.0.1:9090".parse()?,
        None,
    )?;
    let token = std::fs::read_to_string(directory.0.join("data/management-token"))?
        .trim()
        .to_owned();
    let state = HttpState::new(Management::new(manager.clone(), auth));
    Ok((manager, state, token))
}
fn request(path: &str, method: &str, token: Option<&str>) -> Result<Request<Body>> {
    let mut builder = Request::builder()
        .uri(path)
        .method(method)
        .header(header::HOST, "127.0.0.1:9090")
        .header(header::ACCEPT, "text/html");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    Ok(builder.body(Body::empty())?)
}

#[tokio::test]
async fn public_assets_and_navigation_preserve_authenticated_api_boundaries() -> Result<()> {
    let directory = directory()?;
    let (manager, state, token) = manager(&directory)?;
    let app = router(state.with_web_assets(&directory.0.join("web"))?);
    let result = async {
        for path in [
            "/",
            "/profiles",
            "/config",
            "/proxies",
            "/unlock",
            "/logs",
            "/settings",
            "/core",
        ] {
            let response = app.clone().oneshot(request(path, "GET", None)?).await?;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                response.headers()[header::CONTENT_TYPE]
                    .to_str()?
                    .starts_with("text/html")
            );
            assert!(response.headers().contains_key(header::CONTENT_SECURITY_POLICY));
        }
        let script = app.clone().oneshot(request("/assets/app.js?v=1", "GET", None)?).await?;
        assert_eq!(script.status(), StatusCode::OK);
        assert!(script.headers()[header::CONTENT_TYPE].to_str()?.contains("javascript"));
        let head = app.clone().oneshot(request("/settings", "HEAD", None)?).await?;
        assert_eq!(head.status(), StatusCode::OK);
        assert!(to_bytes(head.into_body(), 1024).await?.is_empty());
        for path in ["/api/status", "/api/unknown"] {
            assert_eq!(
                app.clone().oneshot(request(path, "GET", None)?).await?.status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            app.clone()
                .oneshot(request("/api/status", "GET", Some(&token))?)
                .await?
                .status(),
            StatusCode::OK
        );
        for path in ["/api/unknown", "/api/unknown.js", "/assets/missing.js", "/unknown-page"] {
            let response = app.clone().oneshot(request(path, "GET", Some(&token))?).await?;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        }
        assert_eq!(
            app.clone().oneshot(request("/profiles", "POST", None)?).await?.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        let mut wrong_host = request("/", "GET", None)?;
        wrong_host.headers_mut().insert(header::HOST, "evil.test".parse()?);
        assert_eq!(
            app.clone().oneshot(wrong_host).await?.status(),
            StatusCode::UNAUTHORIZED
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn asset_paths_cannot_escape_root_or_follow_external_links() -> Result<()> {
    let directory = directory()?;
    std::fs::write(directory.0.join("secret.txt"), "private")?;
    symlink(
        directory.0.join("secret.txt"),
        directory.0.join("web/assets/escape.txt"),
    )?;
    let (manager, state, _) = manager(&directory)?;
    let app = router(state.with_web_assets(&directory.0.join("web"))?);
    let result = async {
        for path in [
            "/../secret.txt",
            "/%2e%2e/secret.txt",
            "/assets/escape.txt",
            "/assets",
            "/%252e%252e/secret.txt",
        ] {
            assert_eq!(
                app.clone().oneshot(request(path, "GET", None)?).await?.status(),
                StatusCode::NOT_FOUND
            );
        }
        let mut request = request("/config", "GET", None)?;
        request
            .headers_mut()
            .insert(header::ACCEPT, "application/json".parse()?);
        assert_eq!(app.clone().oneshot(request).await?.status(), StatusCode::NOT_FOUND);
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn configured_assets_require_a_real_index_inside_the_resource_root() -> Result<()> {
    let directory = directory()?;
    let (manager, state, _) = manager(&directory)?;
    assert!(state.clone().with_web_assets(&directory.0.join("missing")).is_err());
    std::fs::remove_file(directory.0.join("web/index.html"))?;
    assert!(state.clone().with_web_assets(&directory.0.join("web")).is_err());
    std::fs::write(directory.0.join("external.html"), "private")?;
    symlink(directory.0.join("external.html"), directory.0.join("web/index.html"))?;
    assert!(state.with_web_assets(&directory.0.join("web")).is_err());
    manager.shutdown().await
}
