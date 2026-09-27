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
    for route in ["rehearsal", "restore"] {
        for case in ["disconnect", "http-close", "manager-shutdown"] {
            let dir = Directory::new()?;
            let manager = dir.manager(false)?;
            let auth =
                Authentication::load_or_create(&dir.0.join("management-token"), "127.0.0.1:9090".parse()?, None)?;
            let token = fs::read_to_string(dir.0.join("management-token"))?.trim().to_owned();
            let state = HttpState::new(Management::new(manager.clone(), auth));
            let app = router(state.clone());
            let result = async {
            let download = manager.export_backup().await?; let bytes = download.bytes.clone(); drop(download);
            let marker = dir.0.join("probe.json");
            fs::write(dir.0.join("validator.py"), format!("#!/usr/bin/python3\nimport sys,time,pathlib,json,os\npathlib.Path({:?}).write_text(json.dumps({{'pid':os.getpid(),'config':sys.argv[sys.argv.index('-f')+1],'data':sys.argv[sys.argv.index('-d')+1]}}))\ntime.sleep(60)\n", marker.to_str().unwrap()))?;
            let request = if route == "restore" {apply_restore_request(&token,bytes,"archived")} else {restore_request(&token,bytes)};
            let upload = tokio::spawn(app.oneshot(request));
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
            if restart == 0 {
                let (app, token) = dir.app(&manager)?;
                let pid = manager.status().pid;
                let saved: headless_core::backup::RetainedBackupReceipt =
                    retained_json(&app, &token, "POST", "", StatusCode::CREATED).await?;
                ensure!(
                    saved.committed && !saved.durability_pending && manager.status().pid == pid,
                    "retaining changed the running core"
                );
                let response = app
                    .clone()
                    .oneshot(retained_request(&token, "GET", &format!("/{}", saved.backup.id)))
                    .await?;
                ensure!(
                    response.status() == StatusCode::OK,
                    "retained real-node download failed"
                );
                let bytes = to_bytes(response.into_body(), MAX_ARCHIVE_BYTES).await?.to_vec();
                ensure!(hash(&bytes) == saved.backup.sha256, "retained actual archive changed");
                let response = app
                    .clone()
                    .oneshot(apply_restore_request(&token, bytes.clone(), "regenerated"))
                    .await?;
                ensure!(
                    response.status() == StatusCode::OK && manager.status().pid == pid,
                    "running restore must hot reload the core"
                );
                let live: headless_core::backup::BackupRestoreReceipt =
                    serde_json::from_slice(&to_bytes(response.into_body(), 4096).await?)?;
                ensure!(
                    live.committed && live.core_running && !live.core_restarted,
                    "live restore receipt missing"
                );
                manager.stop().await?;
                let mut settings = manager.settings().await?.runtime;
                settings.mode = Some(headless_core::config::settings::Mode::Direct);
                manager.set_settings(settings).await?;
                let receipt = apply_restore_report(&app, &token, bytes, "regenerated").await?;
                ensure!(
                    receipt.committed && !receipt.cleanup_pending && manager.status().phase == CorePhase::Stopped,
                    "stopped API restore did not commit"
                );
                ensure!(
                    manager.runtime_config().await?["mode"].as_str() == Some("global"),
                    "archived mode was not restored"
                );
            }
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

fn apply_restore_request(token: &str, bytes: Vec<u8>, policy: &str) -> Request<Body> {
    let mut request = restore_request(token, bytes);
    *request.uri_mut() = "/api/backup/restore".parse().unwrap();
    request
        .headers_mut()
        .insert("x-backup-runtime", policy.parse().unwrap());
    request
}
async fn apply_restore_report(
    app: &Router,
    token: &str,
    bytes: Vec<u8>,
    policy: &str,
) -> Result<headless_core::backup::BackupRestoreReceipt> {
    let response = app.clone().oneshot(apply_restore_request(token, bytes, policy)).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    Ok(serde_json::from_slice(&to_bytes(response.into_body(), 4096).await?)?)
}
#[tokio::test]
async fn stopped_restore_publishes_explicit_runtime_policy_catalog_settings_and_persists_restart() -> Result<()> {
    for policy in ["archived", "regenerated"] {
        let dir = Directory::new()?;
        let manager = dir.manager(false)?;
        let (app, token) = dir.app(&manager)?;
        let result = async {
            let item = manager
                .import_profile_yaml(
                    "# exact archived raw\r\nmode: rule\r\nproxies: []\r\nrules: ['MATCH,DIRECT']\r\n".into(),
                    "archived profile".into(),
                )
                .await?;
            let uid = item.uid.unwrap().to_string();
            manager.select_profile(uid.clone()).await?;
            let settings = manager.settings().await?;
            let download = manager.export_backup().await?;
            let bytes = download.bytes.clone();
            drop(download);
            let archived_runtime =
                b"# exact manual runtime\r\nmode: direct\r\nlog-level: debug\r\nrules: ['MATCH,DIRECT']\r\n";
            let bytes = rewrite_archive(&bytes, |files| {
                files.insert("runtime.yaml".into(), archived_runtime.to_vec());
            })?;
            let changed = manager
                .import_profile_yaml("mode: global\n".into(), "later profile".into())
                .await?;
            manager.select_profile(changed.uid.unwrap().to_string()).await?;
            let before = manager.status().config_revision;
            let receipt = apply_restore_report(&app, &token, bytes.clone(), policy).await?;
            assert!(receipt.committed && !receipt.cleanup_pending);
            assert_eq!(receipt.archive.archive_sha256, hash(&bytes));
            assert_eq!(manager.status().phase, CorePhase::Stopped);
            assert_eq!(manager.status().active_profile.as_deref(), Some(uid.as_str()));
            assert_eq!(
                manager.status().config_revision.as_deref(),
                Some(receipt.runtime_revision.as_str())
            );
            assert_ne!(manager.status().config_revision, before);
            assert_eq!(manager.settings().await?, settings);
            assert_eq!(
                manager.profile_raw(uid.clone()).await?.yaml,
                "# exact archived raw\r\nmode: rule\r\nproxies: []\r\nrules: ['MATCH,DIRECT']\r\n"
            );
            assert!(manager.profile_raw(uid.clone()).await?.revision.starts_with("restore-"));
            let runtime = manager.runtime_config().await?;
            if policy == "archived" {
                assert_eq!(receipt.runtime_sha256, hash(archived_runtime));
                assert_eq!(runtime["mode"].as_str(), Some("direct"));
            } else {
                assert_ne!(receipt.runtime_sha256, hash(archived_runtime));
                assert_eq!(runtime["mode"].as_str(), Some("rule"));
            }
            assert!(!dir.0.join("backup-restore.yaml").exists());
            let export = manager.export_backup().await?;
            drop(export);
            Ok::<_, anyhow::Error>((uid, receipt.runtime_revision))
        }
        .await;
        let cleanup = manager.shutdown().await;
        let (uid, revision) = result?;
        cleanup?;
        let restarted = dir.manager(false)?;
        let result = async {
            assert_eq!(restarted.status().config_revision.as_deref(), Some(revision.as_str()));
            assert_eq!(restarted.status().active_profile.as_deref(), Some(uid.as_str()));
            restarted.profile_raw(uid).await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        result.and(restarted.shutdown().await)?;
    }
    Ok(())
}
#[tokio::test]
async fn restore_requires_single_explicit_policy_auth_and_bootstrap_archived_choice() -> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        let download = manager.export_backup().await?;
        let bytes = download.bytes.clone();
        drop(download);
        assert_eq!(
            app.clone()
                .oneshot(apply_restore_request("wrong", bytes.clone(), "archived"))
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );
        for value in ["unknown", "", "Archived"] {
            assert_eq!(
                app.clone()
                    .oneshot(apply_restore_request(&token, bytes.clone(), value))
                    .await?
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        let mut missing = apply_restore_request(&token, bytes.clone(), "archived");
        missing.headers_mut().remove("x-backup-runtime");
        assert_eq!(app.clone().oneshot(missing).await?.status(), StatusCode::BAD_REQUEST);
        let mut duplicate = apply_restore_request(&token, bytes.clone(), "archived");
        duplicate
            .headers_mut()
            .append("x-backup-runtime", "regenerated".parse()?);
        assert_eq!(app.clone().oneshot(duplicate).await?.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            app.clone()
                .oneshot(apply_restore_request(&token, bytes.clone(), "regenerated"))
                .await?
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(manager.status().config_revision.is_none());
        let receipt = apply_restore_report(&app, &token, bytes, "archived").await?;
        assert!(receipt.committed && !receipt.archive.active_profile_present);
        assert!(manager.status().active_profile.is_none());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(manager.shutdown().await)
}
#[tokio::test]
async fn restore_failure_and_private_source_mutation_leave_publication_unchanged_and_release_slot() -> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        let item = manager.import_profile_yaml("mode: rule\nrules: ['MATCH,DIRECT']\n".into(),"source".into()).await?;
        manager.select_profile(item.uid.unwrap().to_string()).await?;
        let download = manager.export_backup().await?; let bytes = download.bytes.clone();drop(download);
        let before=json!(manager.profiles());let settings=manager.settings().await?;let revision=manager.status().config_revision;
        for fault in ["probe-reject","source-mutation","publication-write"] {
            let script=match fault {
                "probe-reject"=>"#!/usr/bin/python3\nimport sys\nprint('PRIVATE_RESTORE_FAILURE')\nsys.exit(1)\n".into(),
                "source-mutation"=>"#!/usr/bin/python3\nimport pathlib,sys\np=pathlib.Path(sys.argv[sys.argv.index('-f')+1]).parent\nif (p/'profiles.yaml').exists(): (p/'profiles.yaml').write_text('items: []\\ncurrent: null\\n')\nsys.exit(0)\n".into(),
                "publication-write"=>format!("#!/usr/bin/python3\nimport os,sys\nos.chmod({:?},0o500)\nsys.exit(0)\n",dir.0.join("profiles").to_str().unwrap()),
                _=>unreachable!(),
            };
            fs::write(dir.0.join("validator.py"),script)?;
            let response=app.clone().oneshot(apply_restore_request(&token,bytes.clone(),"regenerated")).await?;
            assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY,"{fault}");
            let error=to_bytes(response.into_body(),4096).await?;
            assert!(!String::from_utf8_lossy(&error).contains("PRIVATE_RESTORE_FAILURE"));
            assert!(!String::from_utf8_lossy(&error).contains(dir.0.to_str().unwrap()));
            fs::set_permissions(dir.0.join("profiles"),fs::Permissions::from_mode(0o700))?;
            assert_eq!(json!(manager.profiles()),before);assert_eq!(manager.settings().await?,settings);assert_eq!(manager.status().config_revision,revision);
            assert!(!dir.0.join("backup-restore.yaml").exists());
            let export=manager.export_backup().await?;drop(export);
        }
        Ok::<_,anyhow::Error>(())
    }.await;
    result.and(manager.shutdown().await)
}
#[tokio::test]
async fn restoring_provider_dns_requires_regeneration_and_disables_imported_preference() -> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result=async {
        let item=manager.import_profile_yaml("mode: rule\ndns: {enable: false, nameserver: [8.8.8.8], nameserver-policy: {'+.example.test': 8.8.8.8}}\nrules: ['MATCH,DIRECT']\n".into(),"DNS source".into()).await?;
        let uid=item.uid.unwrap().to_string();manager.select_profile(uid.clone()).await?;
        let download=manager.export_backup().await?;let bytes=download.bytes.clone();drop(download);
        let bytes=rewrite_archive(&bytes,|files| {
            let mut settings:ServiceSettings=serde_yaml_ng::from_slice(&files["settings.yaml"]).unwrap();
            settings.runtime.dns=Some(serde_yaml_ng::from_str("enable: true\nnameserver: [1.1.1.1]").unwrap());
            settings.profile_dns.insert(uid.clone(),headless_core::config::dns::ProfileDnsSettings{enabled:true});
            files.insert("settings.yaml".into(),serde_yaml_ng::to_string(&settings).unwrap().into_bytes());
        })?;
        let before=manager.status().config_revision;
        assert_eq!(app.clone().oneshot(apply_restore_request(&token,bytes.clone(),"archived")).await?.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(manager.status().config_revision,before);
        let receipt=apply_restore_report(&app,&token,bytes,"regenerated").await?;
        assert!(receipt.dns_override_requires_confirmation);
        assert!(!manager.settings().await?.profile_dns[&uid].enabled);
        assert_eq!(manager.runtime_config().await?["dns"]["nameserver"][0].as_str(),Some("8.8.8.8"));
        let state=manager.profile_dns(uid).await?;assert!(state.source.is_some() && !state.requested && !state.enabled);
        Ok::<_,anyhow::Error>(())
    }.await;
    result.and(manager.shutdown().await)
}

#[tokio::test]
async fn committed_restore_reports_private_cleanup_failure_without_undoing_publication() -> Result<()> {
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let marker = dir.0.join("private-candidate-path");
    let result=async {
        let item=manager.import_profile_yaml("mode: rule\nrules: ['MATCH,DIRECT']\n".into(),"source".into()).await?;
        manager.select_profile(item.uid.unwrap().to_string()).await?;
        let download=manager.export_backup().await?;let bytes=download.bytes.clone();drop(download);
        let before=manager.status().config_revision;
        fs::write(dir.0.join("validator.py"),format!("#!/usr/bin/python3\nimport pathlib,sys,os\np=pathlib.Path(sys.argv[sys.argv.index('-f')+1])\nif p.name=='regenerated.yaml':\n pathlib.Path({:?}).write_text(str(p.parent))\n os.chmod(p.parent,0o500)\nsys.exit(0)\n",marker.to_str().unwrap()))?;
        let receipt=apply_restore_report(&app,&token,bytes,"regenerated").await?;
        assert!(receipt.committed && receipt.cleanup_pending);
        assert_ne!(manager.status().config_revision,before);
        assert_eq!(manager.status().config_revision.as_deref(),Some(receipt.runtime_revision.as_str()));
        assert!(manager.status().error.is_some_and(|e| e.contains("committed")));
        assert!(!dir.0.join("backup-restore.yaml").exists());
        assert_eq!(manager.runtime_config().await?["mode"].as_str(),Some("rule"));
        Ok::<_,anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    // The deliberately inaccessible candidate is test-owned and recorded by the probe.
    if let Ok(path) = fs::read_to_string(marker) {
        let path = PathBuf::from(path);
        assert!(path.file_name().unwrap().to_str().unwrap().starts_with("ms-restore-"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        fs::remove_dir_all(path)?;
    }
    result.and(cleanup)
}

fn controlled_restore_manager(dir: &Directory) -> Result<CoreManager> {
    let binary = dir.0.join("controlled-mihomo");
    fs::write(&binary, include_str!("fixtures/mihomo.py"))?;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
    let mut options = CoreOptions::new(binary, dir.0.clone(), dir.0.join("bootstrap.yaml"));
    options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
    options.policy.readiness_attempts = 30;
    options.policy.probe_interval = Duration::from_millis(20);
    options.policy.probe_timeout = Duration::from_millis(100);
    options.policy.stop_timeout = Duration::from_millis(300);
    CoreManager::spawn(options)
}
async fn assert_owned_pid_reaped(pid: u32) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if unsafe { libc::kill(pid as libc::pid_t, 0) } == -1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}
#[tokio::test]
async fn running_restore_hot_reload_or_restart_fallback_reconciles_archived_nodes() -> Result<()> {
    for reload in [true, false] {
        let dir = Directory::new()?;
        let manager = controlled_restore_manager(&dir)?;
        let (app, token) = dir.app(&manager)?;
        let result=async {
            let yaml=format!("mode: rule\nfixture-proxy-api: true\nfixture-reload: {reload}\nproxy-groups: [{{name: Main, type: select, proxies: [DIRECT, REJECT]}}]\nrules: ['MATCH,Main']\n");
            let item=manager.import_profile_yaml(yaml,"original".into()).await?;let uid=item.uid.unwrap().to_string();manager.select_profile(uid.clone()).await?;manager.start().await?;
            manager.select_node("Main".into(),"REJECT".into()).await?;
            let download=manager.export_backup().await?;let bytes=download.bytes.clone();drop(download);
            manager.select_node("Main".into(),"DIRECT".into()).await?;
            let before=manager.status();
            let receipt=apply_restore_report(&app,&token,bytes,"regenerated").await?;
            assert!(receipt.committed && receipt.core_running && !receipt.cleanup_pending);
            assert_eq!(receipt.core_restarted,!reload);
            assert_eq!(manager.status().phase,CorePhase::Running);
            assert_eq!(manager.status().active_profile.as_deref(),Some(uid.as_str()));
            assert_ne!(manager.status().config_revision,before.config_revision);
            assert_eq!(manager.client().get_proxies().await?.proxies["Main"].now.as_deref(),Some("REJECT"));
            if reload {assert_eq!(manager.status().pid,before.pid);} else {assert_ne!(manager.status().pid,before.pid);assert_owned_pid_reaped(before.pid.unwrap()).await?;}
            assert!(!dir.0.join("backup-restore.yaml").exists());
            Ok::<_,anyhow::Error>(())
        }.await;
        result.and(manager.shutdown().await)?;
    }
    Ok(())
}
#[tokio::test]
async fn running_restore_failed_restart_recovers_old_core_catalog_settings_and_nodes() -> Result<()> {
    let dir = Directory::new()?;
    let manager = controlled_restore_manager(&dir)?;
    let (app, token) = dir.app(&manager)?;
    let result=async {
        let item=manager.import_profile_yaml("mode: rule\nfixture-proxy-api: true\nfixture-reload: true\nproxy-groups: [{name: Main, type: select, proxies: [DIRECT, REJECT]}]\nrules: ['MATCH,Main']\n".into(),"original".into()).await?;
        let uid=item.uid.unwrap().to_string();manager.select_profile(uid).await?;manager.start().await?;manager.select_node("Main".into(),"REJECT".into()).await?;
        let download=manager.export_backup().await?;let bytes=download.bytes.clone();drop(download);
        let bytes=rewrite_archive(&bytes,|files| {files.insert("runtime.yaml".into(),b"mode: direct\nfixture-fail-start: true\nfixture-proxy-api: true\nrules: ['MATCH,DIRECT']\n".to_vec());})?;
        let before=manager.status();let catalog=json!(manager.profiles());let settings=manager.settings().await?;let config=manager.runtime_config().await?;
        let response=app.clone().oneshot(apply_restore_request(&token,bytes,"archived")).await?;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(manager.status().phase,CorePhase::Running);
        assert_eq!(manager.status().config_revision,before.config_revision);
        assert_eq!(manager.status().active_profile,before.active_profile);
        assert_eq!(json!(manager.profiles()),catalog);assert_eq!(manager.settings().await?,settings);assert_eq!(manager.runtime_config().await?,config);
        assert_eq!(manager.client().get_proxies().await?.proxies["Main"].now.as_deref(),Some("REJECT"));
        assert_owned_pid_reaped(before.pid.unwrap()).await?;
        assert!(!dir.0.join("backup-restore.yaml").exists());
        let download=manager.export_backup().await?;drop(download);
        Ok::<_,anyhow::Error>(())
    }.await;
    result.and(manager.shutdown().await)
}
#[tokio::test]
async fn live_restore_disconnect_http_close_and_shutdown_rollback_and_reap() -> Result<()> {
    for case in ["disconnect", "http-close", "manager-shutdown"] {
        let dir = Directory::new()?;
        let manager = controlled_restore_manager(&dir)?;
        let auth = Authentication::load_or_create(&dir.0.join("management-token"), "127.0.0.1:9090".parse()?, None)?;
        let token = fs::read_to_string(dir.0.join("management-token"))?.trim().to_owned();
        let state = HttpState::new(Management::new(manager.clone(), auth));
        let app = router(state.clone());
        let result = async {
            let item = manager
                .import_profile_yaml(
                    "mode: rule\nfixture-proxy-api: true\nfixture-reload: true\nrules: ['MATCH,DIRECT']\n".into(),
                    "original".into(),
                )
                .await?;
            manager.select_profile(item.uid.unwrap().to_string()).await?;
            manager.start().await?;
            let download = manager.export_backup().await?;
            let bytes = download.bytes.clone();
            drop(download);
            let bytes = rewrite_archive(&bytes, |files| {
                files.insert(
                    "runtime.yaml".into(),
                    b"mode: direct\nfixture-reload-delay: true\nfixture-proxy-api: true\nrules: ['MATCH,DIRECT']\n"
                        .to_vec(),
                );
            })?;
            let before = manager.status();
            let catalog = json!(manager.profiles());
            let settings = manager.settings().await?;
            let upload = tokio::spawn(app.oneshot(apply_restore_request(&token, bytes, "archived")));
            tokio::time::timeout(Duration::from_secs(3), async {
                while !dir.0.join("reload-started").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await?;
            match case {
                "disconnect" => {
                    upload.abort();
                    assert!(upload.await.unwrap_err().is_cancelled());
                }
                "http-close" => {
                    state.close();
                    assert_eq!(upload.await??.status(), StatusCode::SERVICE_UNAVAILABLE);
                }
                "manager-shutdown" => {
                    tokio::time::timeout(Duration::from_secs(3), manager.shutdown()).await??;
                    assert_eq!(upload.await??.status(), StatusCode::SERVICE_UNAVAILABLE);
                }
                _ => unreachable!(),
            }
            if case != "manager-shutdown" {
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(3), manager.settings()).await??,
                    settings
                );
                assert_eq!(manager.status().phase, CorePhase::Running);
                assert_eq!(manager.status().config_revision, before.config_revision);
                assert_eq!(json!(manager.profiles()), catalog);
                assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("rule"));
            } else {
                let store = headless_core::config::runtime::RuntimeStore::open(&dir.0)?;
                assert_eq!(store.state().current.map(|r| r.file), before.config_revision);
            }
            assert_owned_pid_reaped(before.pid.unwrap()).await?;
            assert!(!dir.0.join("backup-restore.yaml").exists());
            Ok::<_, anyhow::Error>(())
        }
        .await;
        result.and(manager.shutdown().await)?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires real MIHOMO_TEST_BINARY; listener restart fallback and occupied-port rollback"]
async fn real_running_restore_restarts_listener_switch_and_rolls_back_occupied_port() -> Result<()> {
    let dir = Directory::new()?;
    let reserved = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = reserved.local_addr()?.port();
    drop(reserved);
    fs::write(
        dir.0.join("bootstrap.yaml"),
        format!(
            "mode: direct\nmixed-port: {port}\nallow-lan: false\ndns: {{enable: false}}\nrules: ['MATCH,DIRECT']\n"
        ),
    )?;
    let manager = dir.manager(true)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        manager.start().await?;
        let before = manager.status();
        let download = manager.export_backup().await?;
        let bytes = download.bytes.clone();
        drop(download);
        let switched = rewrite_archive(&bytes, |files| {
            files.insert("runtime.yaml".into(), format!("mode: direct\nport: {port}\nallow-lan: false\ndns: {{enable: false}}\nrules: ['MATCH,DIRECT']\n").into_bytes());
        })?;
        let receipt = apply_restore_report(&app, &token, switched, "archived").await?;
        assert!(receipt.committed && receipt.core_running && receipt.core_restarted && !receipt.cleanup_pending);
        assert_ne!(manager.status().pid, before.pid);
        assert_owned_pid_reaped(before.pid.unwrap()).await?;
        let config = manager.client().get_base_config().await?;
        assert_eq!(config.port, port);
        assert_eq!(config.mixed_port, 0);
        tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        let before = manager.status();
        let catalog = json!(manager.profiles());
        let settings = manager.settings().await?;
        let runtime = manager.runtime_config().await?;
        let blocked = std::net::TcpListener::bind("127.0.0.1:0")?;
        let occupied = blocked.local_addr()?.port();
        let download = manager.export_backup().await?;
        let bytes = download.bytes.clone();
        drop(download);
        let conflicting = rewrite_archive(&bytes, |files| {
            files.insert("runtime.yaml".into(), format!("mode: direct\nport: {occupied}\nallow-lan: false\ndns: {{enable: false}}\nrules: ['MATCH,DIRECT']\n").into_bytes());
        })?;
        let response = app.oneshot(apply_restore_request(&token, conflicting, "archived")).await?;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(json!(manager.profiles()), catalog);
        assert_eq!(manager.settings().await?, settings);
        assert_eq!(manager.runtime_config().await?, runtime);
        assert_eq!(manager.client().get_base_config().await?.port, port);
        assert_owned_pid_reaped(before.pid.unwrap()).await?;
        tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        assert!(!dir.0.join("backup-restore.yaml").exists());
        Ok::<_, anyhow::Error>(())
    }.await;
    result.and(manager.shutdown().await)
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn killed_service_restore_candidate_is_cleaned_on_restart_without_changing_committed_data() -> Result<()> {
    use tokio::process::Command;
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let download = manager.export_backup().await?;
    let bytes = download.bytes.clone();
    drop(download);
    let revision = manager.status().config_revision;
    let settings = manager.settings().await?;
    let catalog = json!(manager.profiles());
    manager.shutdown().await?;
    let marker = dir.0.join("killed-probe.json");
    fs::write(
        dir.0.join("validator.py"),
        format!(
            "#!/usr/bin/python3\nimport sys,time,pathlib,json,os\npathlib.Path({:?}).write_text(json.dumps({{'pid':os.getpid(),'candidate':str(pathlib.Path(sys.argv[sys.argv.index('-f')+1]).parent)}}))\ntime.sleep(60)\n",
            marker.to_str().unwrap()
        ),
    )?;
    let reserved = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = reserved.local_addr()?;
    drop(reserved);
    let mut service = Command::new(env!("CARGO_BIN_EXE_mihomo-server"))
        .arg("--data-dir")
        .arg(&dir.0)
        .arg("--config")
        .arg(dir.0.join("bootstrap.yaml"))
        .arg("--mihomo")
        .arg(dir.0.join("validator.py"))
        .arg("--listen")
        .arg(address.to_string())
        .arg("--no-start")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let client = reqwest::Client::builder().no_proxy().build()?;
    let token = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(token) = fs::read_to_string(dir.0.join("management-token")) {
                let token = token.trim().to_owned();
                if client
                    .post(format!("http://{address}/api/commands"))
                    .bearer_auth(&token)
                    .json(&json!({"command":"status"}))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
                {
                    break token;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    let upload = tokio::spawn(async move {
        client
            .post(format!("http://{address}/api/backup/restore"))
            .bearer_auth(token)
            .header("Content-Type", "application/zip")
            .header("X-Backup-Runtime", "archived")
            .body(bytes)
            .send()
            .await
    });
    let probe: serde_json::Value = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(data) = fs::read(&marker)
                && let Ok(probe) = serde_json::from_slice(&data)
            {
                break probe;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let candidate = PathBuf::from(probe["candidate"].as_str().unwrap());
    assert!(candidate.starts_with(dir.0.join("restore-candidates")) && candidate.join(".lease").exists());
    service.kill().await?;
    assert!(tokio::time::timeout(Duration::from_secs(3), upload).await??.is_err());
    let probe_pid = probe["pid"].as_u64().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            // A killed grandchild may briefly be an init-owned zombie; it is no longer using the candidate.
            let status = fs::read_to_string(format!("/proc/{probe_pid}/status"));
            if status.is_err()
                || status.is_ok_and(|s| s.lines().any(|l| l.starts_with("State:") && l.contains("Z (zombie)")))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    assert!(
        candidate.exists(),
        "SIGKILL must leave a candidate for startup recovery"
    );
    let manager = dir.manager(false)?;
    let result = async {
        assert!(!candidate.exists());
        assert_eq!(fs::read_dir(dir.0.join("restore-candidates"))?.count(), 0);
        assert_eq!(manager.status().config_revision, revision);
        assert_eq!(manager.settings().await?, settings);
        assert_eq!(json!(manager.profiles()), catalog);
        assert!(!dir.0.join("backup-restore.yaml").exists());
        let download = manager.export_backup().await?;
        drop(download);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(manager.shutdown().await)
}

fn retained_request(token: &str, method: &str, suffix: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(format!("/api/backups{suffix}"))
        .header(header::HOST, "127.0.0.1:9090")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}
async fn retained_json<T: serde::de::DeserializeOwned>(
    app: &Router,
    token: &str,
    method: &str,
    suffix: &str,
    expected: StatusCode,
) -> Result<T> {
    let response = app.clone().oneshot(retained_request(token, method, suffix)).await?;
    assert_eq!(response.status(), expected);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    Ok(serde_json::from_slice(&to_bytes(response.into_body(), 65536).await?)?)
}
#[tokio::test]
async fn retained_backups_create_download_restore_survive_restart_and_delete_without_touching_sources() -> Result<()> {
    use headless_core::backup::{BackupDeletionReceipt, RetainedBackupList, RetainedBackupReceipt};
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        let empty: RetainedBackupList = retained_json(&app, &token, "GET", "", StatusCode::OK).await?;
        assert!(empty.archives.is_empty());
        assert!(!dir.0.join("backups").exists());
        let item = manager
            .import_profile_yaml("mode: rule\nrules: ['MATCH,DIRECT']\n".into(), "original".into())
            .await?;
        let uid = item.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        let config = manager.runtime_config().await?;
        let catalog = json!(manager.profiles());
        let settings = manager.settings().await?;
        let receipt: RetainedBackupReceipt = retained_json(&app, &token, "POST", "", StatusCode::CREATED).await?;
        assert!(receipt.committed && !receipt.durability_pending);
        assert_eq!(fs::metadata(dir.0.join("backups"))?.permissions().mode() & 0o777, 0o700);
        let path = fs::read_dir(dir.0.join("backups"))?.next().unwrap()?.path();
        assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        let listing: RetainedBackupList = retained_json(&app, &token, "GET", "", StatusCode::OK).await?;
        assert_eq!(listing.archives, vec![receipt.backup.clone()]);
        assert_eq!(listing.total_bytes, receipt.backup.content_length);
        assert_eq!(listing.max_archives, 32);
        assert_eq!(listing.max_total_bytes, 256 * 1024 * 1024);
        let response = app
            .clone()
            .oneshot(retained_request(&token, "GET", &format!("/{}", receipt.backup.id)))
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/zip");
        assert_eq!(response.headers()["x-backup-sha256"], receipt.backup.sha256);
        assert_eq!(
            app.clone().oneshot(retained_request(&token, "GET", "")).await?.status(),
            StatusCode::CONFLICT
        );
        let bytes = to_bytes(response.into_body(), MAX_ARCHIVE_BYTES).await?.to_vec();
        assert_eq!(hash(&bytes), receipt.backup.sha256);
        inspection_report(&app, &token, bytes.clone()).await?;
        assert_eq!(manager.runtime_config().await?, config);
        assert_eq!(json!(manager.profiles()), catalog);
        assert_eq!(manager.settings().await?, settings);
        let applied = apply_restore_report(&app, &token, bytes, "regenerated").await?;
        assert!(applied.committed && !applied.core_running);
        assert_eq!(
            manager.profile_raw(uid).await?.yaml,
            "mode: rule\nrules: ['MATCH,DIRECT']\n"
        );
        Ok::<_, anyhow::Error>(receipt.backup)
    }
    .await;
    let cleanup = manager.shutdown().await;
    let backup = result?;
    cleanup?;
    // Simulate a process dying before partial-file rename; committed archives survive.
    let part = dir.0.join("backups").join(format!(".{}.part", "a".repeat(24)));
    fs::write(&part, b"incomplete")?;
    fs::set_permissions(&part, fs::Permissions::from_mode(0o600))?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        assert!(!part.exists());
        let listing: RetainedBackupList = retained_json(&app, &token, "GET", "", StatusCode::OK).await?;
        assert_eq!(listing.archives, vec![backup.clone()]);
        let deleted: BackupDeletionReceipt =
            retained_json(&app, &token, "DELETE", &format!("/{}", backup.id), StatusCode::OK).await?;
        assert!(deleted.deleted && !deleted.durability_pending);
        let again: BackupDeletionReceipt =
            retained_json(&app, &token, "DELETE", &format!("/{}", backup.id), StatusCode::OK).await?;
        assert!(!again.deleted);
        assert_eq!(
            app.clone()
                .oneshot(retained_request(&token, "GET", &format!("/{}", backup.id)))
                .await?
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(fs::read_dir(dir.0.join("backups"))?.count(), 0);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(manager.shutdown().await)
}
#[tokio::test]
async fn retained_backup_auth_ids_bodies_capacity_corruption_and_unsafe_paths_are_bounded() -> Result<()> {
    use headless_core::backup::RetainedBackupReceipt;
    let dir = Directory::new()?;
    let manager = dir.manager(false)?;
    let (app, token) = dir.app(&manager)?;
    let result = async {
        for (method, suffix) in [("GET", ""), ("POST", ""), ("GET", "/bad"), ("DELETE", "/bad")] {
            let mut request = retained_request(&token, method, suffix);
            request.headers_mut().remove(header::AUTHORIZATION);
            assert_eq!(app.clone().oneshot(request).await?.status(), StatusCode::UNAUTHORIZED);
        }
        for suffix in ["/bad", "/..%2fsecret", "/AAAAAAAAAAAAAAAAAAAAAAAA", "?path=private"] {
            assert_eq!(
                app.clone()
                    .oneshot(retained_request(&token, "GET", suffix))
                    .await?
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        let mut request = retained_request(&token, "POST", "");
        *request.body_mut() = Body::from("unexpected");
        assert_eq!(app.clone().oneshot(request).await?.status(), StatusCode::BAD_REQUEST);
        let before = manager.status().config_revision;
        let receipt: RetainedBackupReceipt = retained_json(&app, &token, "POST", "", StatusCode::CREATED).await?;
        let path = fs::read_dir(dir.0.join("backups"))?.next().unwrap()?.path();
        let bytes = fs::read(&path)?;
        fs::write(&path, b"PRIVATE_CORRUPTED_ARCHIVE")?;
        let response = app
            .clone()
            .oneshot(retained_request(&token, "GET", &format!("/{}", receipt.backup.id)))
            .await?;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let message = to_bytes(response.into_body(), 4096).await?;
        assert!(!String::from_utf8_lossy(&message).contains("PRIVATE_CORRUPTED_ARCHIVE"));
        fs::remove_file(&path)?;
        symlink(dir.0.join("bootstrap.yaml"), &path)?;
        assert_eq!(
            app.clone()
                .oneshot(retained_request(&token, "DELETE", &format!("/{}", receipt.backup.id)))
                .await?
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(dir.0.join("bootstrap.yaml").exists());
        fs::remove_file(&path)?;
        fs::write(&path, bytes)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        for _ in 1..32 {
            let _: RetainedBackupReceipt = retained_json(&app, &token, "POST", "", StatusCode::CREATED).await?;
        }
        assert_eq!(
            app.clone()
                .oneshot(retained_request(&token, "POST", ""))
                .await?
                .status(),
            StatusCode::INSUFFICIENT_STORAGE
        );
        assert_eq!(fs::read_dir(dir.0.join("backups"))?.count(), 32);
        assert_eq!(manager.status().config_revision, before);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result.and(manager.shutdown().await)
}
