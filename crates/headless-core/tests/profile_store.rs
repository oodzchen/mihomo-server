use std::{fs, path::PathBuf};

use anyhow::{Result, ensure};
use headless_core::config::{IProfiles, profile_store::ProfileStore};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let dir = Self(std::env::temp_dir().join(format!("ms-profiles-{}-{stamp:x}", std::process::id())));
        fs::create_dir_all(&dir.0)?;
        Ok(dir)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn source_controllers_are_ignored_without_rewriting_profiles_or_allowing_merge_overrides() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let yaml = "# original\nmode: direct\nexternal-controller: '127.0.0.1:9090'\nexternal-controller-tls: '0.0.0.0:9443'\nexternal-controller-unix: /tmp/source.sock\nexternal-controller-pipe: source-pipe\n";
    let imported = store.import_local("desktop export", yaml)?;
    let uid = imported.uid.as_deref().unwrap();
    let generated = store.read_mapping(uid)?;
    assert_eq!(generated["mode"].as_str(), Some("direct"));
    for field in [
        "external-controller",
        "external-controller-tls",
        "external-controller-unix",
        "external-controller-pipe",
    ] {
        assert!(!generated.contains_key(field));
        assert!(store.prepare_merge(uid, Some(format!("{field}: outside"))).is_err());
    }
    assert_eq!(store.read_raw(uid)?.yaml, yaml);
    let parsed = headless_core::config::runtime::parse_profile(yaml)?;
    assert_eq!(parsed["mode"], generated["mode"]);
    assert!(!parsed.contains_key("external-controller"));
    assert!(headless_core::config::runtime::parse(yaml).is_err());
    assert!(headless_core::config::runtime::parse_profile("- not-a-mapping").is_err());
    Ok(())
}

#[test]
fn local_import_preserves_content_schema_and_separate_activation() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let yaml = "# original subscription\nmode: rule\nrules: [MATCH,DIRECT]\n";
    let imported = store.import_local("本地订阅", yaml)?;
    let uid = imported.uid.as_deref().unwrap();
    assert_eq!(imported.itype.as_deref(), Some("local"));
    assert_eq!(
        fs::read_to_string(directory.0.join("profiles").join(imported.file.as_deref().unwrap()))?,
        yaml
    );
    ensure!(store.snapshot().current.is_none());
    ensure!(store.import_local("bad", "- sequence").is_err());
    ensure!(store.import_local("bad", "mode: [").is_err());
    assert_eq!(store.snapshot().items.as_ref().unwrap().len(), 1);
    store.set_current(Some(uid))?;
    drop(store);
    let store = ProfileStore::open(&directory.0)?;
    assert_eq!(store.snapshot().current.as_deref(), Some(uid));
    assert_eq!(store.read_mapping(uid)?["mode"].as_str(), Some("rule"));
    let schema: IProfiles = serde_yaml_ng::from_str(&fs::read_to_string(directory.0.join("profiles.yaml"))?)?;
    ensure!(schema.items.unwrap()[0].file_data.is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for path in [
            directory.0.join("profiles.yaml"),
            directory.0.join("profiles").join(imported.file.as_deref().unwrap()),
        ] {
            assert_eq!(path.metadata()?.permissions().mode() & 0o777, 0o600);
        }
    }
    Ok(())
}

#[test]
fn upstream_remote_metadata_and_node_selections_survive_current_updates() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let item = store.import_local("cached", "mode: direct")?;
    let uid = item.uid.as_deref().unwrap();
    let path = directory.0.join("profiles.yaml");
    let mut profiles = store.snapshot();
    let item = &mut profiles.items.as_mut().unwrap()[0];
    item.itype = Some("remote".into());
    item.url = Some("https://example.test/subscription".into());
    item.selected = Some(vec![headless_core::config::PrfSelected {
        name: Some("Main".into()),
        now: Some("node".into()),
    }]);
    item.extra = Some(headless_core::config::PrfExtra {
        upload: 1,
        download: 2,
        total: 3,
        expire: 4,
    });
    fs::write(&path, serde_yaml_ng::to_string(&profiles)?)?;
    let mut store = ProfileStore::open(&directory.0)?;
    store.set_current(Some(uid))?;
    assert_eq!(store.read_mapping(uid)?["mode"].as_str(), Some("direct"));
    profiles.current = Some(uid.into());
    assert_eq!(
        serde_yaml_ng::to_value(store.snapshot())?,
        serde_yaml_ng::to_value(profiles)?
    );
    Ok(())
}

