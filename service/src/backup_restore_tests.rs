use super::*;
use headless_core::backup::{BackupEntry, BackupManifest};
use std::{
    io::Cursor,
    os::unix::fs::{PermissionsExt as _, symlink},
};
struct TestDirectory(std::path::PathBuf);
impl TestDirectory {
    fn new() -> Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!("ms-restore-tests-{}", hash(&random)));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn archive() -> Vec<u8> {
    let contents = [
        (
            "profiles.yaml",
            b"current: main\nitems: [{uid: main, type: local, file: source.yaml}]\n".as_slice(),
        ),
        ("settings.yaml", b"schema_version: 1\n".as_slice()),
        ("runtime.yaml", b"mode: direct\nrules: ['MATCH,DIRECT']\n".as_slice()),
        (
            "profiles/source.yaml",
            b"# retained\nproxies: []\nrules: ['MATCH,DIRECT']\n".as_slice(),
        ),
    ];
    let manifest = BackupManifest {
        schema_version: 1,
        source: "mihomo-server".into(),
        service_version: "0.1.0".into(),
        created_at: 1,
        active_profile: Some("main".into()),
        runtime_revision: None,
        entries: contents
            .iter()
            .map(|(path, bytes)| BackupEntry {
                path: (*path).into(),
                bytes: bytes.len() as u64,
                sha256: hash(bytes),
            })
            .collect(),
    };
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in contents {
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(0o600),
        )
        .unwrap();
        zip.write_all(data).unwrap();
    }
    zip.start_file(
        "manifest.json",
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o600),
    )
    .unwrap();
    zip.write_all(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    zip.finish().unwrap().into_inner()
}
#[test]
fn verified_private_candidate_preserves_sources_generates_plan_copies_geo_and_cleans_up() -> Result<()> {
    let live = TestDirectory::new()?;
    fs::write(live.0.join("geoip.metadb"), b"resource fixture")?;
    let candidate = prepare(&archive(), &live.0, watch::channel(false).1)?;
    let root = candidate.directory.0.clone();
    assert_eq!(fs::metadata(&root)?.mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(root.join("profiles/source.yaml"))?.mode() & 0o777, 0o600);
    assert_eq!(
        fs::read(root.join("profiles/source.yaml"))?,
        b"# retained\nproxies: []\nrules: ['MATCH,DIRECT']\n"
    );
    assert_eq!(
        fs::read(root.join("validation-data/geoip.metadb"))?,
        b"resource fixture"
    );
    assert_eq!(candidate.generation.as_ref().unwrap().profile_uid, "main");
    assert_eq!(fs::read_dir(&live.0)?.count(), 2);
    assert!(root.starts_with(live.0.join("restore-candidates")));
    assert!(!root.join("management-token").exists() && !root.join("config/state.yaml").exists());
    drop(candidate);
    assert!(!root.exists());
    assert!(prepare(&archive(), &live.0, watch::channel(true).1).is_err());
    Ok(())
}
#[test]
fn geo_links_nonregular_unsafe_shared_and_oversized_resources_fail_before_validation() -> Result<()> {
    for case in ["link", "hardlink", "directory", "fifo", "mode", "size"] {
        let live = TestDirectory::new()?;
        let dest = TestDirectory::new()?;
        let path = live.0.join("geoip.metadb");
        match case {
            "link" => symlink("/missing", &path)?,
            "hardlink" => {
                fs::write(&path, b"resource")?;
                fs::hard_link(&path, live.0.join("shared"))?;
            }
            "directory" => fs::create_dir(&path)?,
            "fifo" => {
                let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            "mode" => {
                fs::write(&path, b"resource")?;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o666))?;
            }
            "size" => {
                File::create(&path)?.set_len(MAX_GEO_BYTES + 1)?;
            }
            _ => unreachable!(),
        }
        assert!(
            copy_geo(&live.0, &dest.0, &Budget::new(watch::channel(false).1)).is_err(),
            "{case}"
        );
        assert_eq!(fs::read_dir(&dest.0)?.count(), 0);
    }
    Ok(())
}
#[test]
fn provider_paths_and_candidate_identity_checks_do_not_follow_escape_links_or_large_replacements() -> Result<()> {
    for path in [
        "../live.yaml",
        "/tmp/live.yaml",
        "a/../../live.yaml",
        "C:live",
        "a\\live",
        "a\n",
        "",
    ] {
        let config = serde_yaml_ng::from_value(serde_yaml_ng::to_value(
            serde_json::json!({"rule-providers":{"test":{"path":path}}}),
        )?)?;
        assert!(resource_paths(&config).is_err(), "{path}");
    }
    let config = serde_yaml_ng::from_str("proxy-providers: {test: {path: ./providers/source.yaml}}")?;
    resource_paths(&config)?;
    let dir = TestDirectory::new()?;
    let path = dir.0.join("candidate.yaml");
    let budget = Budget::new(watch::channel(false).1);
    write(&path, b"mode: direct\n", &budget)?;
    check_config(&path, b"mode: direct\n", &budget)?;
    assert!(check_config(&path, b"changed\n", &budget).is_err());
    fs::remove_file(&path)?;
    symlink("/missing", &path)?;
    assert!(check_config(&path, b"mode: direct\n", &budget).is_err());
    fs::remove_file(&path)?;
    File::create(&path)?.set_len(MAX_GEO_BYTES)?;
    assert!(check_config(&path, b"mode: direct\n", &budget).is_err());
    Ok(())
}
