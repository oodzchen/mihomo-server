use super::export::{Snapshot, build, hash};
use anyhow::Result;
use headless_core::{
    backup::{BackupManifest, MAX_FILE_BYTES},
    config::settings::ServiceSettings,
};
use std::{
    fs,
    io::{Cursor, Read as _},
    os::unix::fs::{PermissionsExt as _, symlink},
    path::PathBuf,
};
use tokio::sync::watch;
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let p = std::env::temp_dir().join(format!("ms-backup-{}", &hash(&random)[..24]));
        fs::create_dir(&p)?;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700))?;
        fs::create_dir(p.join("profiles"))?;
        fs::write(p.join("runtime.yaml"), "mode: direct\nrules: ['MATCH,DIRECT']\n")?;
        fs::write(p.join("profiles/source.yaml"), "proxies: []\n")?;
        fs::write(p.join("profiles/unreferenced.yaml"), "excluded orphan")?;
        fs::write(p.join("management-token"), "excluded-management-credential")?;
        fs::create_dir(p.join("run"))?;
        fs::write(p.join("run/controller.yaml"), "excluded-controller-credential")?;
        Ok(Self(p))
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {data_dir:self.0.clone(), runtime_path:self.0.join("runtime.yaml"), profiles:serde_yaml_ng::from_str("current: main\nitems: [{uid: main, type: local, file: source.yaml, selected: [{name: Main, now: DIRECT}]}]\n").unwrap(), settings: ServiceSettings::default(), runtime_revision:None, active_profile:Some("main".into())}
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn zip_export_preserves_referenced_content_selections_and_digests_without_runtime_credentials() -> Result<()> {
    let dir = Directory::new()?;
    let before = fs::read(dir.0.join("profiles/source.yaml"))?;
    let (_, rx) = watch::channel(false);
    let (metadata, bytes) = build(dir.snapshot(), rx)?;
    assert_eq!(metadata.content_length, bytes.len() as u64);
    assert_eq!(metadata.sha256, hash(&bytes));
    assert!(metadata.filename.starts_with("mihomo-server-backup-") && metadata.filename.ends_with(".zip"));
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    assert_eq!(zip.len(), 5);
    let manifest: BackupManifest = serde_json::from_reader(zip.by_name("manifest.json")?)?;
    manifest.validate()?;
    assert_eq!(manifest.active_profile.as_deref(), Some("main"));
    for entry in manifest.entries {
        let mut file = zip.by_name(&entry.path)?;
        assert_eq!(file.compression(), zip::CompressionMethod::Stored);
        assert_eq!(file.unix_mode().unwrap() & 0o777, 0o600);
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        assert_eq!(data.len() as u64, entry.bytes);
        assert_eq!(hash(&data), entry.sha256);
        assert!(!data.windows(30).any(|part| part == b"excluded-management-credential"));
    }
    assert!(zip.by_name("management-token").is_err());
    assert!(zip.by_name("run/controller.yaml").is_err());
    assert!(zip.by_name("profiles/unreferenced.yaml").is_err());
    assert_eq!(fs::read(dir.0.join("profiles/source.yaml"))?, before);
    Ok(())
}
#[test]
fn unsafe_and_oversized_sources_are_rejected_without_following_links_or_blocking_on_fifo() -> Result<()> {
    for case in [
        "symlink",
        "hardlink",
        "directory",
        "fifo",
        "oversize",
        "permissions",
        "runtime-link",
    ] {
        let dir = Directory::new()?;
        let source = dir.0.join("profiles/source.yaml");
        match case {
            "symlink" => {
                fs::remove_file(&source)?;
                symlink(dir.0.join("management-token"), &source)?;
            }
            "hardlink" => fs::hard_link(&source, dir.0.join("outside-link"))?,
            "directory" => {
                fs::remove_file(&source)?;
                fs::create_dir(&source)?;
            }
            "fifo" => {
                fs::remove_file(&source)?;
                let path = std::ffi::CString::new(source.to_str().unwrap())?;
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            "oversize" => fs::OpenOptions::new()
                .write(true)
                .open(&source)?
                .set_len(MAX_FILE_BYTES + 1)?,
            "permissions" => fs::set_permissions(&source, fs::Permissions::from_mode(0o666))?,
            "runtime-link" => {
                fs::remove_file(dir.0.join("runtime.yaml"))?;
                symlink(&source, dir.0.join("runtime.yaml"))?;
            }
            _ => unreachable!(),
        }
        let (_, rx) = watch::channel(false);
        assert!(build(dir.snapshot(), rx).is_err(), "{case}");
        assert_eq!(
            fs::read(dir.0.join("management-token"))?,
            b"excluded-management-credential"
        );
    }
    Ok(())
}
#[test]
fn cancellation_and_invalid_runtime_fail_without_publishing_an_archive() -> Result<()> {
    let dir = Directory::new()?;
    let (_, rx) = watch::channel(true);
    assert!(build(dir.snapshot(), rx).is_err());
    fs::write(dir.0.join("runtime.yaml"), "external-controller: 127.0.0.1:9090\n")?;
    let (_, rx) = watch::channel(false);
    assert!(build(dir.snapshot(), rx).is_err());
    assert!(!dir.0.join("backup").exists());
    Ok(())
}

#[test]
fn export_rejects_too_many_references_and_aggregate_content_before_archive_publication() -> Result<()> {
    let dir = Directory::new()?;
    let mut snapshot = dir.snapshot();
    let items: Vec<_> = (0..1021)
        .map(|n| serde_json::json!({"uid":n.to_string(),"type":"local","file":format!("missing-{n}.yaml")}))
        .collect();
    snapshot.profiles = serde_json::from_value(serde_json::json!({"items":items}))?;
    let (_, rx) = watch::channel(false);
    let error = build(snapshot, rx).unwrap_err();
    assert!(format!("{error:#}").contains("too many entries"));
    let mut items = Vec::new();
    for n in 0..8 {
        let name = format!("large-{n}.yaml");
        let file = fs::File::create(dir.0.join("profiles").join(&name))?;
        file.set_len(MAX_FILE_BYTES)?;
        items.push(serde_json::json!({"uid":n.to_string(),"type":"local","file":name}));
    }
    let mut snapshot = dir.snapshot();
    snapshot.profiles = serde_json::from_value(serde_json::json!({"items":items}))?;
    let (_, rx) = watch::channel(false);
    let error = build(snapshot, rx).unwrap_err();
    assert!(format!("{error:#}").contains("content exceeds 64 MiB"));
    assert!(!dir.0.join("backups").exists());
    Ok(())
}