#[test]
fn unsupported_enhancements_are_preserved_and_rejected_on_selection() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let imported = store.import_local("linked", "mode: rule")?;
    let mut profiles = store.snapshot();
    profiles.items.as_mut().unwrap()[0].option = Some(headless_core::config::PrfOption {
        script: Some("script-one".into()),
        ..Default::default()
    });
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&profiles)?)?;
    let store = ProfileStore::open(&directory.0)?;
    let error = store.read_mapping(imported.uid.as_deref().unwrap()).unwrap_err();
    ensure!(format!("{error:#}").contains("enhancement links"));
    assert_eq!(
        store.snapshot().items.unwrap()[0]
            .option
            .as_ref()
            .unwrap()
            .script
            .as_deref(),
        Some("script-one")
    );
    Ok(())
}

#[test]
fn failed_catalog_write_does_not_publish_import_or_leave_referenced_missing_file() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    fs::create_dir(directory.0.join("profiles.yaml"))?;
    ensure!(store.import_local("fail", "mode: rule").is_err());
    ensure!(store.snapshot().items.is_none());
    assert_eq!(fs::read_dir(directory.0.join("profiles"))?.count(), 0);
    Ok(())
}

#[test]
fn unsafe_filenames_and_duplicate_uids_are_rejected() -> Result<()> {
    let directory = Directory::new()?;
    for yaml in [
        "items: [{uid: one, type: local, file: '../outside.yaml'}]",
        "items: [{uid: one, type: local, file: 'dir\\outside.yaml'}]",
        "items: [{uid: one, type: local}, {uid: one, type: remote}]",
    ] {
        fs::write(directory.0.join("profiles.yaml"), yaml)?;
        ensure!(ProfileStore::open(&directory.0).is_err());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn selected_profile_cannot_follow_a_symlink_outside_its_directory() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let item = store.import_local("link", "mode: rule")?;
    let path = directory.0.join("profiles").join(item.file.as_deref().unwrap());
    fs::remove_file(&path)?;
    let outside = directory.0.join("outside.yaml");
    fs::write(&outside, "mode: direct")?;
    std::os::unix::fs::symlink(outside, path)?;
    ensure!(store.read_mapping(item.uid.as_deref().unwrap()).is_err());
    Ok(())
}

#[test]
fn downloaded_remote_import_retains_metadata_and_separate_activation() -> Result<()> {
    use headless_core::config::{
        PrfOption,
        remote::{from_response, subscription_url},
    };
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let local = store.import_local("working", "mode: direct")?;
    store.set_current(local.uid.as_deref())?;
    let headers = vec![
        ("subscription-userinfo".into(), "download=42; total=100".into()),
        ("profile-update-interval".into(), "12".into()),
    ];
    let remote = from_response(
        &subscription_url("https://example.test/sub?token=private")?,
        Some("remote"),
        &headers,
        "\u{feff}# saved raw\nproxies: []\n",
        PrfOption::default(),
    )?;
    let item = store.import_remote(remote)?;
    assert!(item.uid.as_deref().unwrap().starts_with('R'));
    assert_eq!(item.itype.as_deref(), Some("remote"));
    assert_eq!(item.extra.unwrap().download, 42);
    assert_eq!(item.option.as_ref().unwrap().update_interval, Some(720));
    assert_eq!(store.snapshot().current, local.uid);
    assert_eq!(
        fs::read_to_string(directory.0.join("profiles").join(item.file.as_deref().unwrap()))?,
        "# saved raw\nproxies: []\n"
    );
    let reopened = ProfileStore::open(&directory.0)?;
    assert_eq!(
        reopened.get_item(item.uid.as_deref().unwrap())?.url.as_deref(),
        Some("https://example.test/sub?token=private")
    );
    assert!(
        reopened
            .read_mapping(item.uid.as_deref().unwrap())?
            .contains_key("proxies")
    );
    assert!(item.file_data.is_none());
    Ok(())
}

#[test]
fn failed_remote_catalog_save_preserves_prior_items_and_removes_new_content() -> Result<()> {
    use headless_core::config::{
        PrfOption,
        remote::{from_response, subscription_url},
    };
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let local = store.import_local("retained", "mode: direct")?;
    store.set_current(local.uid.as_deref())?;
    let prior = serde_yaml_ng::to_value(store.snapshot())?;
    fs::rename(directory.0.join("profiles.yaml"), directory.0.join("saved.yaml"))?;
    fs::create_dir(directory.0.join("profiles.yaml"))?;
    let remote = from_response(
        &subscription_url("https://example.test/sub")?,
        None,
        &[],
        "proxies: []",
        PrfOption::default(),
    )?;
    assert!(store.import_remote(remote).is_err());
    assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, prior);
    assert_eq!(fs::read_dir(directory.0.join("profiles"))?.count(), 1);
    assert_eq!(store.snapshot().current, local.uid);
    Ok(())
}

