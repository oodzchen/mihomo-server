use anyhow::Result;
use headless_core::config::{
    profile_store::{ProfileStore, SequenceKind},
    runtime::{RuntimeStore, parse},
};
use std::{fs, path::PathBuf};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = Self(std::env::temp_dir().join(format!("ms-sequences-{}-{stamp:x}", std::process::id())));
        fs::create_dir_all(&directory.0)?;
        Ok(directory)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
const KINDS: [SequenceKind; 3] = [SequenceKind::Rules, SequenceKind::Proxies, SequenceKind::Groups];
const EMPTY: &str = "prepend: []\nappend: []\ndelete: []\n";
fn save(store: &mut ProfileStore, uid: &str, kind: SequenceKind, yaml: Option<&str>) -> Result<()> {
    let plan = store.prepare_sequence(uid, kind, yaml.map(str::to_owned))?;
    store.begin_merge(plan, None)?;
    store.publish_merge()?;
    store.recover_merge(None)
}

#[test]
fn linked_sequences_follow_upstream_order_preserve_raw_and_merge_overrides() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let raw = "proxies: [{name: Old, type: direct}]\nproxy-groups: [{name: Main, type: select, proxies: [Old, DIRECT]}, {name: Second, type: select, proxies: [Old]}]\nrules: ['MATCH,DIRECT']\n";
    let base = store.import_local("base", raw)?;
    let uid = base.uid.as_deref().unwrap();
    store.record_selection(uid, "Main", "DIRECT")?;
    for (kind, yaml, prefix, itype) in [
        (
            SequenceKind::Rules,
            "# rules\nprepend: ['DOMAIN,sequence.test,DIRECT']\nappend: ['MATCH,REJECT']\ndelete: ['MATCH,DIRECT']\n",
            'r',
            "rules",
        ),
        (
            SequenceKind::Proxies,
            "prepend: [{name: Added, type: direct}]\nappend: []\ndelete: [Old]\n",
            'p',
            "proxies",
        ),
        (
            SequenceKind::Groups,
            "prepend: [{name: Front, type: select, proxies: [DIRECT]}]\nappend: []\ndelete: [Second]\n",
            'g',
            "groups",
        ),
    ] {
        save(&mut store, uid, kind, Some(yaml))?;
        let saved = store.read_sequence(uid, kind)?;
        assert!(saved.uid.as_ref().unwrap().starts_with(prefix));
        assert_eq!(saved.yaml.as_deref(), Some(yaml));
        assert_eq!(store.get_item(saved.uid.unwrap())?.itype.as_deref(), Some(itype));
    }
    let config = store.read_mapping(uid)?;
    assert_eq!(
        config["rules"],
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>("['DOMAIN,sequence.test,DIRECT', 'MATCH,REJECT']")?
    );
    assert_eq!(config["proxies"][0]["name"].as_str(), Some("Added"));
    assert_eq!(config["proxy-groups"][0]["name"].as_str(), Some("Front"));
    assert_eq!(config["proxy-groups"][0]["proxies"][0].as_str(), Some("DIRECT"));
    assert_eq!(
        config["proxy-groups"][1]["proxies"],
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>("[Added, DIRECT]")?
    );
    let plan = store.prepare_merge(uid, Some("rules: ['MATCH,DIRECT']".into()))?;
    store.begin_merge(plan, None)?;
    store.publish_merge()?;
    store.recover_merge(None)?;
    assert_eq!(store.read_mapping(uid)?["rules"][0].as_str(), Some("MATCH,DIRECT"));
    // Sequence edits must keep the saved merge and all other sequence links.
    save(&mut store, uid, SequenceKind::Rules, Some(EMPTY))?;
    let reopened = ProfileStore::open(&directory.0)?;
    assert_eq!(reopened.read_mapping(uid)?["rules"][0].as_str(), Some("MATCH,DIRECT"));
    assert_eq!(reopened.snapshot().items.unwrap().len(), 5);
    assert_eq!(reopened.selections(uid)?[0].now.as_deref(), Some("DIRECT"));
    assert_eq!(reopened.get_item(uid)?.file, base.file);
    assert_eq!(
        fs::read_to_string(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,
        raw
    );
    Ok(())
}

