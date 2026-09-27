use anyhow::Result;
use headless_core::config::{
    PrfItem, PrfOption, dns::ProfileDnsSettings, profile_store::ProfileStore, settings::SettingsStore,
};
use std::{fs, path::PathBuf};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-cascade-{}-{stamp:x}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn setup(dir: &Directory) -> Result<(ProfileStore, SettingsStore, String, String)> {
    let mut store = ProfileStore::open(&dir.0)?;
    store.ensure_global_defaults()?;
    let a = store.import_local("deleted", "mode: direct")?.uid.unwrap().to_string();
    let b = store.import_local("retained", "mode: rule")?.uid.unwrap().to_string();
    let mut catalog = store.snapshot();
    for kind in ["merge", "script", "rules", "proxies", "groups"] {
        let file = format!("aux-{kind}.yaml");
        fs::write(dir.0.join("profiles").join(&file), "fixture content")?;
        catalog.items.as_mut().unwrap().push(PrfItem {
            uid: Some(format!("aux-{kind}").into()),
            itype: Some(kind.into()),
            file: Some(file.into()),
            ..Default::default()
        });
    }
    catalog
        .items
        .as_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item.uid.as_deref() == Some(&a))
        .unwrap()
        .option = Some(PrfOption {
        merge: Some("aux-merge".into()),
        script: Some("aux-script".into()),
        rules: Some("aux-rules".into()),
        proxies: Some("aux-proxies".into()),
        groups: Some("aux-groups".into()),
        ..Default::default()
    });
    fs::write(dir.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    let mut settings = SettingsStore::open(&dir.0)?;
    let mut value = settings.snapshot();
    value
        .profile_dns
        .insert(a.clone(), ProfileDnsSettings { enabled: true });
    value
        .profile_dns
        .insert(b.clone(), ProfileDnsSettings { enabled: false });
    settings.replace(value)?;
    Ok((ProfileStore::open(&dir.0)?, settings, a, b))
}

#[test]
fn cascade_removes_all_five_exclusive_auxiliaries_and_only_deleted_dns_preference() -> Result<()> {
    let dir = Directory::new()?;
    let (mut store, mut settings, a, b) = setup(&dir)?;
    let file = store.get_item(&a)?.file.clone().unwrap();
    let runtime = settings.snapshot().runtime;
    store.set_current(Some(&b))?;
    assert!(store.delete_profile_with_settings(&b, None, &mut settings).is_err());
    assert!(store.delete_profile_with_settings(&a, Some(&a), &mut settings).is_err());
    assert!(settings.snapshot().profile_dns.contains_key(&a));
    store.delete_profile_with_settings(&a, Some(&b), &mut settings)?;
    assert_eq!(store.snapshot().current.as_deref(), Some(b.as_str()));
    assert!(store.get_item(&a).is_err());
    for kind in ["merge", "script", "rules", "proxies", "groups"] {
        assert!(store.get_item(format!("aux-{kind}")).is_err());
        assert!(!dir.0.join("profiles").join(format!("aux-{kind}.yaml")).exists());
    }
    assert!(!dir.0.join("profiles").join(file.as_str()).exists());
    assert!(store.get_item("Merge").is_ok());
    assert!(store.get_item("Script").is_ok());
    assert!(!settings.snapshot().profile_dns.contains_key(&a));
    assert_eq!(settings.snapshot().profile_dns.len(), 1);
    assert_eq!(settings.snapshot().runtime, runtime);
    assert!(!dir.0.join("profile-delete.yaml").exists());
    assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), settings.snapshot());
    Ok(())
}