fn refreshed(body: &str, used: u64) -> Result<headless_core::config::remote::RemoteProfile> {
    use headless_core::config::{
        PrfOption,
        remote::{from_response, subscription_url},
    };
    from_response(
        &subscription_url("https://example.test/sub")?,
        Some("provider title"),
        &[("subscription-userinfo".into(), format!("download={used}; total=100"))],
        body,
        PrfOption {
            allow_auto_update: Some(false),
            ..Default::default()
        },
    )
}

#[test]
fn inactive_refresh_preserves_identity_options_nodes_and_commits_at_catalog_rename() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let mut initial = refreshed("proxies: []\nmode: rule", 1)?;
    initial.name = "user title".into();
    initial.home = Some("https://example.test/old-home".into());
    initial.option.user_agent = Some("saved-agent".into());
    initial.option.with_proxy = Some(false);
    let item = store.import_remote(initial)?;
    let uid = item.uid.as_deref().unwrap();
    store.record_selection(uid, "Main", "REJECT")?;
    let before = store.get_item(uid)?.clone();
    store.begin_refresh(uid, refreshed("# fresh\nproxies: []\nmode: direct", 20)?, None)?;
    assert_eq!(store.get_item(uid)?.file, before.file);
    // Interrupted before catalog publication: old item/content survive.
    let mut reopened = ProfileStore::open(&directory.0)?;
    reopened.recover_refresh(None)?;
    assert_eq!(
        serde_yaml_ng::to_value(reopened.get_item(uid)?)?,
        serde_yaml_ng::to_value(&before)?
    );
    reopened.begin_refresh(uid, refreshed("# fresh\nproxies: []\nmode: direct", 20)?, None)?;
    reopened.publish_refresh()?;
    let candidate = reopened.get_item(uid)?.clone();
    let mut reopened = ProfileStore::open(&directory.0)?;
    reopened.recover_refresh(None)?;
    assert_eq!(reopened.get_item(uid)?.file, candidate.file);
    assert_eq!(candidate.uid, before.uid);
    assert_eq!(candidate.name, before.name);
    assert_eq!(candidate.name.as_deref(), Some("user title"));
    assert!(candidate.home.is_none());
    assert_eq!(
        candidate.option.as_ref().unwrap().user_agent.as_deref(),
        Some("saved-agent")
    );
    assert_eq!(candidate.option.as_ref().unwrap().with_proxy, Some(false));
    assert_eq!(candidate.url, before.url);
    assert_eq!(candidate.selected, before.selected);
    assert_eq!(candidate.option.as_ref().unwrap().allow_auto_update, Some(false));
    assert_eq!(candidate.extra.unwrap().download, 20);
    assert_eq!(reopened.snapshot().items.unwrap().len(), 1);
    assert_eq!(reopened.read_mapping(uid)?["mode"].as_str(), Some("direct"));
    assert!(!directory.0.join("profile-refresh.yaml").exists());
    Ok(())
}

