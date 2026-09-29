#![cfg(target_os = "linux")]
#[path = "support/tls.rs"]
#[allow(dead_code)]
mod tls;
use anyhow::Result;
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions},
    geo::online::{Request, RouteChoice},
};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn geo_tls_fallback_keeps_source_private_and_explicit_exception_reaches_body() -> Result<()> {
    let fixture = tls::Fixture::new().await?;
    let directory = tls::Directory::new()?;
    let manager = CoreManager::spawn(CoreOptions::new(
        directory.validator()?,
        directory.0.clone(),
        directory.0.join("missing.yaml"),
    ))?;
    let result = async {
        let source = format!(
            "mode: direct\ngeox-url: {{geosite: '{}'}}\ngeo-auto-update: false\n",
            fixture.url
        );
        let uid = manager
            .import_profile_yaml(source, "Geo TLS".into())
            .await?
            .uid
            .unwrap()
            .to_string();
        manager.select_profile(uid).await?;
        let info = manager.geo_online_info("geosite.dat".into()).await?;
        let mut request = Request {
            name: "geosite.dat".into(),
            expected_current_sha256: info.current_sha256.clone(),
            expected_source_sha256: info.source_sha256,
            expected_download_sha256: None,
            accept_metadata_only: false,
            route: RouteChoice::Direct,
            danger_accept_invalid_certs: false,
        };
        let error = format!("{:#}", manager.update_geo_online(request.clone()).await.unwrap_err());
        assert!(error.contains("static webpki roots fallback failed"), "{error}");
        assert!(!error.contains("private-test-token"));
        assert_eq!(fixture.state.connections.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.state.count(), 0);
        request.danger_accept_invalid_certs = true;
        let error = format!("{:#}", manager.update_geo_online(request).await.unwrap_err());
        assert!(!error.contains("private-test-token"));
        assert!(!error.contains("static webpki roots fallback"));
        assert_eq!(fixture.state.connections.load(Ordering::SeqCst), 3);
        assert_eq!(fixture.state.count(), 1);
        assert_eq!(
            manager.geo_online_info("geosite.dat".into()).await?.current_sha256,
            info.current_sha256
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
