#![cfg(unix)]
use anyhow::{Context as _, Result};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use headless_core::{
    backup::{BackupManifest, MAX_ARCHIVE_BYTES},
    config::{IProfiles, settings::ServiceSettings},
};
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions, CorePhase},
    management::{
        Management,
        auth::Authentication,
        http::{HttpState, router},
    },
};
use serde_json::json;
use std::{
    fs,
    io::{Cursor, Read as _},
    os::unix::fs::{PermissionsExt as _, symlink},
    path::PathBuf,
    time::Duration,
};
use tower::ServiceExt as _;
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let p = std::env::temp_dir().join(format!("ms-backup-api-{}", &hash(&random)[..24]));
        fs::create_dir(&p)?;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700))?;
        fs::write(
            p.join("bootstrap.yaml"),
            "mode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']\n",
        )?;
        fs::write(
            p.join("validator.py"),
            "#!/usr/bin/python3\nimport sys\nsys.exit(0 if '-t' in sys.argv else 1)\n",
        )?;
        fs::set_permissions(p.join("validator.py"), fs::Permissions::from_mode(0o700))?;
        Ok(Self(p))
    }
    fn manager(&self, real: bool) -> Result<CoreManager> {
        let binary = if real {
            PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?)
        } else {
            self.0.join("validator.py")
        };
        let mut options = CoreOptions::new(binary, self.0.clone(), self.0.join("bootstrap.yaml"));
        options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
        options.policy.readiness_attempts = 40;
        options.policy.probe_interval = Duration::from_millis(30);
        CoreManager::spawn(options)
    }
    fn app(&self, manager: &CoreManager) -> Result<(Router, String)> {
        let auth = Authentication::load_or_create(&self.0.join("management-token"), "127.0.0.1:9090".parse()?, None)?;
        let token = fs::read_to_string(self.0.join("management-token"))?.trim().to_owned();
        Ok((router(HttpState::new(Management::new(manager.clone(), auth))), token))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn request(token: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/backup")
        .header(header::HOST, "127.0.0.1:9090")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}
fn inspection_request(token: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/backup/inspect")
        .header(header::HOST, "127.0.0.1:9090")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/zip")
        .body(body)
        .unwrap()
}
async fn inspection_report(
    app: &Router,
    token: &str,
    bytes: Vec<u8>,
) -> Result<headless_core::backup::BackupInspection> {
    let expected_digest = hash(&bytes);
    let expected_length = bytes.len() as u64;
    let response = app
        .clone()
        .oneshot(inspection_request(token, Body::from(bytes)))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let body = to_bytes(response.into_body(), 4096).await?;
    let report: headless_core::backup::BackupInspection = serde_json::from_slice(&body)?;
    assert_eq!(report.archive_sha256, expected_digest);
    assert_eq!(report.archive_bytes, expected_length);
    Ok(report)
}
fn inspect(bytes: Vec<u8>) -> Result<(BackupManifest, IProfiles, ServiceSettings, serde_yaml_ng::Mapping)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let manifest: BackupManifest = serde_json::from_reader(zip.by_name("manifest.json")?)?;
    manifest.validate()?;
    assert_eq!(zip.len(), manifest.entries.len() + 1);
    for entry in &manifest.entries {
        let mut data = Vec::new();
        zip.by_name(&entry.path)?.read_to_end(&mut data)?;
        assert_eq!(data.len() as u64, entry.bytes);
        assert_eq!(hash(&data), entry.sha256);
    }
    assert!(zip.by_name("management-token").is_err());
    assert!(zip.by_name("run/core.sock").is_err());
    assert!(zip.by_name("profiles/unrelated-secret.yaml").is_err());
    let profiles = serde_yaml_ng::from_reader(zip.by_name("profiles.yaml")?)?;
    let settings = serde_yaml_ng::from_reader(zip.by_name("settings.yaml")?)?;
    let runtime = serde_yaml_ng::from_reader(zip.by_name("runtime.yaml")?)?;
    Ok((manifest, profiles, settings, runtime))
}
#[tokio::test]
async fn authenticated_zip_stream_has_verified_snapshot_headers_and_holds_single_admission_until_drop_or_eof()
-> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        let profile = manager
            .import_profile_yaml("# original source\nproxies: []\n".into(), "backup source".into())
            .await?;
        manager.select_profile(profile.uid.unwrap().to_string()).await?;
        fs::write(dir.0.join("profiles/unrelated-secret.yaml"), "excluded")?;
        let before = json!(manager.profiles());
        let generation = manager.status().generation;
        let held = app.clone().oneshot(request(&token)).await?;
        assert_eq!(held.status(), StatusCode::OK);
        assert_eq!(held.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(held.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(held.headers()[header::CONTENT_TYPE], "application/zip");
        assert!(
            held.headers()[header::CONTENT_DISPOSITION]
                .to_str()?
                .starts_with("attachment; filename=\"mihomo-server-backup-")
        );
        assert_eq!(
            app.clone().oneshot(request(&token)).await?.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        // Admission precedes any body read, including an indefinitely stalled stream.
        let unread = Body::from_stream(futures_util::stream::pending::<Result<axum::body::Bytes, std::io::Error>>());
        let busy = tokio::time::timeout(
            Duration::from_secs(1),
            app.clone().oneshot(inspection_request(&token, unread)),
        )
        .await??;
        assert_eq!(busy.status(), StatusCode::CONFLICT);
        manager.settings().await?;
        assert_eq!(json!(manager.profiles()), before);
        assert_eq!(manager.status().generation, generation);
        drop(held); // A disconnected download releases admission without reading its body.
        let response = app.clone().oneshot(request(&token)).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let length = response.headers()[header::CONTENT_LENGTH].to_str()?.parse::<usize>()?;
        let digest = response.headers()["x-backup-sha256"].to_str()?.to_owned();
        let bytes = to_bytes(response.into_body(), MAX_ARCHIVE_BYTES).await?;
        assert_eq!(bytes.len(), length);
        assert_eq!(hash(&bytes), digest);
        let report = inspection_report(&app, &token, bytes.to_vec()).await?;
        assert_eq!(report.profile_count, 1);
        assert!(report.active_profile_present);
        assert!(!bytes.windows(token.len()).any(|chunk| chunk == token.as_bytes()));
        let (_, profiles, settings, runtime) = inspect(bytes.to_vec())?;
        assert_eq!(json!(profiles), before);
        assert_eq!(settings, manager.settings().await?);
        assert_eq!(runtime, manager.runtime_config().await?);
        let next = app.clone().oneshot(request(&token)).await?;
        assert_eq!(next.status(), StatusCode::OK);
        drop(next);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn inspection_upload_auth_media_length_and_integrity_errors_preserve_state_and_do_not_expose_content()
-> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        let download = manager.export_backup().await?;
        let bytes = download.bytes.clone();
        drop(download);
        let before = manager.status();
        let catalog = json!(manager.profiles());
        let settings = manager.settings().await?;
        for (case, expected) in [
            ("auth", StatusCode::UNAUTHORIZED),
            ("origin", StatusCode::UNAUTHORIZED),
            ("query", StatusCode::BAD_REQUEST),
            ("method", StatusCode::METHOD_NOT_ALLOWED),
            ("media", StatusCode::UNSUPPORTED_MEDIA_TYPE),
            ("duplicate-media", StatusCode::UNSUPPORTED_MEDIA_TYPE),
            ("encoding", StatusCode::UNSUPPORTED_MEDIA_TYPE),
            ("length", StatusCode::PAYLOAD_TOO_LARGE),
            ("mismatch", StatusCode::BAD_REQUEST),
            ("empty", StatusCode::UNPROCESSABLE_ENTITY),
            ("corrupt", StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            let mut request =
                inspection_request(if case == "auth" { "wrong" } else { &token }, Body::from(bytes.clone()));
            match case {
                "origin" => {
                    request
                        .headers_mut()
                        .insert(header::ORIGIN, "https://untrusted.invalid".parse()?);
                }
                "query" => *request.uri_mut() = "/api/backup/inspect?destination=/tmp/private".parse()?,
                "method" => *request.method_mut() = axum::http::Method::GET,
                "media" => {
                    request.headers_mut().remove(header::CONTENT_TYPE);
                }
                "duplicate-media" => {
                    request
                        .headers_mut()
                        .append(header::CONTENT_TYPE, "application/zip".parse()?);
                }
                "encoding" => {
                    request.headers_mut().insert(header::CONTENT_ENCODING, "gzip".parse()?);
                }
                "length" => {
                    request
                        .headers_mut()
                        .insert(header::CONTENT_LENGTH, (MAX_ARCHIVE_BYTES + 1).to_string().parse()?);
                }
                "mismatch" => {
                    request.headers_mut().insert(header::CONTENT_LENGTH, "1".parse()?);
                }
                "empty" => *request.body_mut() = Body::empty(),
                "corrupt" => *request.body_mut() = Body::from("PRIVATE SOURCE CONTENT /tmp/sensitive"),
                _ => {}
            }
            let response = app.clone().oneshot(request).await?;
            assert_eq!(response.status(), expected, "{case}");
            let error = to_bytes(response.into_body(), 4096).await?;
            let text = String::from_utf8_lossy(&error);
            assert!(
                !text.contains("PRIVATE SOURCE")
                    && !text.contains("sensitive")
                    && !text.contains(dir.0.to_str().unwrap())
                    && !text.contains(&token)
            );
        }
        // Chunked uploads cannot evade the binary cap; ordinary JSON keeps its 9 MiB cap.
        let chunks = futures_util::stream::iter(
            (0..66).map(|_| Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![0; 1024 * 1024]))),
        );
        let oversized = app
            .clone()
            .oneshot(inspection_request(&token, Body::from_stream(chunks)))
            .await?;
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let mut command = request(&token);
        *command.uri_mut() = "/api/commands".parse()?;
        command
            .headers_mut()
            .insert(header::CONTENT_TYPE, "application/json".parse()?);
        *command.body_mut() = Body::from(vec![b' '; mihomo_server::management::http::MAX_REQUEST_BYTES + 1]);
        assert_eq!(
            app.clone().oneshot(command).await?.status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        inspection_report(&app, &token, bytes).await?;
        assert_eq!(manager.status().generation, before.generation);
        assert_eq!(manager.status().phase, before.phase);
        assert_eq!(json!(manager.profiles()), catalog);
        assert_eq!(manager.settings().await?, settings);
        assert!(!dir.0.join("backups").exists());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn stalled_upload_holds_shared_admission_without_blocking_actor_and_disconnect_releases_it() -> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        let (polled, wait) = tokio::sync::oneshot::channel();
        let stream = futures_util::stream::once(async move {
            let _ = polled.send(());
            std::future::pending::<Result<axum::body::Bytes, std::io::Error>>().await
        });
        let upload = tokio::spawn(
            app.clone()
                .oneshot(inspection_request(&token, Body::from_stream(stream))),
        );
        tokio::time::timeout(Duration::from_secs(1), wait).await??;
        assert!(manager.export_backup().await.is_err());
        let rejected = app.clone().oneshot(inspection_request(&token, Body::empty())).await?;
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
        tokio::time::timeout(Duration::from_secs(1), manager.settings()).await??;
        upload.abort();
        assert!(upload.await.unwrap_err().is_cancelled());
        let next = manager.export_backup().await?;
        drop(next);
        // HTTP shutdown cancels a stalled body and releases admission immediately.
        let auth = Authentication::load_or_create(&dir.0.join("management-token"), "127.0.0.1:9090".parse()?, None)?;
        let state = HttpState::new(Management::new(manager.clone(), auth));
        let closing_app = router(state.clone());
        let (polled, wait) = tokio::sync::oneshot::channel();
        let stream = futures_util::stream::once(async move {
            let _ = polled.send(());
            std::future::pending::<Result<axum::body::Bytes, std::io::Error>>().await
        });
        let upload = tokio::spawn(closing_app.oneshot(inspection_request(&token, Body::from_stream(stream))));
        tokio::time::timeout(Duration::from_secs(1), wait).await??;
        state.close();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), upload).await???.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let next = manager.export_backup().await?;
        drop(next);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test(start_paused = true)]
