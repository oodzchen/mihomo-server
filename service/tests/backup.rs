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

fn restore_request(token: &str, bytes: Vec<u8>) -> Request<Body> {
    let mut request = inspection_request(token, Body::from(bytes));
    *request.uri_mut() = "/api/backup/validate".parse().unwrap();
    request
}
async fn restore_report(
    app: &Router,
    token: &str,
    bytes: Vec<u8>,
) -> Result<headless_core::backup::BackupRestoreValidation> {
    let digest = hash(&bytes);
    let response = app.clone().oneshot(restore_request(token, bytes)).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let report: headless_core::backup::BackupRestoreValidation =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await?)?;
    assert_eq!(report.archive.archive_sha256, digest);
    Ok(report)
}
fn rewrite_archive(
    bytes: &[u8],
    change: impl FnOnce(&mut std::collections::BTreeMap<String, Vec<u8>>),
) -> Result<Vec<u8>> {
    use std::io::Write as _;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut files = std::collections::BTreeMap::new();
    for index in 0..zip.len() {
        let mut file = zip.by_index(index)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        files.insert(file.name().to_string(), data);
    }
    let mut manifest: BackupManifest = serde_json::from_slice(&files.remove("manifest.json").unwrap())?;
    change(&mut files);
    manifest.entries = files
        .iter()
        .map(|(path, data)| headless_core::backup::BackupEntry {
            path: path.clone(),
            bytes: data.len() as u64,
            sha256: hash(data),
        })
        .collect();
    files.insert("manifest.json".into(), serde_json::to_vec(&manifest)?);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in files {
        writer.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(0o600),
        )?;
        writer.write_all(&data)?;
    }
    Ok(writer.finish()?.into_inner())
}