#[test]
fn interrupted_active_refresh_tracks_committed_runtime_and_never_promotes_pending() -> Result<()> {
    use headless_core::config::runtime::{RuntimeStore, parse};
    for committed in [false, true] {
        let directory = Directory::new()?;
        let mut store = ProfileStore::open(&directory.0)?;
        let item = store.import_remote(refreshed("proxies: []\nmode: rule", 1)?)?;
        let uid = item.uid.as_deref().unwrap();
        store.record_selection(uid, "Main", "REJECT")?;
        let before = store.get_item(uid)?.clone();
        let mut runtime = RuntimeStore::open(&directory.0)?;
        let old = runtime.stage(parse("mode: rule")?)?;
        runtime.begin_profile(old.clone(), Some(uid.into()))?;
        runtime.commit()?;
        let new = runtime.stage(parse("mode: direct")?)?;
        runtime.begin_profile(new.clone(), Some(uid.into()))?;
        store.begin_refresh(uid, refreshed("proxies: []\nmode: direct", 20)?, Some(new.clone()))?;
        store.publish_refresh()?;
        if committed {
            runtime.commit()?;
        }
        drop(runtime);
        let runtime = RuntimeStore::open(&directory.0)?;
        assert!(runtime.state().pending.is_none());
        let mut reopened = ProfileStore::open(&directory.0)?;
        reopened.recover_refresh(runtime.state().current.as_ref())?;
        assert_eq!(reopened.get_item(uid)?.selected, before.selected);
        assert_eq!(
            reopened.get_item(uid)?.extra.unwrap().download,
            if committed { 20 } else { 1 }
        );
        assert_eq!(reopened.get_item(uid)?.file == before.file, !committed);
        assert_eq!(runtime.state().current, Some(if committed { new } else { old }));
        assert!(!directory.0.join("profile-refresh.yaml").exists());
    }
    Ok(())
}

#[test]
fn failed_refresh_catalog_write_is_recoverable_and_keeps_other_profiles() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let local = store.import_local("working", "mode: rule")?;
    store.set_current(local.uid.as_deref())?;
    let item = store.import_remote(refreshed("proxies: []", 1)?)?;
    let uid = item.uid.as_deref().unwrap();
    let prior = serde_yaml_ng::to_value(store.snapshot())?;
    store.begin_refresh(uid, refreshed("proxies: []\nmode: direct", 20)?, None)?;
    fs::rename(directory.0.join("profiles.yaml"), directory.0.join("saved.yaml"))?;
    fs::create_dir(directory.0.join("profiles.yaml"))?;
    assert!(store.publish_refresh().is_err());
    store.recover_refresh(None)?;
    assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, prior);
    fs::remove_dir(directory.0.join("profiles.yaml"))?;
    fs::rename(directory.0.join("saved.yaml"), directory.0.join("profiles.yaml"))?;
    assert_eq!(
        serde_yaml_ng::to_value(ProfileStore::open(&directory.0)?.snapshot())?,
        prior
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn unsafe_refresh_journal_or_content_is_rejected_without_catalog_mutation() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let item = store.import_remote(refreshed("proxies: []", 1)?)?;
    let uid = item.uid.as_deref().unwrap();
    let prior = serde_yaml_ng::to_value(store.snapshot())?;
    store.begin_refresh(uid, refreshed("proxies: []\nmode: direct", 20)?, None)?;
    let journal = directory.0.join("profile-refresh.yaml");
    let raw = fs::read_to_string(&journal)?;
    let mut value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&raw)?;
    value["candidate"]["file"] = "../outside.yaml".into();
    fs::write(&journal, serde_yaml_ng::to_string(&value)?)?;
    assert!(store.recover_refresh(None).is_err());
    fs::write(&journal, &raw)?;
    value = serde_yaml_ng::from_str(&raw)?;
    let path = directory
        .0
        .join("profiles")
        .join(value["candidate"]["file"].as_str().unwrap());
    fs::remove_file(&path)?;
    std::os::unix::fs::symlink(directory.0.join("profiles").join(item.file.as_deref().unwrap()), &path)?;
    assert!(store.publish_refresh().is_err());
    assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, prior);
    Ok(())
}

#[test]
fn upstream_refresh_option_merge_overrides_only_supplied_fields() {
    use headless_core::config::PrfOption;
    let saved = PrfOption {
        user_agent: Some("saved-agent".into()),
        update_interval: Some(240),
        allow_auto_update: Some(true),
        script: Some("existing-script".into()),
        ..Default::default()
    };
    let downloaded = PrfOption {
        allow_auto_update: Some(false),
        timeout_seconds: Some(10),
        ..Default::default()
    };
    let merged = PrfOption::merge(Some(&saved), Some(&downloaded)).unwrap();
    assert_eq!(merged.user_agent, saved.user_agent);
    assert_eq!(merged.script, saved.script);
    assert_eq!(merged.update_interval, Some(240));
    assert_eq!(merged.allow_auto_update, Some(false));
    assert_eq!(merged.timeout_seconds, Some(10));
    assert_eq!(PrfOption::merge(Some(&saved), None), Some(saved.clone()));
    assert_eq!(PrfOption::merge(None, Some(&saved)), Some(saved));
    assert_eq!(PrfOption::merge(None, None), None);
}