#[test]
fn shared_links_reserved_rows_and_shared_files_survive_cascade() -> Result<()> {
    let dir = Directory::new()?;
    let (store, mut settings, a, b) = setup(&dir)?;
    let mut catalog = store.snapshot();
    let items = catalog.items.as_mut().unwrap();
    items
        .iter_mut()
        .find(|item| item.uid.as_deref() == Some(&b))
        .unwrap()
        .option = Some(PrfOption {
        merge: Some("aux-merge".into()),
        ..Default::default()
    });
    items
        .iter_mut()
        .find(|item| item.uid.as_deref() == Some(&a))
        .unwrap()
        .option
        .as_mut()
        .unwrap()
        .script = Some("Script".into());
    let shared = items
        .iter()
        .find(|item| item.uid.as_deref() == Some("aux-groups"))
        .unwrap()
        .file
        .clone();
    items
        .iter_mut()
        .find(|item| item.uid.as_deref() == Some("Merge"))
        .unwrap()
        .file = shared;
    fs::write(dir.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    let mut store = ProfileStore::open(&dir.0)?;
    store.delete_profile_with_settings(&a, None, &mut settings)?;
    assert!(store.get_item("aux-merge").is_ok());
    assert!(dir.0.join("profiles/aux-merge.yaml").is_file());
    assert!(store.get_item("aux-groups").is_err());
    assert!(dir.0.join("profiles/aux-groups.yaml").is_file());
    assert!(store.get_item("Script").is_ok());
    assert!(store.get_item("aux-script").is_ok()); // unlinked orphan is outside this deletion
    assert!(store.delete_profile("Merge", None).is_err());
    Ok(())
}

#[test]
fn interruption_uses_single_catalog_commit_for_cascade_and_dns_cleanup() -> Result<()> {
    for committed in [false, true] {
        let dir = Directory::new()?;
        let (mut store, _, a, _) = setup(&dir)?;
        let before = serde_yaml_ng::to_value(store.snapshot())?;
        store.begin_delete(&a, None)?;
        if committed {
            store.publish_delete()?;
        }
        drop(store);
        let mut store = ProfileStore::open(&dir.0)?;
        let mut settings = SettingsStore::open(&dir.0)?;
        store.recover_delete_with_settings(&mut settings)?;
        assert_eq!(store.get_item(&a).is_err(), committed);
        assert_eq!(store.get_item("aux-merge").is_err(), committed);
        assert_eq!(!settings.snapshot().profile_dns.contains_key(&a), committed);
        assert_eq!(!dir.0.join("profiles/aux-merge.yaml").exists(), committed);
        if !committed {
            assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, before);
        }
    }
    Ok(())
}

#[test]
fn settings_failure_keeps_journal_and_files_for_restart_retry() -> Result<()> {
    let dir = Directory::new()?;
    let (mut store, mut settings, a, _) = setup(&dir)?;
    store.begin_delete(&a, None)?;
    store.publish_delete()?;
    fs::rename(dir.0.join("settings.yaml"), dir.0.join("saved-settings.yaml"))?;
    fs::create_dir(dir.0.join("settings.yaml"))?;
    assert!(store.recover_delete_with_settings(&mut settings).is_err());
    assert!(settings.snapshot().profile_dns.contains_key(&a));
    assert!(dir.0.join("profile-delete.yaml").is_file());
    assert!(dir.0.join("profiles/aux-merge.yaml").is_file());
    fs::remove_dir(dir.0.join("settings.yaml"))?;
    fs::rename(dir.0.join("saved-settings.yaml"), dir.0.join("settings.yaml"))?;
    let mut settings = SettingsStore::open(&dir.0)?;
    ProfileStore::open(&dir.0)?.recover_delete_with_settings(&mut settings)?;
    assert!(!settings.snapshot().profile_dns.contains_key(&a));
    assert!(!dir.0.join("profiles/aux-merge.yaml").exists());
    Ok(())
}

#[test]
fn partial_file_cleanup_is_idempotent_and_catalog_write_failure_preserves_preferences() -> Result<()> {
    let dir = Directory::new()?;
    let (mut store, mut settings, a, _) = setup(&dir)?;
    fs::rename(dir.0.join("profiles.yaml"), dir.0.join("saved-catalog.yaml"))?;
    fs::create_dir(dir.0.join("profiles.yaml"))?;
    assert!(store.delete_profile_with_settings(&a, None, &mut settings).is_err());
    assert!(settings.snapshot().profile_dns.contains_key(&a));
    assert!(dir.0.join("profiles/aux-merge.yaml").is_file());
    fs::remove_dir(dir.0.join("profiles.yaml"))?;
    fs::rename(dir.0.join("saved-catalog.yaml"), dir.0.join("profiles.yaml"))?;
    store.begin_delete(&a, None)?;
    store.publish_delete()?;
    fs::remove_file(dir.0.join("profiles/aux-script.yaml"))?;
    fs::create_dir(dir.0.join("profiles/aux-script.yaml"))?;
    assert!(store.recover_delete_with_settings(&mut settings).is_err());
    assert!(!settings.snapshot().profile_dns.contains_key(&a));
    assert!(!dir.0.join("profiles/aux-merge.yaml").exists());
    fs::remove_dir(dir.0.join("profiles/aux-script.yaml"))?;
    store.recover_delete_with_settings(&mut settings)?;
    assert!(!dir.0.join("profile-delete.yaml").exists());
    Ok(())
}