#[tokio::test]
async fn restore_rehearsal_validates_snapshot_and_scripts_without_publishing_or_logging_uploaded_content() -> Result<()>
{
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        // A bootstrap-only backup has no profile regeneration stage.
        let download = manager.export_backup().await?; let bytes = download.bytes.clone(); drop(download);
        let report = restore_report(&app, &token, bytes).await?;
        assert!(report.regenerated_runtime_sha256.is_none());
        assert_eq!(app.clone().oneshot(restore_request("wrong", Vec::new())).await?.status(), StatusCode::UNAUTHORIZED);
        let mut invalid = restore_request(&token, Vec::new());
        invalid.headers_mut().remove(header::CONTENT_TYPE);
        assert_eq!(app.clone().oneshot(invalid).await?.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let profile = manager.import_profile_yaml("mode: rule\nproxies: []\nrules: ['MATCH,DIRECT']\n".into(), "candidate source".into()).await?;
        manager.select_profile(profile.uid.unwrap().to_string()).await?;
        let before = manager.status(); let catalog = json!(manager.profiles()); let settings = manager.settings().await?; let runtime = manager.runtime_config().await?;
        let script = manager.profiles().items.unwrap().into_iter().find(|item| item.uid.as_deref() == Some("Script")).unwrap().file.unwrap();
        let download = manager.export_backup().await?; let bytes = download.bytes.clone(); drop(download);
        let altered = rewrite_archive(&bytes, |files| { files.insert(format!("profiles/{script}"), b"function main(config) { console.log('PRIVATE_ARCHIVE_DIAGNOSTIC'); config['log-level']='debug'; return config; }".to_vec()); })?;
        inspection_report(&app, &token, altered.clone()).await?;
        let report = restore_report(&app, &token, altered).await?;
        assert!(report.regenerated_runtime_bytes.is_some());
        assert_ne!(report.regenerated_runtime_sha256.as_deref(), Some(report.runtime_sha256.as_str()));
        let logs = manager.logs().into_iter().map(|line| line.message).collect::<Vec<_>>().join("\n");
        assert!(!logs.contains("PRIVATE_ARCHIVE_DIAGNOSTIC"));
        assert_eq!(manager.status().generation, before.generation); assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(json!(manager.profiles()), catalog); assert_eq!(manager.settings().await?, settings); assert_eq!(manager.runtime_config().await?, runtime);
        assert!(!dir.0.join("backup-restore.yaml").exists());
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn restore_rehearsal_enforces_archived_authority_and_requires_fresh_provider_dns_confirmation() -> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        let profile = manager.import_profile_yaml("mode: rule\nproxies: []\ndns: {enable: true, nameserver: [8.8.8.8], nameserver-policy: {'+.example.test': 8.8.8.8}}\nrules: ['MATCH,DIRECT']\n".into(), "provider DNS".into()).await?;
        let uid = profile.uid.unwrap().to_string(); manager.select_profile(uid.clone()).await?;
        let download = manager.export_backup().await?; let bytes = download.bytes.clone(); drop(download);
        let archive = rewrite_archive(&bytes, |files| {
            let mut profile_dns = serde_json::Map::new(); profile_dns.insert(uid, json!({"enabled":true}));
            files.insert("settings.yaml".into(), serde_yaml_ng::to_string(&json!({"schema_version":1,"runtime":{"mixed-port":12345,"allow-lan":false,"tun":{"enable":false},"dns":{"enable":true,"nameserver":["1.1.1.1"]}},"profile_dns":profile_dns})).unwrap().into_bytes());
        })?;
        let script = format!("#!/usr/bin/python3\nimport sys,pathlib\np=pathlib.Path(sys.argv[sys.argv.index('-f')+1])\nif p.name=='regenerated.yaml': pathlib.Path({:?}).write_bytes(p.read_bytes())\nsys.exit(0)\n", dir.0.join("regenerated-seen.yaml").to_str().unwrap());
        fs::write(dir.0.join("validator.py"), script)?;
        let before = manager.settings().await?;
        let report = restore_report(&app, &token, archive).await?;
        assert!(report.dns_override_requires_confirmation);
        let regenerated: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&fs::read(dir.0.join("regenerated-seen.yaml"))?)?;
        assert_eq!(regenerated["mixed-port"].as_u64(), Some(12345));
        assert_eq!(regenerated["dns"]["nameserver"][0].as_str(), Some("8.8.8.8"));
        assert!(regenerated["dns"]["nameserver-policy"].as_mapping().is_some());
        assert_eq!(manager.settings().await?, before);
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn restore_probe_and_script_failures_are_sanitized_and_release_backup_admission() -> Result<()> {
    let dir = Directory::new()?;
    let mut options = CoreOptions::new(dir.0.join("validator.py"), dir.0.clone(), dir.0.join("bootstrap.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    options.policy.script_timeout = Duration::from_millis(120);
    let manager = CoreManager::spawn(options)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        let profile = manager.import_profile_yaml("proxies: []\nrules: ['MATCH,DIRECT']\n".into(), "source".into()).await?; manager.select_profile(profile.uid.unwrap().to_string()).await?;
        let script = manager.profiles().items.unwrap().into_iter().find(|item| item.uid.as_deref() == Some("Script")).unwrap().file.unwrap();
        let download = manager.export_backup().await?; let bytes = download.bytes.clone(); drop(download);
        let before = manager.status(); let original = fs::read(dir.0.join("validator.py"))?;
        for case in ["probe", "probe-mutation", "script-error", "script-loop", "provider-escape"] {
            let archive = rewrite_archive(&bytes, |files| {
                match case {
                    "script-error" => { files.insert(format!("profiles/{script}"), b"function main(config) { throw Error('PRIVATE_DIAGNOSTIC'); }".to_vec()); },
                    "script-loop" => { files.insert(format!("profiles/{script}"), b"function main(config) { while (true) {} }".to_vec()); },
                    "provider-escape" => { files.insert("runtime.yaml".into(), b"rule-providers: {escape: {type: file, behavior: domain, path: ../live.yaml}}\n".to_vec()); },
                    _ => {},
                }
            })?;
            inspection_report(&app, &token, archive.clone()).await?;
            let validator = if case == "probe" { b"#!/usr/bin/python3\nimport sys\nprint('PRIVATE_DIAGNOSTIC /tmp/private')\nsys.exit(1)\n".to_vec() }
                else if case == "probe-mutation" { b"#!/usr/bin/python3\nimport sys,pathlib\np=pathlib.Path(sys.argv[sys.argv.index('-f')+1]);p.write_text('changed')\n".to_vec() } else { original.clone() };
            fs::write(dir.0.join("validator.py"), validator)?;
            let response = tokio::time::timeout(Duration::from_secs(3), app.clone().oneshot(restore_request(&token, archive))).await??;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{case}");
            let body = to_bytes(response.into_body(), 4096).await?; let body = String::from_utf8_lossy(&body);
            assert!(!body.contains("PRIVATE_DIAGNOSTIC") && !body.contains("/tmp/") && !body.contains(&token));
            assert_eq!(manager.status().generation, before.generation); assert_eq!(manager.status().config_revision, before.config_revision);
            let download = manager.export_backup().await?; drop(download);
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
async fn restore_disconnect_http_close_and_manager_shutdown_reap_probes_and_remove_private_candidates() -> Result<()> {
    for case in ["disconnect", "http-close", "manager-shutdown"] {
        let dir = Directory::new()?;
        let manager = dir.manager(false)?;
        let auth = Authentication::load_or_create(&dir.0.join("management-token"), "127.0.0.1:9090".parse()?, None)?;
        let token = fs::read_to_string(dir.0.join("management-token"))?.trim().to_owned();
        let state = HttpState::new(Management::new(manager.clone(), auth));
        let app = router(state.clone());
        let result = async {
            let download = manager.export_backup().await?; let bytes = download.bytes.clone(); drop(download);
            let marker = dir.0.join("probe.json");
            fs::write(dir.0.join("validator.py"), format!("#!/usr/bin/python3\nimport sys,time,pathlib,json,os\npathlib.Path({:?}).write_text(json.dumps({{'pid':os.getpid(),'config':sys.argv[sys.argv.index('-f')+1],'data':sys.argv[sys.argv.index('-d')+1]}}))\ntime.sleep(60)\n", marker.to_str().unwrap()))?;
            let upload = tokio::spawn(app.oneshot(restore_request(&token, bytes)));
            let probe: serde_json::Value = tokio::time::timeout(Duration::from_secs(3), async { loop { if let Ok(data) = fs::read(&marker) && let Ok(probe) = serde_json::from_slice::<serde_json::Value>(&data) { break probe; } tokio::time::sleep(Duration::from_millis(10)).await; } }).await?;
            let temporary = PathBuf::from(probe["config"].as_str().unwrap()).parent().unwrap().to_path_buf();
            assert_eq!(fs::metadata(&temporary)?.permissions().mode() & 0o777, 0o700);
            assert!(probe["data"].as_str().unwrap().starts_with(temporary.to_str().unwrap()));
            match case {
                "disconnect" => { upload.abort(); assert!(upload.await.unwrap_err().is_cancelled()); },
                "http-close" => { state.close(); assert_eq!(upload.await??.status(), StatusCode::SERVICE_UNAVAILABLE); },
                "manager-shutdown" => { tokio::time::timeout(Duration::from_secs(3), manager.shutdown()).await??; assert_eq!(upload.await??.status(), StatusCode::SERVICE_UNAVAILABLE); },
                _ => unreachable!(),
            }
            if case != "manager-shutdown" { tokio::time::timeout(Duration::from_secs(3), manager.settings()).await??; let download = manager.export_backup().await?; drop(download); }
            assert!(!temporary.exists(), "{case}");
            let pid = probe["pid"].as_i64().unwrap() as libc::pid_t;
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "validator must be reaped");
            Ok::<_, anyhow::Error>(())
        }.await;
        let cleanup = manager.shutdown().await;
        result.and(cleanup)?;
    }
    Ok(())
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
        let download=manager.export_backup().await?;let bytes=download.bytes.clone();drop(download);
        let report=restore_report(&app,&token,bytes.clone()).await?;assert!(report.regenerated_runtime_sha256.is_some());
        let invalid=rewrite_archive(&bytes,|files| {files.insert("runtime.yaml".into(),b"mode: rule\nproxies: [{name: Invalid, type: invalid-protocol}]\nrules: ['MATCH,DIRECT']\n".to_vec());})?;
        inspection_report(&app,&token,invalid.clone()).await?;
        assert_eq!(app.clone().oneshot(restore_request(&token,invalid)).await?.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(manifest.active_profile.as_deref(),Some(uid.as_str()));assert_eq!(manifest.runtime_revision,before.config_revision);assert_eq!(json!(catalog),profiles);assert_eq!(yaml,config);assert_eq!(manager.status().pid,before.pid);assert_eq!(manager.status().generation,before.generation);assert_eq!(manager.client().get_proxies().await?.proxies["Main"].now.as_deref(),Some("REJECT"));
        manager.stop().await?;let download=manager.export_backup().await?;let bytes=download.bytes.clone();let (_,catalog,_,yaml)=inspect(bytes.clone())?;drop(download);inspection_report(&app,&token,bytes.clone()).await?;restore_report(&app,&token,bytes).await?;assert_eq!(json!(catalog),profiles);assert_eq!(yaml,config);assert_eq!(manager.status().phase,CorePhase::Stopped);
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
        inspection_report(&app, &token, bytes.clone()).await?;
        restore_report(&app, &token, bytes).await?;
        assert_eq!(json!(catalog), profiles);
        assert_eq!(yaml, config);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restarted.shutdown().await;
    result.and(cleanup)
}

fn seed_backup_restore(dir: &Directory, committed: bool) -> Result<(String, String)> {
    use headless_core::config::{
        dns::ProfileDnsSettings,
        profile_store::ProfileStore,
        runtime::{RuntimeStore, parse},
        settings::SettingsStore,
    };
    let candidate_dir = Directory::new()?;
    let mut old = ProfileStore::open(&dir.0)?;
    let mut candidate = ProfileStore::open(&candidate_dir.0)?;
    old.ensure_global_defaults()?;
    candidate.ensure_global_defaults()?;
    let old_yaml = "mode: direct\nmixed-port: 0\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']\n";
    let new_yaml = "mode: global\nmixed-port: 0\ndns: {enable: false}\ntun: {enable: false}\nproxy-groups: [{name: Main, type: select, proxies: [DIRECT, REJECT]}]\nrules: ['MATCH,Main']\n";
    let old_uid = old
        .import_local_with_defaults("previous", old_yaml)?
        .uid
        .unwrap()
        .to_string();
    let new_uid = candidate
        .import_local_with_defaults("archived", new_yaml)?
        .uid
        .unwrap()
        .to_string();
    old.set_current(Some(&old_uid))?;
    candidate.set_current(Some(&new_uid))?;
    candidate.record_selection(&new_uid, "Main", "REJECT")?;
    let mut settings = SettingsStore::open(&dir.0)?;
    let mut target = settings.snapshot();
    target
        .profile_dns
        .insert(new_uid.clone(), ProfileDnsSettings { enabled: true });
    let mut runtime = RuntimeStore::open(&dir.0)?;
    let revision = runtime.stage(parse(old_yaml)?)?;
    runtime.begin_profile(revision, Some(old_uid.clone()))?;
    runtime.commit()?;
    let revision = runtime.stage(parse(new_yaml)?)?;
    let plan = old.prepare_restore(&candidate, &settings, target, &runtime, revision.clone())?;
    runtime.begin_profile(revision, Some(new_uid.clone()))?;
    old.begin_restore(plan, &settings, &runtime)?;
    old.publish_restore(&mut settings, &runtime)?;
    if committed {
        runtime.commit()?;
    }
    Ok((old_uid, new_uid))
}
#[tokio::test]
async fn startup_recovers_backup_restore_before_dns_pruning_defaults_and_active_mirror() -> Result<()> {
    for committed in [false, true] {
        let dir = Directory::new()?;
        let (old_uid, new_uid) = seed_backup_restore(&dir, committed)?;
        let manager = dir.manager(false)?;
        let result = async {
            let expected = if committed { &new_uid } else { &old_uid };
            assert_eq!(manager.status().active_profile.as_deref(), Some(expected.as_str()));
            assert_eq!(manager.profiles().current.as_deref(), Some(expected.as_str()));
            let settings = manager.settings().await?;
            assert_eq!(settings.profile_dns.contains_key(&new_uid), committed);
            assert_eq!(
                manager.runtime_config().await?["mode"].as_str(),
                Some(if committed { "global" } else { "direct" })
            );
            assert!(!dir.0.join("backup-restore.yaml").exists());
            manager.export_backup().await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = manager.shutdown().await;
        result.and(cleanup)?;
    }
    Ok(())
}
#[tokio::test]
async fn startup_rejects_conflicting_restore_intent_and_releases_data_lock() -> Result<()> {
    let dir = Directory::new()?;
    seed_backup_restore(&dir, true)?;
    let mut catalog: IProfiles = serde_yaml_ng::from_slice(&fs::read(dir.0.join("profiles.yaml"))?)?;
    catalog.items.as_mut().unwrap()[0].name = Some("conflict".into());
    fs::write(dir.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    assert!(dir.manager(false).is_err());
    assert!(dir.0.join("backup-restore.yaml").exists());
    // A second attempt also reaches recovery rather than an unreleased ownership lock.
    let error = dir.manager(false).err().context("expected recovery failure")?;
    assert!(error.to_string().contains("restore conflicts"));
    Ok(())
}
#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; durable restore startup and saved-node recovery"]
async fn real_committed_restore_journal_recovers_config_and_saved_node_on_restart() -> Result<()> {
    let dir = Directory::new()?;
    let (_, uid) = seed_backup_restore(&dir, true)?;
    for _ in 0..2 {
        let manager = dir.manager(true)?;
        let result = async {
            assert_eq!(manager.status().active_profile.as_deref(), Some(uid.as_str()));
            manager.start().await?;
            tokio::time::timeout(Duration::from_secs(8), async {
                loop {
                    if manager.client().get_proxies().await.is_ok_and(|p| {
                        p.proxies
                            .get("Main")
                            .is_some_and(|p| p.now.as_deref() == Some("REJECT"))
                    }) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
            })
            .await?;
            assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("global"));
            assert!(!dir.0.join("backup-restore.yaml").exists());
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = manager.shutdown().await;
        result.and(cleanup)?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires MIHOMO_TEST_BINARY and MIHOMO_TEST_DATA; isolated real-node restore transaction"]
async fn real_data_restore_transaction_preserves_sources_and_proxy_traffic_after_restart() -> Result<()> {
    use anyhow::ensure;
    use headless_core::config::{
        profile_store::ProfileStore,
        runtime::{RuntimeStore, parse},
        settings::SettingsStore,
    };
    let source = PathBuf::from(std::env::var_os("MIHOMO_TEST_DATA").context("set MIHOMO_TEST_DATA")?);
    let original_catalog = fs::read(source.join("profiles.yaml"))?;
    let catalog: IProfiles = serde_yaml_ng::from_slice(&original_catalog)?;
    let item = catalog
        .items
        .as_ref()
        .context("actual catalog missing")?
        .iter()
        .find(|i| i.uid == catalog.current)
        .context("actual active source missing")?;
    let file = item.file.as_deref().context("actual source filename missing")?;
    headless_core::config::profile_store::validate_profile_file(file)?;
    let source_path = source.join("profiles").join(file);
    let raw = fs::read_to_string(&source_path)?;
    let config: serde_yaml_ng::Mapping = serde_yaml_ng::from_str(&raw)?;
    let nodes: Vec<_> = config
        .get("proxies")
        .and_then(|v| v.as_sequence())
        .context("actual nodes missing")?
        .iter()
        .filter_map(|p| {
            let name = p.get("name")?.as_str()?;
            let kind = p.get("type")?.as_str()?;
            (!matches!(kind, "direct" | "reject")
                && !["剩余", "到期", "流量", "套餐", "官网"]
                    .iter()
                    .any(|s| name.contains(s)))
            .then(|| name.to_owned())
        })
        .collect();
    ensure!(!nodes.is_empty(), "actual proxy nodes missing");
    let dir = Directory::new()?;
    let candidate_dir = Directory::new()?;
    for name in ["geoip.metadb", "GeoSite.dat", "Country.mmdb", "GeoIP.dat"] {
        if source.join(name).is_file() {
            fs::copy(source.join(name), dir.0.join(name))?;
            fs::set_permissions(dir.0.join(name), fs::Permissions::from_mode(0o600))?;
        }
    }
    let mut profiles = ProfileStore::open(&dir.0)?;
    profiles.ensure_global_defaults()?;
    let previous = profiles.import_local_with_defaults("previous", "mode: direct\nrules: ['MATCH,DIRECT']")?;
    profiles.set_current(previous.uid.as_deref())?;
    let mut runtime = RuntimeStore::open(&dir.0)?;
    let old = runtime.stage(parse(
        "mode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']",
    )?)?;
    runtime.begin_profile(old, previous.uid.map(|s| s.to_string()))?;
    runtime.commit()?;
    let mut candidate = ProfileStore::open(&candidate_dir.0)?;
    candidate.ensure_global_defaults()?;
    let item = candidate.import_local_with_defaults("actual archived source", &raw)?;
    let uid = item.uid.unwrap().to_string();
    candidate.set_current(Some(&uid))?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let mut settings = SettingsStore::open(&dir.0)?;
    let target: ServiceSettings = serde_yaml_ng::from_str(&format!(
        "schema_version: 1\nruntime:\n  mode: global\n  mixed-port: {port}\n  port: 0\n  socks-port: 0\n  redir-port: 0\n  tproxy-port: 0\n  allow-lan: false\n  dns: {{enable: false}}\n  tun: {{enable: false}}\n"
    ))?;
    let generated = headless_core::enhance::finalize::finalize(target.runtime.prepare(candidate.read_mapping(&uid)?)?);
    let revision = runtime.stage(generated)?;
    let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?);
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(binary)
            .args(["-t", "-d"])
            .arg(&dir.0)
            .arg("-f")
            .arg(runtime.path(&revision)?)
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    ensure!(output.status.success(), "actual candidate rejected by Mihomo");
    let plan = profiles.prepare_restore(&candidate, &settings, target, &runtime, revision.clone())?;
    runtime.begin_profile(revision, Some(uid.clone()))?;
    profiles.begin_restore(plan, &settings, &runtime)?;
    profiles.publish_restore(&mut settings, &runtime)?;
    runtime.commit()?;
    let mut selected = None;
    for restart in 0..2 {
        let manager = dir.manager(true)?;
        let result = async {
            ensure!(
                manager.profile_raw(uid.clone()).await?.yaml == raw,
                "archived raw source changed"
            );
            manager.start().await?;
            let choices = if restart == 0 {
                nodes.iter().take(8).cloned().collect::<Vec<_>>()
            } else {
                vec![selected.clone().context("selected node missing")?]
            };
            let mut connected = false;
            for node in choices {
                if restart == 0 {
                    manager.select_node("GLOBAL".into(), node.clone()).await?;
                } else {
                    tokio::time::timeout(Duration::from_secs(8), async {
                        loop {
                            if manager.client().get_proxies().await.is_ok_and(|p| {
                                p.proxies
                                    .get("GLOBAL")
                                    .is_some_and(|p| p.now.as_deref() == Some(node.as_str()))
                            }) {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    })
                    .await?;
                }
                let result = tokio::time::timeout(
                    Duration::from_secs(12),
                    tokio::process::Command::new("curl")
                        .args([
                            "--silent",
                            "--show-error",
                            "--noproxy",
                            "",
                            "--proxy",
                            &format!("http://127.0.0.1:{port}"),
                            "--max-time",
                            "8",
                            "--output",
                            "/dev/null",
                            "--write-out",
                            "%{http_code}",
                            "https://www.gstatic.com/generate_204",
                        ])
                        .kill_on_drop(true)
                        .output(),
                )
                .await??;
                if result.status.success() && result.stdout == b"204" {
                    connected = true;
                    selected = Some(node);
                    break;
                }
            }
            ensure!(connected, "restored actual proxy did not return HTTPS 204");
            ensure!(!dir.0.join("backup-restore.yaml").exists(), "restore cleanup pending");
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = manager.shutdown().await;
        result.and(cleanup)?;
    }
    ensure!(
        fs::read(source.join("profiles.yaml"))? == original_catalog && fs::read_to_string(source_path)? == raw,
        "original data changed"
    );
    println!(
        "isolated restore transaction: {} actual nodes; HTTPS 204 before/after restart; original source unchanged",
        nodes.len()
    );
    Ok(())
}
