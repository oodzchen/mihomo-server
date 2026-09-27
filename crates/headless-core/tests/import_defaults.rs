use anyhow::Result;
use headless_core::config::{
    PrfOption,
    profile_store::{DEFAULT_GLOBAL_SCRIPT, ProfileStore},
    remote::{from_response, subscription_url},
};
use std::{fs, path::PathBuf};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-import-{}-{stamp:x}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn links(store: &ProfileStore, uid: &str) -> Result<Vec<(String, String)>> {
    let op = store.get_item(uid)?.option.as_ref().unwrap();
    Ok([&op.merge, &op.script, &op.rules, &op.proxies, &op.groups]
        .into_iter()
        .map(|uid| {
            let uid = uid.as_deref().unwrap();
            (
                uid.to_owned(),
                store.get_item(uid).unwrap().file.as_deref().unwrap().to_owned(),
            )
        })
        .collect())
}
#[test]
fn local_import_creates_distinct_upstream_defaults_and_preserves_exact_raw() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = ProfileStore::open(&dir.0)?;
    store.ensure_global_defaults()?;
    let yaml = "# actual raw\r\nmode: direct\r\nproxies: []\r\n";
    let first = store.import_local_with_defaults("first", yaml)?;
    let second = store.import_local_with_defaults("second", yaml)?;
    assert_eq!(store.snapshot().items.unwrap().len(), 14);
    let uid = first.uid.as_deref().unwrap();
    assert_eq!(store.read_raw(uid)?.yaml, yaml);
    let owned = links(&store, uid)?;
    let other = links(&store, second.uid.as_deref().unwrap())?;
    assert!(owned.iter().all(|item| !other.iter().any(|row| row.0 == item.0)));
    for ((uid, file), kind) in owned.iter().zip(["merge", "script", "rules", "proxies", "groups"]) {
        assert_eq!(store.get_item(uid)?.itype.as_deref(), Some(kind));
        let source = fs::read_to_string(dir.0.join("profiles").join(file))?;
        if kind == "merge" {
            assert!(serde_yaml_ng::from_str::<serde_yaml_ng::Mapping>(&source)?.is_empty());
        } else if kind == "script" {
            assert_eq!(source, DEFAULT_GLOBAL_SCRIPT);
            assert!(file.ends_with(".js"));
        } else {
            let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&source)?;
            for key in ["prepend", "append", "delete"] {
                assert!(value[key].as_sequence().unwrap().is_empty());
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(dir.0.join("profiles").join(file))?.permissions().mode() & 0o777,
                0o600
            );
        }
    }
    assert!(!dir.0.join("profile-import.yaml").exists());
    assert_eq!(store.read_mapping(uid)?["mode"].as_str(), Some("direct"));
    Ok(())
}
#[test]
fn remote_import_preserves_metadata_and_reuses_shared_or_reserved_links() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = ProfileStore::open(&dir.0)?;
    store.ensure_global_defaults()?;
    let owner = store.import_local_with_defaults("owner", "mode: direct")?;
    let shared = owner.option.unwrap().rules.unwrap();
    let op = PrfOption {
        merge: Some("Merge".into()),
        script: Some("Script".into()),
        rules: Some(shared.clone()),
        user_agent: Some("agent".into()),
        update_interval: Some(123),
        allow_auto_update: Some(false),
        timeout_seconds: Some(5),
        ..Default::default()
    };
    let remote = from_response(
        &subscription_url("https://example.test/sub")?,
        Some("remote"),
        &[(
            "subscription-userinfo".into(),
            "upload=1; download=2; total=3; expire=4".into(),
        )],
        "# source\nproxies: []\n",
        op.clone(),
    )?;
    let extra = serde_json::to_value(remote.extra)?;
    let item = store.import_remote_with_defaults(remote)?;
    let option = item.option.as_ref().unwrap();
    assert_eq!(option.merge, op.merge);
    assert_eq!(option.script, op.script);
    assert_eq!(option.rules.as_deref(), Some(shared.as_str()));
    assert_eq!(option.update_interval, Some(123));
    assert_eq!(option.user_agent, op.user_agent);
    assert_eq!(option.timeout_seconds, Some(5));
    assert_eq!(option.allow_auto_update, Some(false));
    assert_eq!(serde_json::to_value(item.extra)?, extra);
    assert_eq!(store.snapshot().items.unwrap().len(), 11); // two globals + owner six + remote three
    let uid = item.uid.as_deref().unwrap();
    store.delete_profile(uid, None)?;
    assert!(store.get_item(&shared).is_ok());
    assert!(store.get_item("Merge").is_ok());
    assert!(store.get_item("Script").is_ok());
    Ok(())
}
#[test]
fn import_interruption_follows_catalog_commit_and_partial_file_cleanup_is_safe() -> Result<()> {
    for committed in [false, true] {
        let dir = Directory::new()?;
        let mut store = ProfileStore::open(&dir.0)?;
        let plan = store.prepare_local_import("import", "mode: direct")?;
        let uid = plan.profile().uid.as_deref().unwrap().to_owned();
        store.begin_import(plan)?;
        if committed {
            store.publish_import()?;
        } else {
            let file = fs::read_dir(dir.0.join("profiles"))?.next().unwrap()?.path();
            fs::remove_file(file)?;
        }
        drop(store);
        let mut store = ProfileStore::open(&dir.0)?;
        store.recover_import()?;
        assert_eq!(store.get_item(&uid).is_ok(), committed);
        assert_eq!(
            fs::read_dir(dir.0.join("profiles"))?.count(),
            if committed { 6 } else { 0 }
        );
        assert!(!dir.0.join("profile-import.yaml").exists());
        if committed {
            assert_eq!(links(&store, &uid)?.len(), 5);
        }
    }
    Ok(())
}
#[test]
fn failed_catalog_publication_cleans_all_allocated_files_and_keeps_legacy_rows() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = ProfileStore::open(&dir.0)?;
    let legacy = store.import_local("legacy", "mode: direct")?;
    let before = serde_yaml_ng::to_value(store.snapshot())?;
    fs::rename(dir.0.join("profiles.yaml"), dir.0.join("saved.yaml"))?;
    fs::create_dir(dir.0.join("profiles.yaml"))?;
    assert!(store.import_local_with_defaults("failed", "mode: rule").is_err());
    assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, before);
    assert_eq!(fs::read_dir(dir.0.join("profiles"))?.count(), 1);
    assert!(!dir.0.join("profile-import.yaml").exists());
    assert_eq!(store.read_raw(legacy.uid.as_deref().unwrap())?.yaml, "mode: direct");
    Ok(())
}
#[test]
fn allocated_filename_collision_fails_before_journal_and_preserves_existing_file() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = ProfileStore::open(&dir.0)?;
    let plan = store.prepare_local_import("import", "mode: direct")?;
    let file = plan.profile().file.as_deref().unwrap();
    let path = dir.0.join("profiles").join(file);
    fs::write(&path, "owned elsewhere")?;
    assert!(store.begin_import(plan).is_err());
    assert_eq!(fs::read_to_string(path)?, "owned elsewhere");
    assert!(!dir.0.join("profile-import.yaml").exists());
    Ok(())
}
#[test]
fn changed_reused_links_and_missing_or_wrong_type_links_reject_before_file_creation() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = ProfileStore::open(&dir.0)?;
    store.ensure_global_defaults()?;
    let make = |merge: &str| {
        from_response(
            &subscription_url("https://example.test/sub")?,
            None,
            &[],
            "proxies: []",
            PrfOption {
                merge: Some(merge.into()),
                ..Default::default()
            },
        )
    };
    for invalid in ["missing", "Script"] {
        assert!(store.prepare_remote_import(make(invalid)?).is_err());
    }
    let plan = store.prepare_remote_import(make("Merge")?)?;
    fs::write(
        dir.0
            .join("profiles")
            .join(store.get_item("Merge")?.file.as_deref().unwrap()),
        "mode: direct",
    )?;
    assert!(store.begin_import(plan).is_err());
    assert!(!dir.0.join("profile-import.yaml").exists());
    assert_eq!(fs::read_dir(dir.0.join("profiles"))?.count(), 2);
    Ok(())
}
#[test]
fn pending_import_blocks_mutations_and_corrupted_content_cannot_be_published() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = ProfileStore::open(&dir.0)?;
    let old = store.import_local("existing", "mode: direct")?;
    let uid = old.uid.as_deref().unwrap();
    let plan = store.prepare_local_import("import", "mode: direct")?;
    let file = plan.profile().file.as_deref().unwrap().to_owned();
    store.begin_import(plan)?;
    assert!(store.import_local("other", "mode: direct").is_err());
    assert!(store.ensure_global_defaults().is_err());
    assert!(store.begin_delete(uid, None).is_err());
    assert!(store.prepare_local_import("another", "mode: direct").is_err());
    fs::write(dir.0.join("profiles").join(file), "mode: rule")?;
    assert!(store.publish_import().is_err());
    store.recover_import()?;
    assert_eq!(store.snapshot().items.unwrap().len(), 1);
    assert_eq!(fs::read_dir(dir.0.join("profiles"))?.count(), 1);
    Ok(())
}
#[test]
fn forged_import_journals_and_partial_catalog_commit_fail_closed() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = ProfileStore::open(&dir.0)?;
    let plan = store.prepare_local_import("import", "mode: direct")?;
    store.begin_import(plan)?;
    let path = dir.0.join("profile-import.yaml");
    let original = fs::read_to_string(&path)?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&original)?;
    for variant in 0..5 {
        let mut bad = value.clone();
        match variant {
            0 => bad["items"][0]["file"] = "../outside.yaml".into(),
            1 => bad["items"][1]["uid"] = "Merge".into(),
            2 => bad["items"][1]["type"] = "local".into(),
            3 => bad["transaction"] = "../evil".into(),
            _ => bad["items"][0]["option"]["script"] = "missing".into(),
        };
        fs::write(&path, serde_yaml_ng::to_string(&bad)?)?;
        assert!(store.publish_import().is_err());
        assert!(store.recover_import().is_err());
        assert_eq!(fs::read_dir(dir.0.join("profiles"))?.count(), 6);
    }
    fs::write(&path, original)?;
    store.publish_import()?;
    let mut catalog = store.snapshot();
    catalog.items.as_mut().unwrap().remove(1);
    fs::write(dir.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    assert!(ProfileStore::open(&dir.0)?.recover_import().is_err());
    assert!(path.is_file());
    Ok(())
}
#[cfg(unix)]
#[test]
fn unsafe_abort_cleanup_keeps_journal_until_retry_without_following_symlinks() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = ProfileStore::open(&dir.0)?;
    let plan = store.prepare_local_import("import", "mode: direct")?;
    let file = plan.profile().file.as_deref().unwrap().to_owned();
    store.begin_import(plan)?;
    let outside = dir.0.join("outside.yaml");
    fs::write(&outside, "preserved")?;
    let allocated = dir.0.join("profiles").join(file);
    fs::remove_file(&allocated)?;
    std::os::unix::fs::symlink(&outside, &allocated)?;
    assert!(store.recover_import().is_err());
    assert_eq!(fs::read_to_string(&outside)?, "preserved");
    assert!(dir.0.join("profile-import.yaml").is_file());
    fs::remove_file(allocated)?;
    store.recover_import()?;
    assert_eq!(fs::read_to_string(outside)?, "preserved");
    assert_eq!(fs::read_dir(dir.0.join("profiles"))?.count(), 0);
    Ok(())
}