async fn upload_deadline_releases_admission_without_waiting_for_the_sender() -> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        let (polled, wait) = tokio::sync::oneshot::channel();
        let stream = futures_util::stream::once(async move {
            let _ = polled.send(());
            std::future::pending::<Result<axum::body::Bytes, std::io::Error>>().await
        });
        let upload = tokio::spawn(app.oneshot(inspection_request(&token, Body::from_stream(stream))));
        wait.await?;
        tokio::time::advance(Duration::from_secs(15)).await;
        assert_eq!(upload.await??.status(), StatusCode::REQUEST_TIMEOUT);
        let next = manager.export_backup().await?;
        drop(next);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn valid_binary_upload_above_json_body_limit_passes_without_extracting_files() -> Result<()> {
    use headless_core::backup::BackupEntry;
    use std::io::Write as _;
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        let mut yaml = vec![b'#'; 5 * 1024 * 1024]; yaml.extend_from_slice(b"\nproxies: []\n");
        let contents = vec![
            ("profiles.yaml", b"items: [{uid: first, type: local, file: first.yaml}, {uid: second, type: local, file: second.yaml}]\n".to_vec()),
            ("settings.yaml", serde_yaml_ng::to_string(&ServiceSettings::default())?.into_bytes()),
            ("runtime.yaml", b"mode: direct\n".to_vec()),
            ("profiles/first.yaml", yaml.clone()), ("profiles/second.yaml", yaml),
        ];
        let manifest = BackupManifest { schema_version: 1, source: "mihomo-server".into(), service_version: "0.1.0".into(), created_at: 1, active_profile: None, runtime_revision: None,
            entries: contents.iter().map(|(path, data)| BackupEntry { path: (*path).into(), bytes: data.len() as u64, sha256: hash(data) }).collect() };
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, source) in contents.into_iter().chain(std::iter::once(("manifest.json", serde_json::to_vec(&manifest)?))) {
            writer.start_file(name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).unix_permissions(0o600))?;
            writer.write_all(&source)?;
        }
        let bytes = writer.finish()?.into_inner();
        assert!(bytes.len() > mihomo_server::management::http::MAX_REQUEST_BYTES);
        let before = json!(manager.profiles());
        let report = inspection_report(&app, &token, bytes).await?;
        assert_eq!(report.profile_count, 2); assert!(!report.active_profile_present);
        assert!(!dir.0.join("profiles/first.yaml").exists()); assert!(!dir.0.join("profiles/second.yaml").exists());
        assert_eq!(json!(manager.profiles()), before);
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
#[tokio::test]
async fn backup_auth_methods_queries_bodies_and_unsafe_sources_fail_without_leaking_paths() -> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        assert_eq!(
            app.clone().oneshot(request("wrong")).await?.status(),
            StatusCode::UNAUTHORIZED
        );
        let mut invalid = request(&token);
        *invalid.uri_mut() = "/api/backup?destination=/tmp/private".parse()?;
        assert_eq!(app.clone().oneshot(invalid).await?.status(), StatusCode::BAD_REQUEST);
        let mut invalid = request(&token);
        *invalid.method_mut() = axum::http::Method::GET;
        assert_eq!(
            app.clone().oneshot(invalid).await?.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        let mut invalid = request(&token);
        *invalid.body_mut() = Body::from(r#"{"destination":"/tmp/private"}"#);
        assert_eq!(
            app.clone().oneshot(invalid).await?.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let mut invalid = request(&token);
        invalid
            .headers_mut()
            .insert(header::ORIGIN, "https://untrusted.invalid".parse()?);
        assert_eq!(app.clone().oneshot(invalid).await?.status(), StatusCode::UNAUTHORIZED);
        let profile = manager
            .import_profile_yaml("proxies: []\n".into(), "source".into())
            .await?;
        let path = dir.0.join("profiles").join(profile.file.unwrap().as_str());
        fs::remove_file(&path)?;
        symlink(dir.0.join("management-token"), &path)?;
        let response = app.clone().oneshot(request(&token)).await?;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let error = to_bytes(response.into_body(), 4096).await?;
        assert!(!error.windows(token.len()).any(|b| b == token.as_bytes()));
        assert!(!String::from_utf8_lossy(&error).contains(dir.0.to_str().unwrap()));
        assert!(!dir.0.join("backups").exists());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; running/stopped backup snapshots and restored selections"]
async fn real_backup_exports_preserve_running_pid_config_and_saved_nodes_and_survive_restart() -> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(true)?;
    let result=async {
        let profile=manager.import_profile_yaml("mode: rule\nproxies: []\nproxy-groups: [{name: Main, type: select, proxies: [DIRECT, REJECT]}]\nrules: ['MATCH,Main']\n".into(),"backup".into()).await?;let uid=profile.uid.unwrap().to_string();manager.select_profile(uid.clone()).await?;manager.start().await?;
        tokio::time::timeout(Duration::from_secs(5),async {loop {if manager.client().get_proxies().await.is_ok_and(|p|p.proxies.contains_key("Main")){break;}tokio::time::sleep(Duration::from_millis(20)).await;}}).await?;
        manager.select_node("Main".into(),"REJECT".into()).await?;
        let before=manager.status();let config=manager.runtime_config().await?;let profiles=json!(manager.profiles());let (app,token)=dir.app(&manager)?;
        let download=manager.export_backup().await?;let (manifest,catalog,_,yaml)=inspect(download.bytes.clone())?;drop(download);
        let download=manager.export_backup().await?;let bytes=download.bytes.clone();drop(download);inspection_report(&app,&token,bytes).await?;
        assert_eq!(manifest.active_profile.as_deref(),Some(uid.as_str()));assert_eq!(manifest.runtime_revision,before.config_revision);assert_eq!(json!(catalog),profiles);assert_eq!(yaml,config);assert_eq!(manager.status().pid,before.pid);assert_eq!(manager.status().generation,before.generation);assert_eq!(manager.client().get_proxies().await?.proxies["Main"].now.as_deref(),Some("REJECT"));
        manager.stop().await?;let download=manager.export_backup().await?;let bytes=download.bytes.clone();let (_,catalog,_,yaml)=inspect(bytes.clone())?;drop(download);inspection_report(&app,&token,bytes).await?;assert_eq!(json!(catalog),profiles);assert_eq!(yaml,config);assert_eq!(manager.status().phase,CorePhase::Stopped);
        Ok::<_,anyhow::Error>((config,profiles))
    }.await;
    let cleanup = manager.shutdown().await;
    let (config, profiles) = result?;
    cleanup?;
    drop(manager);
    let restarted = dir.manager(true)?;
    let result = async {
        let download = restarted.export_backup().await?;
        let bytes = download.bytes.clone();
        let (_, catalog, _, yaml) = inspect(download.bytes.clone())?;
        drop(download);
        let (app, token) = dir.app(&restarted)?;
        inspection_report(&app, &token, bytes).await?;
        assert_eq!(json!(catalog), profiles);
        assert_eq!(yaml, config);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restarted.shutdown().await;
    result.and(cleanup)
}