#[test]
fn metadata_patch_preserves_content_identity_and_merges_only_supported_options() -> Result<()> {
    use headless_core::config::profile_store::{ProfilePatch, RemoteOptionsPatch};
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let item = store.import_remote(refreshed("proxies: []\nmode: rule", 1)?)?;
    let uid = item.uid.as_deref().unwrap();
    store.record_selection(uid, "Main", "REJECT")?;
    store.set_current(Some(uid))?;
    let before = store.get_item(uid)?.clone();
    let updated = store.edit_profile(
        uid,
        ProfilePatch {
            name: Some("new name".into()),
            desc: Some("new description".into()),
            url: Some("https://other.test/sub?token=new".into()),
            options: Some(RemoteOptionsPatch {
                timeout_seconds: Some(7),
                update_interval: Some(0),
                ..Default::default()
            }),
        },
    )?;
    assert_eq!(updated.uid, before.uid);
    assert_eq!(updated.file, before.file);
    assert_eq!(updated.selected, before.selected);
    assert_eq!(updated.updated, before.updated);
    assert_eq!(updated.extra.unwrap().download, 1);
    assert_eq!(updated.option.as_ref().unwrap().timeout_seconds, Some(7));
    assert_eq!(updated.option.as_ref().unwrap().allow_auto_update, Some(false));
    assert_eq!(store.snapshot().current.as_deref(), Some(uid));
    assert_eq!(store.read_mapping(uid)?["mode"].as_str(), Some("rule"));
    let reopened = ProfileStore::open(&directory.0)?;
    assert_eq!(reopened.get_item(uid)?.name.as_deref(), Some("new name"));
    assert_eq!(
        reopened.get_item(uid)?.url.as_deref(),
        Some("https://other.test/sub?token=new")
    );
    Ok(())
}