#[test]
fn changed_links_and_forged_reserved_or_duplicate_auxiliaries_fail_closed() -> Result<()> {
    let dir = Directory::new()?;
    let (mut store, _, a, b) = setup(&dir)?;
    store.begin_delete(&a, None)?;
    let path = dir.0.join("profile-delete.yaml");
    let original: serde_yaml_ng::Value = serde_yaml_ng::from_str(&fs::read_to_string(&path)?)?;
    for uid in ["Merge", "Script", "Rules", "Proxies", "Groups", "aux-script", &a, &b] {
        let mut journal = original.clone();
        journal["auxiliaries"][0]["uid"] = uid.into();
        fs::write(&path, serde_yaml_ng::to_string(&journal)?)?;
        assert!(store.publish_delete().is_err());
        assert!(store.get_item(&a).is_ok());
        assert!(dir.0.join("profiles/aux-merge.yaml").is_file());
    }
    fs::write(&path, serde_yaml_ng::to_string(&original)?)?;
    let mut catalog = store.snapshot();
    catalog
        .items
        .as_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item.uid.as_deref() == Some(&b))
        .unwrap()
        .option = store.get_item(&a)?.option.clone();
    fs::write(dir.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    let mut store = ProfileStore::open(&dir.0)?;
    assert!(store.publish_delete().is_err());
    assert!(store.recover_delete().is_err());
    Ok(())
}

#[test]
fn legacy_single_file_journal_recovers_and_cleans_dns() -> Result<()> {
    for committed in [false, true] {
        let dir = Directory::new()?;
        let mut store = ProfileStore::open(&dir.0)?;
        let item = store.import_local("legacy", "mode: direct")?;
        let uid = item.uid.as_deref().unwrap();
        let mut settings = SettingsStore::open(&dir.0)?;
        let mut value = settings.snapshot();
        value
            .profile_dns
            .insert(uid.into(), ProfileDnsSettings { enabled: false });
        settings.replace(value)?;
        fs::write(
            dir.0.join("profile-delete.yaml"),
            serde_yaml_ng::to_string(&serde_json::json!({"schema_version":1,"uid":uid,"file":item.file}))?,
        )?;
        if committed {
            store.publish_delete()?;
        }
        store.recover_delete_with_settings(&mut settings)?;
        assert_eq!(settings.snapshot().profile_dns.contains_key(uid), !committed);
    }
    Ok(())
}

#[test]
fn startup_prunes_old_deleted_and_auxiliary_preferences_without_changing_runtime() -> Result<()> {
    let dir = Directory::new()?;
    let (store, mut settings, a, b) = setup(&dir)?;
    let mut value = settings.snapshot();
    value
        .profile_dns
        .insert("deleted-before-upgrade".into(), ProfileDnsSettings { enabled: true });
    value
        .profile_dns
        .insert("Merge".into(), ProfileDnsSettings { enabled: false });
    settings.replace(value)?;
    let runtime = settings.snapshot().runtime;
    settings.prune_profile_dns(&store.snapshot())?;
    assert_eq!(settings.snapshot().profile_dns.keys().cloned().collect::<Vec<_>>(), {
        let mut keys = vec![a, b];
        keys.sort();
        keys
    });
    assert_eq!(settings.snapshot().runtime, runtime);
    let bytes = fs::read(dir.0.join("settings.yaml"))?;
    settings.prune_profile_dns(&store.snapshot())?;
    assert_eq!(fs::read(dir.0.join("settings.yaml"))?, bytes);
    Ok(())
}

#[cfg(unix)]
#[test]
fn unsafe_auxiliary_cleanup_and_journal_paths_never_remove_external_content() -> Result<()> {
    let dir = Directory::new()?;
    let (mut store, mut settings, a, _) = setup(&dir)?;
    let external = dir.0.join("outside.yaml");
    fs::write(&external, "retained")?;
    let aux = dir.0.join("profiles/aux-merge.yaml");
    fs::remove_file(&aux)?;
    std::os::unix::fs::symlink(&external, &aux)?;
    assert!(store.begin_delete(&a, None).is_err());
    assert!(!dir.0.join("profile-delete.yaml").exists());
    assert!(settings.snapshot().profile_dns.contains_key(&a));
    fs::remove_file(&aux)?;
    fs::write(&aux, "fixture")?;
    store.begin_delete(&a, None)?;
    let path = dir.0.join("profile-delete.yaml");
    let original = fs::read_to_string(&path)?;
    let mut journal: serde_yaml_ng::Value = serde_yaml_ng::from_str(&original)?;
    journal["auxiliaries"][0]["file"] = "../outside.yaml".into();
    fs::write(&path, serde_yaml_ng::to_string(&journal)?)?;
    assert!(store.publish_delete().is_err());
    assert!(store.recover_delete_with_settings(&mut settings).is_err());
    fs::write(&path, original)?;
    store.publish_delete()?;
    fs::remove_file(&aux)?;
    std::os::unix::fs::symlink(&external, &aux)?;
    assert!(store.recover_delete_with_settings(&mut settings).is_err());
    assert_eq!(fs::read_to_string(&external)?, "retained");
    assert!(path.is_file());
    fs::remove_file(&aux)?;
    store.recover_delete_with_settings(&mut settings)?;
    assert_eq!(fs::read_to_string(&external)?, "retained");
    Ok(())
}