#[test]
fn sequence_recovery_tracks_runtime_or_catalog_and_reads_legacy_merge_journals() -> Result<()> {
    for kind in KINDS {
        for active in [false, true] {
            for accepted in [false, true] {
                let directory = Directory::new()?;
                let mut store = ProfileStore::open(&directory.0)?;
                let base = store.import_local("base", "mode: rule")?;
                let uid = base.uid.as_deref().unwrap();
                let mut runtime = RuntimeStore::open(&directory.0)?;
                if active {
                    let revision = runtime.stage(parse("mode: rule")?)?;
                    runtime.begin_profile(revision, Some(uid.into()))?;
                    runtime.commit()?;
                    store.set_current(Some(uid))?;
                }
                let plan = store.prepare_sequence(uid, kind, Some(EMPTY.into()))?;
                let revision = if active {
                    let revision = runtime.stage(plan.generation.clone().unwrap().static_mapping()?)?;
                    runtime.begin_profile(revision.clone(), Some(uid.into()))?;
                    Some(revision)
                } else {
                    None
                };
                store.begin_merge(plan, revision)?;
                if active || accepted {
                    store.publish_merge()?;
                }
                if active && accepted {
                    runtime.commit()?;
                }
                drop(runtime);
                let runtime = RuntimeStore::open(&directory.0)?;
                let mut reopened = ProfileStore::open(&directory.0)?;
                reopened.recover_merge(runtime.state().current.as_ref())?;
                assert_eq!(reopened.read_sequence(uid, kind)?.uid.is_some(), accepted);
                assert_eq!(reopened.snapshot().items.unwrap().len(), if accepted { 2 } else { 1 });
                assert!(!directory.0.join("profile-merge.yaml").exists());
            }
        }
    }
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let base = store.import_local("legacy", "mode: rule")?;
    let uid = base.uid.as_deref().unwrap();
    let plan = store.prepare_merge(uid, Some("mode: direct".into()))?;
    store.begin_merge(plan, None)?;
    let path = directory.0.join("profile-merge.yaml");
    let mut journal: serde_yaml_ng::Mapping = serde_yaml_ng::from_str(&fs::read_to_string(&path)?)?;
    journal.remove("kind");
    fs::write(&path, serde_yaml_ng::to_string(&journal)?)?;
    store.publish_merge()?;
    store.recover_merge(None)?;
    assert_eq!(store.read_mapping(uid)?["mode"].as_str(), Some("direct"));
    Ok(())
}

#[test]
fn sequence_inputs_links_and_failed_publication_preserve_catalog() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let base = store.import_local("base", "mode: rule")?;
    let uid = base.uid.as_deref().unwrap();
    let before = serde_yaml_ng::to_value(store.snapshot())?;
    for kind in KINDS {
        for yaml in [
            "- list",
            "prepend: []",
            "prepend: null\nappend: []\ndelete: []",
            "prepend: []\nappend: []\ndelete: []\nunknown: []",
            "prepend: []\nappend: []\ndelete: ['']",
            "prepend: [1]\nappend: []\ndelete: []",
            "prepend: [{name: missing-type}]\nappend: []\ndelete: []",
        ] {
            assert!(store.prepare_sequence(uid, kind, Some(yaml.into())).is_err());
        }
    }
    assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, before);
    assert_eq!(fs::read_dir(directory.0.join("profiles"))?.count(), 1);
    let plan = store.prepare_sequence(uid, SequenceKind::Rules, Some(EMPTY.into()))?;
    store.begin_merge(plan, None)?;
    let journal_path = directory.0.join("profile-merge.yaml");
    let original = fs::read_to_string(&journal_path)?;
    let mut journal: serde_yaml_ng::Value = serde_yaml_ng::from_str(&original)?;
    journal["kind"] = "script".into();
    fs::write(&journal_path, serde_yaml_ng::to_string(&journal)?)?;
    assert!(store.recover_merge(None).is_err());
    fs::write(&journal_path, original)?;
    fs::rename(directory.0.join("profiles.yaml"), directory.0.join("saved.yaml"))?;
    fs::create_dir(directory.0.join("profiles.yaml"))?;
    assert!(store.publish_merge().is_err());
    store.recover_merge(None)?;
    assert_eq!(serde_yaml_ng::to_value(store.snapshot())?, before);
    Ok(())
}

#[test]
fn sequence_detach_preserves_shared_links_and_refuses_unsafe_content() -> Result<()> {
    for kind in KINDS {
        let directory = Directory::new()?;
        let mut store = ProfileStore::open(&directory.0)?;
        let first = store.import_local("first", "mode: rule")?;
        let second = store.import_local("second", "mode: rule")?;
        let uid = first.uid.as_deref().unwrap();
        save(&mut store, uid, kind, Some(EMPTY))?;
        let auxiliary = store.read_sequence(uid, kind)?.uid.unwrap();
        let mut catalog = store.snapshot();
        catalog.items.as_mut().unwrap()[1].option = store.get_item(uid)?.option.clone();
        fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
        let mut store = ProfileStore::open(&directory.0)?;
        assert!(store.delete_profile(uid, None).is_err());
        save(&mut store, uid, kind, None)?;
        assert!(store.get_item(&auxiliary).is_ok());
        save(&mut store, second.uid.as_deref().unwrap(), kind, None)?;
        assert!(store.get_item(&auxiliary).is_err());
        save(&mut store, uid, kind, Some(EMPTY))?;
        let auxiliary = store.read_sequence(uid, kind)?.uid.unwrap();
        let path = directory
            .0
            .join("profiles")
            .join(store.get_item(&auxiliary)?.file.as_deref().unwrap());
        fs::write(&path, "prepend: []\nappend: []\ndelete: []\nunexpected: true")?;
        assert!(store.read_mapping(uid).is_err());
        #[cfg(unix)]
        {
            fs::remove_file(&path)?;
            std::os::unix::fs::symlink(directory.0.join("profiles").join(first.file.as_deref().unwrap()), &path)?;
            assert!(store.read_sequence(uid, kind).is_err());
        }
    }
    Ok(())
}