#[test]
fn invalid_or_failed_metadata_edits_preserve_the_entire_catalog() -> Result<()> {
    use headless_core::config::profile_store::{ProfilePatch, RemoteOptionsPatch};
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let local = store.import_local("local", "mode: direct")?;
    let remote = store.import_remote(refreshed("proxies: []", 1)?)?;
    let uid = remote.uid.as_deref().unwrap();
    let before = serde_yaml_ng::to_value(store.snapshot())?;
    for patch in [
        ProfilePatch {
            name: Some(" ".into()),
            ..Default::default()
        },
        ProfilePatch {
            name: Some("a".repeat(257)),
            ..Default::default()
        },
        ProfilePatch {
            name: Some("would change".into()),
            url: Some("file:///outside".into()),
            ..Default::default()
        },
        ProfilePatch {
            desc: Some("a".repeat(4097)),
            ..Default::default()
        },
        ProfilePatch {
            options: Some(RemoteOptionsPatch {
                timeout_seconds: Some(121),
                ..Default::default()
            }),
            ..Default::default()
        },
        ProfilePatch {
            options: Some(RemoteOptionsPatch {
                user_agent: Some("bad\r\nheader".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
    ] {
        assert!(store.edit_profile(uid, patch).is_err());
        assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, before);
    }
    assert!(
        store
            .edit_profile(
                local.uid.as_deref().unwrap(),
                ProfilePatch {
                    url: Some("https://example.test/sub".into()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert!(store.edit_profile("missing", ProfilePatch::default()).is_err());
    fs::rename(directory.0.join("profiles.yaml"), directory.0.join("saved.yaml"))?;
    fs::create_dir(directory.0.join("profiles.yaml"))?;
    assert!(
        store
            .edit_profile(
                uid,
                ProfilePatch {
                    name: Some("failed".into()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, before);
    Ok(())
}

#[test]
fn deletion_protects_current_and_recovers_on_both_sides_of_catalog_commit() -> Result<()> {
    for committed in [false, true] {
        let directory = Directory::new()?;
        let mut store = ProfileStore::open(&directory.0)?;
        let current = store.import_local("current", "mode: direct")?;
        let uid = current.uid.as_deref().unwrap();
        store.set_current(Some(uid))?;
        assert!(store.delete_profile(uid, None).is_err());
        let target = store.import_remote(refreshed("proxies: []", 1)?)?;
        let target_uid = target.uid.as_deref().unwrap();
        assert!(store.delete_profile(target_uid, Some(target_uid)).is_err());
        let path = directory.0.join("profiles").join(target.file.as_deref().unwrap());
        store.begin_delete(target_uid, Some(uid))?;
        if committed {
            store.publish_delete()?;
        }
        assert!(path.is_file());
        let mut reopened = ProfileStore::open(&directory.0)?;
        reopened.recover_delete()?;
        assert_eq!(reopened.get_item(target_uid).is_ok(), !committed);
        assert_eq!(path.is_file(), !committed);
        assert_eq!(reopened.snapshot().current.as_deref(), Some(uid));
        assert!(!directory.0.join("profile-delete.yaml").exists());
        assert!(reopened.read_mapping(uid).is_ok());
    }
    Ok(())
}

#[test]
fn failed_delete_catalog_write_preserves_file_and_cleanup_failure_can_be_retried() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let item = store.import_local("deletable", "mode: direct")?;
    let uid = item.uid.as_deref().unwrap();
    let path = directory.0.join("profiles").join(item.file.as_deref().unwrap());
    let before = serde_yaml_ng::to_value(store.snapshot())?;
    fs::rename(directory.0.join("profiles.yaml"), directory.0.join("saved.yaml"))?;
    fs::create_dir(directory.0.join("profiles.yaml"))?;
    assert!(store.delete_profile(uid, None).is_err());
    assert!(path.is_file());
    assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, before);
    assert!(!directory.0.join("profile-delete.yaml").exists());
    fs::remove_dir(directory.0.join("profiles.yaml"))?;
    fs::rename(directory.0.join("saved.yaml"), directory.0.join("profiles.yaml"))?;
    store.begin_delete(uid, None)?;
    store.publish_delete()?;
    fs::remove_file(&path)?;
    fs::create_dir(&path)?;
    assert!(store.recover_delete().is_err());
    assert!(directory.0.join("profile-delete.yaml").is_file());
    assert!(store.get_item(uid).is_err());
    fs::remove_dir(&path)?;
    let mut reopened = ProfileStore::open(&directory.0)?;
    reopened.recover_delete()?;
    assert!(reopened.get_item(uid).is_err());
    assert!(!directory.0.join("profile-delete.yaml").exists());
    Ok(())
}

#[test]
fn deletion_keeps_shared_content_and_rejects_enhancement_relationships() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let first = store.import_local("first", "mode: direct")?;
    let second = store.import_local("second", "mode: rule")?;
    let mut catalog = store.snapshot();
    catalog.items.as_mut().unwrap()[1].file = first.file.clone();
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    let mut store = ProfileStore::open(&directory.0)?;
    store.delete_profile(first.uid.as_deref().unwrap(), None)?;
    assert_eq!(
        store.read_mapping(second.uid.as_deref().unwrap())?["mode"].as_str(),
        Some("direct")
    );
    let other = store.import_local("linked", "mode: rule")?;
    let mut catalog = store.snapshot();
    catalog.items.as_mut().unwrap()[1].option = Some(headless_core::config::PrfOption {
        merge: second.uid.clone(),
        ..Default::default()
    });
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    let mut store = ProfileStore::open(&directory.0)?;
    assert!(store.delete_profile(second.uid.as_deref().unwrap(), None).is_err());
    assert!(store.delete_profile(other.uid.as_deref().unwrap(), None).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn deletion_rejects_symlinks_and_unsafe_recovery_paths_without_touching_files() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let item = store.import_local("link", "mode: direct")?;
    let uid = item.uid.as_deref().unwrap();
    let path = directory.0.join("profiles").join(item.file.as_deref().unwrap());
    let outside = directory.0.join("outside.yaml");
    fs::write(&outside, "mode: rule")?;
    fs::remove_file(&path)?;
    std::os::unix::fs::symlink(&outside, &path)?;
    assert!(store.delete_profile(uid, None).is_err());
    assert!(store.get_item(uid).is_ok());
    fs::write(
        directory.0.join("profile-delete.yaml"),
        "schema_version: 1\nuid: missing\nfile: ../outside.yaml\n",
    )?;
    assert!(store.recover_delete().is_err());
    assert_eq!(fs::read_to_string(outside)?, "mode: rule");
    Ok(())
}

#[test]
fn linked_merge_preserves_raw_and_uses_upstream_case_dns_hosts_and_deep_merge() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let raw = "mode: rule\ntun: {enable: false, mtu: 1500}\ndns: {enable: false, nameserver: [9.9.9.9], nameserver-policy: {old: 1.1.1.1}}\nhosts: {old: 127.0.0.1}\n";
    let base = store.import_local("base", raw)?;
    let uid = base.uid.as_deref().unwrap();
    store.record_selection(uid, "Main", "REJECT")?;
    let yaml = "# original merge\nMODE: direct\ntun: {enable: true}\ndns: {nameserver-policy: {new: 8.8.8.8}}\nhosts: {new: 127.0.0.2}\n";
    let plan = store.prepare_merge(uid, Some(yaml.into()))?;
    assert_eq!(
        plan.generation.clone().unwrap().static_mapping()?["mode"].as_str(),
        Some("direct")
    );
    assert_eq!(
        plan.generation.clone().unwrap().static_mapping()?["tun"]["mtu"].as_u64(),
        Some(1500)
    );
    assert!(
        plan.generation.clone().unwrap().static_mapping()?["dns"]["nameserver-policy"]
            .get("old")
            .is_none()
    );
    store.begin_merge(plan, None)?;
    store.publish_merge()?;
    store.recover_merge(None)?;
    let content = store.read_merge(uid)?;
    assert!(content.uid.as_ref().unwrap().starts_with('m'));
    assert_eq!(content.yaml.as_deref(), Some(yaml));
    let item = store.get_item(content.uid.as_ref().unwrap())?;
    assert_eq!(item.itype.as_deref(), Some("merge"));
    assert!(item.file_data.is_none());
    assert_eq!(store.get_item(uid)?.file, base.file);
    assert_eq!(store.selections(uid)?[0].now.as_deref(), Some("REJECT"));
    assert_eq!(
        fs::read_to_string(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,
        raw
    );
    let mapping = ProfileStore::open(&directory.0)?.read_mapping(uid)?;
    assert_eq!(mapping["mode"].as_str(), Some("direct"));
    assert_eq!(mapping["dns"]["nameserver"][0].as_str(), Some("9.9.9.9"));
    assert!(mapping["hosts"].get("old").is_none());
    Ok(())
}

#[test]
fn merge_input_errors_and_catalog_failure_preserve_old_links_and_files() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let base = store.import_local("base", "mode: rule")?;
    let uid = base.uid.as_deref().unwrap();
    let prior = serde_yaml_ng::to_value(store.snapshot())?;
    for yaml in ["- not a mapping", "mode: [", "EXTERNAL-CONTROLLER: '127.0.0.1:9091'"] {
        assert!(store.prepare_merge(uid, Some(yaml.into())).is_err());
        assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, prior);
    }
    assert_eq!(fs::read_dir(directory.0.join("profiles"))?.count(), 1);
    let plan = store.prepare_merge(uid, Some("mode: direct".into()))?;
    store.begin_merge(plan, None)?;
    fs::rename(directory.0.join("profiles.yaml"), directory.0.join("saved.yaml"))?;
    fs::create_dir(directory.0.join("profiles.yaml"))?;
    assert!(store.publish_merge().is_err());
    store.recover_merge(None)?;
    assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, prior);
    assert_eq!(store.read_mapping(uid)?["mode"].as_str(), Some("rule"));
    assert!(!directory.0.join("profile-merge.yaml").exists());
    Ok(())
}

#[test]
fn interrupted_merge_catalog_commit_and_runtime_commit_recover_independently() -> Result<()> {
    use headless_core::config::runtime::{RuntimeStore, parse};
    for active in [false, true] {
        for committed in [false, true] {
            let directory = Directory::new()?;
            let mut store = ProfileStore::open(&directory.0)?;
            let base = store.import_local("base", "mode: rule")?;
            let uid = base.uid.as_deref().unwrap();
            store.record_selection(uid, "Main", "REJECT")?;
            let mut runtime = RuntimeStore::open(&directory.0)?;
            let revision = if active {
                let initial = runtime.stage(parse("mode: rule")?)?;
                runtime.begin_profile(initial, Some(uid.into()))?;
                runtime.commit()?;
                store.set_current(Some(uid))?;
                let candidate = runtime.stage(parse("mode: direct")?)?;
                runtime.begin_profile(candidate.clone(), Some(uid.into()))?;
                Some(candidate)
            } else {
                None
            };
            let plan = store.prepare_merge(uid, Some("mode: direct".into()))?;
            store.begin_merge(plan, revision)?;
            if active || committed {
                store.publish_merge()?;
            }
            if active && committed {
                runtime.commit()?;
            }
            drop(runtime);
            let runtime = RuntimeStore::open(&directory.0)?;
            let mut reopened = ProfileStore::open(&directory.0)?;
            reopened.recover_merge(runtime.state().current.as_ref())?;
            assert_eq!(reopened.read_merge(uid)?.uid.is_some(), committed);
            assert_eq!(reopened.snapshot().items.unwrap().len(), if committed { 2 } else { 1 });
            assert_eq!(
                reopened.read_mapping(uid)?["mode"].as_str(),
                Some(if committed { "direct" } else { "rule" })
            );
            assert_eq!(reopened.selections(uid)?[0].now.as_deref(), Some("REJECT"));
            assert!(!directory.0.join("profile-merge.yaml").exists());
        }
    }
    Ok(())
}

#[test]
fn detaching_merge_preserves_shared_links_and_current_profile_protection() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let first = store.import_local("first", "mode: rule")?;
    let second = store.import_local("second", "mode: rule")?;
    let uid = first.uid.as_deref().unwrap();
    let plan = store.prepare_merge(uid, Some("mode: direct".into()))?;
    store.begin_merge(plan, None)?;
    store.publish_merge()?;
    store.recover_merge(None)?;
    let merge = store.read_merge(uid)?.uid.unwrap();
    let mut catalog = store.snapshot();
    catalog.items.as_mut().unwrap()[1].option = store.get_item(uid)?.option.clone();
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    let mut store = ProfileStore::open(&directory.0)?;
    store.set_current(Some(uid))?;
    assert!(store.delete_profile(uid, None).is_err());
    let plan = store.prepare_merge(uid, None)?;
    store.begin_merge(plan, None)?;
    store.publish_merge()?;
    store.recover_merge(None)?;
    assert!(store.get_item(&merge).is_ok());
    assert_eq!(
        store.read_mapping(second.uid.as_deref().unwrap())?["mode"].as_str(),
        Some("direct")
    );
    let plan = store.prepare_merge(second.uid.as_deref().unwrap(), None)?;
    store.begin_merge(plan, None)?;
    store.publish_merge()?;
    store.recover_merge(None)?;
    assert!(store.get_item(&merge).is_err());
    assert_eq!(store.snapshot().items.unwrap().len(), 2);
    store.set_current(None)?;
    store.delete_profile(uid, None)?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn linked_merge_rejects_missing_wrong_type_and_unsafe_content_or_journal_paths() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let base = store.import_local("base", "mode: rule")?;
    let uid = base.uid.as_deref().unwrap();
    let mut catalog = store.snapshot();
    catalog.items.as_mut().unwrap()[0].option = Some(headless_core::config::PrfOption {
        merge: Some("missing".into()),
        ..Default::default()
    });
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    assert!(ProfileStore::open(&directory.0)?.read_mapping(uid).is_err());
    catalog.items.as_mut().unwrap()[0].option.as_mut().unwrap().merge = base.uid.clone();
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    assert!(ProfileStore::open(&directory.0)?.read_mapping(uid).is_err());
    catalog.items.as_mut().unwrap()[0].option = None;
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    let mut store = ProfileStore::open(&directory.0)?;
    let plan = store.prepare_merge(uid, Some("mode: direct".into()))?;
    store.begin_merge(plan, None)?;
    let journal_path = directory.0.join("profile-merge.yaml");
    let original = fs::read_to_string(&journal_path)?;
    let mut value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&original)?;
    value["candidate"]["items"][1]["file"] = "../outside.yaml".into();
    fs::write(&journal_path, serde_yaml_ng::to_string(&value)?)?;
    assert!(store.publish_merge().is_err());
    fs::write(&journal_path, &original)?;
    value = serde_yaml_ng::from_str(&original)?;
    let file = directory.0.join("profiles").join(value["new_file"].as_str().unwrap());
    fs::remove_file(&file)?;
    std::os::unix::fs::symlink(directory.0.join("profiles").join(base.file.as_deref().unwrap()), &file)?;
    assert!(store.recover_merge(None).is_err());
    assert!(store.read_merge(uid)?.uid.is_none());
    Ok(())
}
