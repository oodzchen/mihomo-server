use anyhow::Result;
use headless_core::config::{
    PrfOption,
    profile_store::{ProfilePatch, ProfileStore},
    remote::{from_response, subscription_url},
    runtime::{MAX_CONFIG_BYTES, RuntimeStore, parse},
};
use std::{fs, path::PathBuf};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-raw-store-{}-{stamp:x}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn raw_replacement_preserves_exact_content_metadata_and_old_revision() -> Result<()> {
    for remote in [false, true] {
        let dir = Directory::new()?;
        let mut store = ProfileStore::open(&dir.0)?;
        let initial = "# original\nproxies: []\nmode: rule\n";
        let item = if remote {
            store.import_remote(from_response(
                &subscription_url("https://example.test/sub")?,
                Some("raw"),
                &[(
                    "subscription-userinfo".into(),
                    "upload=1; download=2; total=3; expire=4".into(),
                )],
                initial,
                PrfOption::default(),
            )?)?
        } else {
            store.import_local("raw", initial)?
        };
        let uid = item.uid.unwrap().to_string();
        store.edit_profile(
            &uid,
            ProfilePatch {
                desc: Some("description".into()),
                ..Default::default()
            },
        )?;
        store.record_selection(&uid, "main", "DIRECT")?;
        let previous = store.get_item(&uid)?.clone();
        let content = store.read_raw(&uid)?;
        let yaml = "\u{feff}# exact\r\nproxies: []\r\nmode: direct\r\n";
        store.begin_raw_edit(&uid, &content.revision, yaml, None)?;
        assert_eq!(store.read_raw(&uid)?, content);
        assert!(fs::read_to_string(dir.0.join("profile-refresh.yaml"))?.contains("kind: raw_edit"));
        store.publish_refresh()?;
        store.recover_refresh(None)?;
        let next = store.read_raw(&uid)?;
        assert_eq!(next.yaml, yaml);
        assert_ne!(next.revision, content.revision);
        assert_eq!(
            fs::read_to_string(dir.0.join("profiles").join(&content.revision))?,
            initial
        );
        let mut metadata = store.get_item(&uid)?.clone();
        metadata.file = previous.file.clone();
        assert_eq!(serde_json::to_value(metadata)?, serde_json::to_value(previous)?);
        assert!(
            store
                .begin_raw_edit(&uid, &content.revision, "mode: global", None)
                .is_err()
        );
        assert_eq!(store.read_raw(&uid)?, next);
    }
    Ok(())
}

#[test]
fn inactive_raw_recovery_follows_catalog_publication() -> Result<()> {
    for published in [false, true] {
        let dir = Directory::new()?;
        let mut store = ProfileStore::open(&dir.0)?;
        let uid = store.import_local("raw", "mode: rule")?.uid.unwrap().to_string();
        let old = store.read_raw(&uid)?;
        store.begin_raw_edit(&uid, &old.revision, "mode: direct", None)?;
        if published {
            store.publish_refresh()?;
        }
        drop(store);
        let mut store = ProfileStore::open(&dir.0)?;
        store.recover_refresh(None)?;
        assert_eq!(
            store.read_raw(&uid)?.yaml,
            if published { "mode: direct" } else { "mode: rule" }
        );
        assert!(!dir.0.join("profile-refresh.yaml").exists());
    }
    Ok(())
}

#[test]
fn active_raw_recovery_follows_runtime_commit_even_before_catalog_publication() -> Result<()> {
    for committed in [false, true] {
        for published in [false, true] {
            let dir = Directory::new()?;
            let mut store = ProfileStore::open(&dir.0)?;
            let uid = store.import_local("raw", "mode: rule")?.uid.unwrap().to_string();
            let old = store.read_raw(&uid)?;
            let mut runtime = RuntimeStore::open(&dir.0)?;
            let prior = runtime.stage(parse("mode: rule")?)?;
            runtime.begin_profile(prior, Some(uid.clone()))?;
            runtime.commit()?;
            let revision = runtime.stage(parse("mode: direct")?)?;
            runtime.begin_profile(revision.clone(), Some(uid.clone()))?;
            store.begin_raw_edit(&uid, &old.revision, "mode: direct", Some(revision))?;
            if published {
                store.publish_refresh()?;
            }
            if committed {
                runtime.commit()?;
            }
            drop(store);
            drop(runtime);
            let runtime = RuntimeStore::open(&dir.0)?;
            let mut store = ProfileStore::open(&dir.0)?;
            store.recover_refresh(runtime.state().current.as_ref())?;
            assert_eq!(
                store.read_raw(&uid)?.yaml,
                if committed { "mode: direct" } else { "mode: rule" }
            );
        }
    }
    Ok(())
}

#[test]
fn raw_journal_rejects_metadata_changes_unknown_kind_and_unsafe_paths() -> Result<()> {
    for change in ["name", "kind", "file"] {
        let dir = Directory::new()?;
        let mut store = ProfileStore::open(&dir.0)?;
        let uid = store.import_local("raw", "mode: rule")?.uid.unwrap().to_string();
        let old = store.read_raw(&uid)?;
        store.begin_raw_edit(&uid, &old.revision, "mode: direct", None)?;
        let journal = dir.0.join("profile-refresh.yaml");
        let mut value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&fs::read_to_string(&journal)?)?;
        match change {
            "name" => value["candidate"]["name"] = "forged".into(),
            "kind" => value["kind"] = "unknown".into(),
            _ => value["candidate"]["file"] = "../escape.yaml".into(),
        }
        fs::write(&journal, serde_yaml_ng::to_string(&value)?)?;
        assert!(store.recover_refresh(None).is_err());
        assert_eq!(store.read_raw(&uid)?, old);
    }
    Ok(())
}

#[test]
fn raw_editor_rejects_invalid_inputs_auxiliary_items_and_symlinks() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = ProfileStore::open(&dir.0)?;
    store.ensure_global_defaults()?;
    assert!(store.read_raw("Merge").is_err());
    assert!(store.read_raw("../escape").is_err());
    let uid = store.import_local("raw", "mode: rule")?.uid.unwrap().to_string();
    let old = store.read_raw(&uid)?;
    for yaml in ["- sequence", "proxies: ["] {
        assert!(store.begin_raw_edit(&uid, &old.revision, yaml, None).is_err());
    }
    assert!(
        store
            .begin_raw_edit(&uid, &old.revision, &"x".repeat(MAX_CONFIG_BYTES + 1), None)
            .is_err()
    );
    assert!(store.begin_raw_edit("Merge", "Merge.yaml", "{}", None).is_err());
    #[cfg(unix)]
    {
        let file = dir.0.join("profiles").join(&old.revision);
        fs::remove_file(&file)?;
        let outside = dir.0.join("outside.yaml");
        fs::write(&outside, "mode: direct")?;
        std::os::unix::fs::symlink(outside, file)?;
        assert!(store.read_raw(&uid).is_err());
        assert!(store.begin_raw_edit(&uid, &old.revision, "mode: direct", None).is_err());
    }
    Ok(())
}
