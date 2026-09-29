#![cfg(target_os = "linux")]
#[path = "fixtures/dat.rs"]
mod dat;
use anyhow::Result;
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions, CorePhase},
    geo::online::{Request, RouteChoice},
};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
#[ignore = "requires real Mihomo; local online Geo sources and isolated DAT core probes"]
async fn online_dat_update_guards_inputs_and_recovers_running_core_and_startup_journal() -> Result<()> {
    let dir = Directory(std::env::temp_dir().join(format!(
            "ms-geo-online-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        )));
    fs::create_dir(&dir.0)?;
    let site = Arc::new(Mutex::new(dat::geosite()));
    let ip = dat::geoip();
    let site_route = site.clone();
    let app = axum::Router::new()
        .route(
            "/site",
            axum::routing::get(move || {
                let site = site_route.clone();
                async move { site.lock().unwrap().clone() }
            }),
        )
        .route(
            "/ip",
            axum::routing::get({
                let ip = ip.clone();
                move || {
                    let ip = ip.clone();
                    async move { ip }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let binary = std::env::var("MIHOMO_TEST_BINARY").unwrap_or_else(|_| "/usr/bin/verge-mihomo".into());
    let options = CoreOptions::new(binary.into(), dir.0.clone(), dir.0.join("missing.yaml"));
    let manager = CoreManager::spawn(options.clone())?;
    let result = async {
        let raw = format!("mode: direct\ngeox-url: {{geoip: '{origin}/ip', geosite: '{origin}/site'}}\ngeo-auto-update: false\n");
        let uid = manager.import_profile_yaml(raw, "Geo update".into()).await?.uid.unwrap().to_string();
        manager.select_profile(uid).await?;
        let before = manager.status();
        let site_info = manager.geo_online_info("geosite.dat".into()).await?;
        assert_eq!(site_info.source_sha256.len(), 64);
        let old = site_info.current_sha256.clone();
        let mut request = Request { name: "geosite.dat".into(), expected_current_sha256: old.clone(), expected_source_sha256: "0".repeat(64), expected_download_sha256: None, accept_metadata_only: false, route: Default::default(), danger_accept_invalid_certs: false };
        assert!(manager.update_geo_online(request.clone()).await.is_err());
        assert_eq!(manager.geo_online_info("geosite.dat".into()).await?.current_sha256, old);
        request.expected_source_sha256 = site_info.source_sha256;
        request.expected_download_sha256 = Some("0".repeat(64));
        assert!(manager.update_geo_online(request.clone()).await.is_err());
        request.expected_download_sha256 = None;
        *site.lock().unwrap() = b"invalid DAT".to_vec();
        assert!(manager.update_geo_online(request.clone()).await.is_err());
        assert_eq!(manager.geo_online_info("geosite.dat".into()).await?.current_sha256, old);
        *site.lock().unwrap() = dat::geosite();
        let receipt = manager.update_geo_online(request.clone()).await?;
        assert!(receipt.changed && receipt.validation.verified && receipt.durable);
        assert_eq!(receipt.core_load_verified, Some(true));
        assert_eq!(fs::read(dir.0.join("geosite.dat"))?, dat::geosite());
        assert_eq!(manager.status().generation, before.generation);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert!(!dir.0.join(".geo-seed").exists());
        assert!(manager.update_geo_online(request).await.is_err());
        let mut invalid_regex = dat::group(b"ms-dat", &[dat::domain(1, b"[")]);
        invalid_regex.extend(dat::group(b"CN", &[dat::domain(3, b"bootstrap.invalid")]));
        *site.lock().unwrap() = invalid_regex;
        let site_info = manager.geo_online_info("geosite.dat".into()).await?;
        let request = Request { name: "geosite.dat".into(), expected_current_sha256: site_info.current_sha256, expected_source_sha256: site_info.source_sha256, expected_download_sha256: None, accept_metadata_only: false, route: Default::default(), danger_accept_invalid_certs: false };
        assert!(manager.update_geo_online(request).await.is_err());
        assert_eq!(fs::read(dir.0.join("geosite.dat"))?, dat::geosite());
        let ip_info = manager.geo_online_info("geoip.dat".into()).await?;
        let ip_hash = ring::digest::digest(&ring::digest::SHA256, &ip).as_ref().iter().map(|b| format!("{b:02x}")).collect::<String>();
        let request = Request { name: "geoip.dat".into(), expected_current_sha256: ip_info.current_sha256, expected_source_sha256: ip_info.source_sha256, expected_download_sha256: Some(ip_hash), accept_metadata_only: false, route: Default::default(), danger_accept_invalid_certs: false };
        let receipt = manager.update_geo_online(request.clone()).await?;
        assert_eq!(receipt.core_load_verified, Some(true));
        assert_eq!(fs::read(dir.0.join("geoip.dat"))?, ip);
        let mixed_port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let config: serde_yaml_ng::Mapping = serde_yaml_ng::from_str(&format!("mode: rule\nmixed-port: {mixed_port}\ngeodata-mode: true\ngeodata-loader: standard\ngeosite-matcher: mph\ngeo-auto-update: false\ngeox-url: {{geoip: '{origin}/ip', geosite: '{origin}/site'}}\ndns: {{enable: false}}\ntun: {{enable: false}}\nrules: ['GEOSITE,ms-dat,DIRECT', 'GEOIP,ms-dat,DIRECT,no-resolve', 'MATCH,DIRECT']\n"))?;
        manager.apply_config(config).await?;
        manager.start().await?;
        assert!(manager.status().pid.is_some());
        let running_before = manager.status();
        let mut changed_site = dat::geosite();
        changed_site.extend(dat::group(b"extra", &[dat::domain(3, b"new.example.test")]));
        *site.lock().unwrap() = changed_site.clone();
        let live_info = manager.geo_online_info("geosite.dat".into()).await?;
        let live_request = Request { name: "geosite.dat".into(), expected_current_sha256: live_info.current_sha256, expected_source_sha256: live_info.source_sha256, expected_download_sha256: None, accept_metadata_only: false, route: Default::default(), danger_accept_invalid_certs: false };
        let live = manager.update_geo_online(live_request).await?;
        assert!(live.changed && live.durable && live.core_load_verified == Some(true));
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_ne!(manager.status().pid, running_before.pid);
        assert_eq!(manager.status().config_revision, running_before.config_revision);
        assert_eq!(fs::read(dir.0.join("geosite.dat"))?, changed_site);
        assert!(!dir.0.join(".geo-live").exists());

        changed_site.extend(dat::group(b"through-proxy", &[dat::domain(3, b"proxied.example.test")]));
        *site.lock().unwrap() = changed_site.clone();
        let proxied_info = manager.geo_online_info("geosite.dat".into()).await?;
        let proxied_request = Request { name: "geosite.dat".into(), expected_current_sha256: proxied_info.current_sha256, expected_source_sha256: proxied_info.source_sha256, expected_download_sha256: None, accept_metadata_only: false, route: RouteChoice::Managed, danger_accept_invalid_certs: false };
        let proxied = manager.update_geo_online(proxied_request.clone()).await?;
        assert!(proxied.changed && proxied.durable);
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_eq!(fs::read(dir.0.join("geosite.dat"))?, changed_site);

        let mut incompatible = dat::group(b"other", &[dat::domain(3, b"unreferenced.example")]);
        incompatible.extend(dat::group(b"CN", &[dat::domain(3, b"bootstrap.invalid")]));
        *site.lock().unwrap() = incompatible;
        let failed_info = manager.geo_online_info("geosite.dat".into()).await?;
        let failed_request = Request { name: "geosite.dat".into(), expected_current_sha256: failed_info.current_sha256, expected_source_sha256: failed_info.source_sha256, expected_download_sha256: None, accept_metadata_only: false, route: Default::default(), danger_accept_invalid_certs: false };
        assert!(manager.update_geo_online(failed_request).await.is_err());
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_eq!(fs::read(dir.0.join("geosite.dat"))?, changed_site);
        assert!(!dir.0.join(".geo-live").exists());
        manager.stop().await?;
        let stopped_info = manager.geo_online_info("geosite.dat".into()).await?;
        let stopped_request = Request { expected_current_sha256: stopped_info.current_sha256, expected_source_sha256: stopped_info.source_sha256, ..proxied_request };
        assert!(format!("{:#}", manager.update_geo_online(stopped_request).await.unwrap_err()).contains("running core"));
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    if result.is_ok() && cleanup.is_ok() {
        use std::os::unix::fs::DirBuilderExt as _;
        let name = "geosite.dat";
        let old = fs::read(dir.0.join(name))?;
        let old_hash = ring::digest::digest(&ring::digest::SHA256, &old)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let candidate = b"interrupted candidate";
        let candidate_hash = ring::digest::digest(&ring::digest::SHA256, candidate)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let journal = dir.0.join(".geo-live");
        fs::DirBuilder::new().mode(0o700).create(&journal)?;
        fs::write(journal.join(name), &old)?;
        fs::write(
            journal.join("pending"),
            serde_json::json!({
                "name": name, "previous_sha256": old_hash, "candidate_sha256": candidate_hash,
            })
            .to_string(),
        )?;
        fs::write(dir.0.join("replacement.tmp"), candidate)?;
        fs::rename(dir.0.join("replacement.tmp"), dir.0.join(name))?;
        let recovered = CoreManager::spawn(options)?;
        assert_eq!(fs::read(dir.0.join(name))?, old);
        assert!(!journal.exists());
        recovered.shutdown().await?;
    }
    server.abort();
    result.and(cleanup)
}
