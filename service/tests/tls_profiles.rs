#![cfg(unix)]
#[path = "support/tls.rs"]
mod tls;
use anyhow::{Context as _, Result};
use headless_core::config::profile_store::{ProfilePatch, RemoteOptionsPatch};
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions},
    remote::{RemoteOptions, download},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tls::{Directory, Fixture};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    time::{Instant, timeout},
};
fn bypass() -> RemoteOptions {
    RemoteOptions {
        danger_accept_invalid_certs: Some(true),
        ..Default::default()
    }
}
#[tokio::test]
async fn untrusted_and_wrong_hostname_fail_twice_by_default_but_explicit_bypass_is_saved() -> Result<()> {
    let fixture = Fixture::new().await?;
    let error = download(&fixture.url, None, RemoteOptions::default())
        .await
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("static webpki roots fallback failed"), "{message}");
    assert!(!message.contains("private-test-token"));
    assert!(!message.contains(&fixture.url));
    assert_eq!(fixture.state.connections.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.state.count(), 0);
    let profile = download(&fixture.url, Some("TLS"), bypass()).await?;
    assert_eq!(fixture.state.connections.load(Ordering::SeqCst), 3);
    assert_eq!(profile.option.danger_accept_invalid_certs, Some(true));
    assert_eq!(fixture.state.count(), 1);
    let headers = fixture.state.requests.lock().unwrap()[0].to_ascii_lowercase();
    assert!(!headers.contains("\r\nauthorization:"));
    assert!(!headers.contains("proxy-authorization:"));
    Ok(())
}
#[tokio::test]
async fn tls_retry_shares_one_total_deadline() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.state.handshake_delay_ms.store(650, Ordering::SeqCst);
    let start = Instant::now();
    let error = download(
        &fixture.url,
        None,
        RemoteOptions {
            timeout_seconds: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("timed out"));
    assert!(start.elapsed() < Duration::from_millis(1300));
    assert_eq!(fixture.state.connections.load(Ordering::SeqCst), 2);
    Ok(())
}
#[tokio::test]
async fn legacy_tls_alert_has_actionable_error_and_never_retries() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("https://{}/?token=secret", listener.local_addr()?);
    let count = Arc::new(AtomicUsize::new(0));
    let shared = Arc::clone(&count);
    let task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            shared.fetch_add(1, Ordering::SeqCst);
            let mut hello = [0_u8; 8192];
            let _ = socket.read(&mut hello).await;
            let _ = socket.write_all(&[0x15, 0x03, 0x03, 0, 2, 2, 0x46]).await;
        }
    });
    let result = download(&url, None, RemoteOptions::default()).await;
    task.abort();
    let message = format!("{:#}", result.unwrap_err());
    assert!(message.contains("only TLS 1.2/1.3"), "{message}");
    assert!(!message.contains("static webpki"));
    assert!(!message.contains("token=secret"));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    Ok(())
}
#[tokio::test]
async fn bypass_does_not_relax_status_body_size_or_yaml_limits_or_retry_them() -> Result<()> {
    let fixture = Fixture::new().await?;
    let cases = [
        (
            "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
            "503",
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 8388609\r\nConnection: close\r\n\r\n".to_owned(),
            "8 MiB",
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]".to_owned(),
            "mapping",
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nx".to_owned(),
            "body",
        ),
    ];
    for (index, (response, expected)) in cases.into_iter().enumerate() {
        *fixture.state.response.lock().unwrap() = response;
        let message = format!("{:#}", download(&fixture.url, None, bypass()).await.unwrap_err());
        assert!(message.contains(expected), "{message}");
        assert!(!message.contains("private-test-token"));
        assert_eq!(fixture.state.connections.load(Ordering::SeqCst), index + 1);
    }
    Ok(())
}
#[tokio::test]
async fn saved_certificate_options_survive_restart_reject_stale_refresh_and_cancel_on_shutdown() -> Result<()> {
    let fixture = Fixture::new().await?;
    let directory = Directory::new()?;
    let options = CoreOptions::new(
        directory.validator()?,
        directory.0.clone(),
        directory.0.join("missing.yaml"),
    );
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let catalog = serde_json::to_value(manager.profiles())?;
        assert!(
            manager
                .import_remote_profile(fixture.url.clone(), None, RemoteOptions::default())
                .await
                .is_err()
        );
        assert_eq!(serde_json::to_value(manager.profiles())?, catalog);
        let profile = manager
            .import_remote_profile(fixture.url.clone(), Some("TLS".into()), bypass())
            .await?;
        let uid = profile.uid.as_deref().unwrap().to_string();
        let file = directory.0.join("profiles").join(profile.file.as_deref().unwrap());
        let raw = std::fs::read(&file)?;
        let linked = profile.option.as_ref().unwrap().clone();
        fixture.state.hold.store(true, Ordering::SeqCst);
        let clone = manager.clone();
        let id = uid.clone();
        let pending = tokio::spawn(async move { clone.refresh_profile(id).await });
        fixture.state.wait(2).await?;
        manager
            .edit_profile(
                uid.clone(),
                ProfilePatch {
                    options: Some(RemoteOptionsPatch {
                        danger_accept_invalid_certs: Some(false),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        let catalog = serde_json::to_value(manager.profiles())?;
        fixture.state.release.add_permits(1);
        let message = format!("{:#}", timeout(Duration::from_secs(5), pending).await??.unwrap_err());
        assert!(message.contains("changed during download"));
        assert_eq!(std::fs::read(&file)?, raw);
        assert_eq!(serde_json::to_value(manager.profiles())?, catalog);
        assert!(manager.refresh_profile(uid.clone()).await.is_err());
        assert_eq!(std::fs::read(&file)?, raw);
        manager
            .edit_profile(
                uid.clone(),
                ProfilePatch {
                    options: Some(RemoteOptionsPatch {
                        danger_accept_invalid_certs: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        let current = manager
            .profiles()
            .items
            .unwrap()
            .into_iter()
            .find(|p| p.uid.as_deref() == Some(&uid))
            .unwrap()
            .option
            .unwrap();
        assert_eq!(current.merge, linked.merge);
        assert_eq!(current.script, linked.script);
        assert_eq!(current.rules, linked.rules);
        assert_eq!(current.proxies, linked.proxies);
        assert_eq!(current.groups, linked.groups);
        manager.shutdown().await?;
        Ok::<_, anyhow::Error>((uid, file, raw))
    }
    .await;
    let cleanup = manager.shutdown().await;
    cleanup?;
    let (uid, file, raw) = result?;
    let restored = CoreManager::spawn(options)?;
    let result = async {
        let profile = restored
            .profiles()
            .items
            .unwrap()
            .into_iter()
            .find(|p| p.uid.as_deref() == Some(&uid))
            .context("saved profile")?;
        assert_eq!(profile.option.unwrap().danger_accept_invalid_certs, Some(true));
        let count = fixture.state.count();
        let clone = restored.clone();
        let pending = tokio::spawn(async move { clone.refresh_profile(uid).await });
        fixture.state.wait(count + 1).await?;
        timeout(Duration::from_secs(1), restored.shutdown()).await??;
        assert!(timeout(Duration::from_secs(1), pending).await??.is_err());
        assert_eq!(std::fs::read(file)?, raw);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn http_redirect_to_https_keeps_explicit_certificate_mode_and_redacts_target() -> Result<()> {
    use axum::{Router, response::Redirect, routing::get};
    let fixture = Fixture::new().await?;
    let target = fixture.url.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/", listener.local_addr()?);
    let task = tokio::spawn(
        axum::serve(
            listener,
            Router::new().route(
                "/",
                get(move || {
                    let target = target.clone();
                    async move { Redirect::temporary(&target) }
                }),
            ),
        )
        .into_future(),
    );
    let strict = download(&url, None, RemoteOptions::default()).await;
    let bypassed = download(&url, None, bypass()).await;
    task.abort();
    let message = format!("{:#}", strict.unwrap_err());
    assert!(message.contains("static webpki roots fallback failed"));
    assert!(!message.contains("private-test-token"));
    bypassed?;
    assert_eq!(fixture.state.connections.load(Ordering::SeqCst), 3);
    assert_eq!(fixture.state.count(), 1);
    Ok(())
}
