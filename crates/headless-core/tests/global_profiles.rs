use anyhow::Result;
use headless_core::{
    config::{
        PrfItem,
        profile_store::{DEFAULT_GLOBAL_MERGE, DEFAULT_GLOBAL_SCRIPT, GenerationPlan, ProfileStore},
        runtime,
    },
    enhance::script::{ScriptRequest, evaluate},
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "ms-globals-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn replace(directory: &Directory, store: &ProfileStore, uid: &str, source: &str) -> Result<()> {
    fs::write(
        directory
            .0
            .join("profiles")
            .join(store.get_item(uid)?.file.as_deref().unwrap()),
        source,
    )?;
    Ok(())
}
fn execute(plan: GenerationPlan) -> Result<serde_yaml_ng::Mapping> {
    let global = evaluate(ScriptRequest {
        check_only: false,
        source: plan.global_script.unwrap(),
        config: runtime::generate(plan.config, &plan.global_merge)?,
        name: plan.name.clone(),
    });
    assert!(global.error.is_none(), "{:?}", global.error);
    let config = runtime::generate(global.config.unwrap(), &plan.profile_merge)?;
    let profile = evaluate(ScriptRequest {
        check_only: false,
        source: plan.script.unwrap(),
        config,
        name: plan.name,
    });
    assert!(profile.error.is_none(), "{:?}", profile.error);
    Ok(profile.config.unwrap())
}
#[test]
fn defaults_are_idempotent_preserve_existing_sources_and_raw_nodes() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let base = store.import_local("base", "mode: rule")?;
    let uid = base.uid.as_deref().unwrap();
    store.record_selection(uid, "Main", "REJECT")?;
    store.ensure_global_defaults()?;
    assert_eq!(store.snapshot().items.unwrap().len(), 3);
    for (uid, kind, extension, source) in [
        ("Merge", "merge", ".yaml", DEFAULT_GLOBAL_MERGE),
        ("Script", "script", ".js", DEFAULT_GLOBAL_SCRIPT),
    ] {
        let item = store.get_item(uid)?;
        assert_eq!(item.itype.as_deref(), Some(kind));
        assert!(item.file.as_deref().unwrap().ends_with(extension));
        assert_eq!(
            fs::read_to_string(directory.0.join("profiles").join(item.file.as_deref().unwrap()))?,
            source
        );
    }
    replace(&directory, &store, "Merge", "mode: direct")?;
    replace(
        &directory,
        &store,
        "Script",
        "function main(c) { c.trace=(c.trace||'')+'G'; return c; }",
    )?;
    let catalog = fs::read(directory.0.join("profiles.yaml"))?;
    store.ensure_global_defaults()?;
    assert_eq!(fs::read(directory.0.join("profiles.yaml"))?, catalog);
    let mut reopened = ProfileStore::open(&directory.0)?;
    reopened.ensure_global_defaults()?;
    let generated = execute(reopened.read_generation(uid)?)?;
    // Upstream fallback applies reserved defaults as both global and profile stages.
    assert_eq!(generated["trace"].as_str(), Some("GG"));
    assert_eq!(generated["mode"].as_str(), Some("direct"));
    assert!(reopened.read_mapping(uid).is_err()); // Never silently bypass custom scripts.
    assert!(reopened.read_merge(uid)?.uid.is_none()); // Editor reads explicit links only.
    assert_eq!(reopened.selections(uid)?[0].now.as_deref(), Some("REJECT"));
    assert_eq!(
        fs::read_to_string(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,
        "mode: rule"
    );
    Ok(())
}
#[test]
fn staged_generation_preserves_sequence_global_and_profile_order_and_detach_fallback() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    store.ensure_global_defaults()?;
    let base = store.import_local("quoted ' name", "mode: rule\nrules: ['MATCH,DIRECT']")?;
    let uid = base.uid.as_deref().unwrap();
    let sequence = store.prepare_sequence(
        uid,
        headless_core::config::profile_store::SequenceKind::Rules,
        Some("prepend: ['DOMAIN,sequence.test,DIRECT']\nappend: []\ndelete: []".into()),
    )?;
    store.begin_enhancement(sequence, None)?;
    store.publish_enhancement()?;
    store.recover_enhancement(None)?;
    replace(&directory, &store, "Merge", "mode: global\nglobal-merge: true")?;
    replace(
        &directory,
        &store,
        "Script",
        "function main(c,name) { if (c.mode !== 'global' || c.rules[0] !== 'DOMAIN,sequence.test,DIRECT' || name !== \"quoted ' name\") throw 'order'; c.trace=(c.trace||'')+'G'; c.mode='direct'; return c; }",
    )?;
    let merge = store.prepare_merge(uid, Some("mode: rule\nprofile-merge: true".into()))?;
    store.begin_enhancement(merge, None)?;
    store.publish_enhancement()?;
    store.recover_enhancement(None)?;
    let script = store.prepare_script(uid, Some("function main(c) { if(c.mode !== 'rule' || c.trace !== 'G' || !c['profile-merge']) throw 'order'; c.trace+='P'; return c; }".into()))?;
    assert_eq!(
        execute(script.generation.clone().unwrap())?["trace"].as_str(),
        Some("GP")
    );
    store.begin_enhancement(script, None)?;
    store.publish_enhancement()?;
    store.recover_enhancement(None)?;
    assert_eq!(execute(store.read_generation(uid)?)?["trace"].as_str(), Some("GP"));
    let detach = store.prepare_script(uid, None)?;
    assert_eq!(
        detach.generation.as_ref().unwrap().script,
        detach.generation.as_ref().unwrap().global_script
    );
    assert_eq!(store.get_item("Merge")?.itype.as_deref(), Some("merge"));
    Ok(())
}
#[test]
fn initialization_defers_to_journal_recovery_and_cleans_unpublished_files() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    store.import_local("base", "mode: rule")?;
    for journal in ["profile-merge.yaml", "profile-refresh.yaml", "profile-delete.yaml"] {
        fs::write(directory.0.join(journal), "pending")?;
        assert!(store.ensure_global_defaults().is_err());
        assert_eq!(fs::read_dir(directory.0.join("profiles"))?.count(), 1);
        fs::remove_file(directory.0.join(journal))?;
    }
    fs::rename(directory.0.join("profiles.yaml"), directory.0.join("saved.yaml"))?;
    fs::create_dir(directory.0.join("profiles.yaml"))?;
    assert!(store.ensure_global_defaults().is_err());
    assert_eq!(store.snapshot().items.unwrap().len(), 1);
    assert_eq!(fs::read_dir(directory.0.join("profiles"))?.count(), 1);
    fs::remove_dir(directory.0.join("profiles.yaml"))?;
    fs::rename(directory.0.join("saved.yaml"), directory.0.join("profiles.yaml"))?;
    store.ensure_global_defaults()?;
    assert_eq!(store.snapshot().items.unwrap().len(), 3);
    Ok(())
}
#[test]
fn reserved_defaults_and_sequence_fallback_fail_closed_on_invalid_sources() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let base = store.import_local("base", "mode: rule\nrules: ['MATCH,DIRECT']")?;
    let uid = base.uid.as_deref().unwrap();
    store.ensure_global_defaults()?;
    assert_eq!(
        store.read_mapping(uid)?["profile"]["store-selected"].as_bool(),
        Some(true)
    );
    let mut catalog = store.snapshot();
    catalog.items.as_mut().unwrap().push(PrfItem {
        uid: Some("Rules".into()),
        itype: Some("rules".into()),
        file: Some("Rules.yaml".into()),
        ..Default::default()
    });
    fs::write(
        directory.0.join("profiles/Rules.yaml"),
        "prepend: ['DOMAIN,reserved.test,DIRECT']\nappend: []\ndelete: []",
    )?;
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    let store = ProfileStore::open(&directory.0)?;
    assert_eq!(
        store.read_mapping(uid)?["rules"][0].as_str(),
        Some("DOMAIN,reserved.test,DIRECT")
    );
    for (reserved, source) in [
        ("Merge", "external-controller: 0.0.0.0:9090"),
        ("Script", " "),
        ("Rules", "prepend: []\nunknown: true"),
    ] {
        let file = directory
            .0
            .join("profiles")
            .join(store.get_item(reserved)?.file.as_deref().unwrap());
        let original = fs::read(&file)?;
        fs::write(&file, source)?;
        assert!(store.read_generation(uid).is_err());
        fs::write(file, original)?;
    }
    catalog
        .items
        .as_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row.uid.as_deref() == Some("Merge"))
        .unwrap()
        .itype = Some("local".into());
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    assert!(ProfileStore::open(&directory.0)?.read_generation(uid).is_err());
    // An existing catalog without an items list is preserved, matching upstream.
    fs::write(directory.0.join("profiles.yaml"), "current: null\nitems: null\n")?;
    let mut store = ProfileStore::open(&directory.0)?;
    store.ensure_global_defaults()?;
    assert!(store.snapshot().items.is_none());
    Ok(())
}

#[test]
fn global_replacement_keeps_reserved_uid_links_metadata_raw_and_nodes() -> Result<()> {
    use headless_core::config::{PrfOption, profile_store::GlobalEnhancementKind};
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    store.ensure_global_defaults()?;
    let base = store.import_local("base", "mode: rule")?;
    let uid = base.uid.as_deref().unwrap();
    store.record_selection(uid, "Main", "REJECT")?;
    let mut catalog = store.snapshot();
    catalog
        .items
        .as_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row.uid.as_deref() == Some(uid))
        .unwrap()
        .option = Some(PrfOption {
        merge: Some("Merge".into()),
        script: Some("Script".into()),
        ..Default::default()
    });
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    let mut store = ProfileStore::open(&directory.0)?;
    let saved_base = serde_yaml_ng::to_value(store.get_item(uid)?)?;
    let old_file = store.get_item("Merge")?.file.clone().unwrap().to_string();
    let plan = store.prepare_global(
        GlobalEnhancementKind::Merge,
        "# kept source\nmode: direct".into(),
        Some(uid),
    )?;
    assert_eq!(
        plan.generation.as_ref().unwrap().global_merge["mode"].as_str(),
        Some("direct")
    );
    assert_eq!(
        plan.generation.as_ref().unwrap().profile_merge["mode"].as_str(),
        Some("direct")
    );
    store.begin_enhancement(plan, None)?;
    store.publish_enhancement()?;
    store.recover_enhancement(None)?;
    assert_eq!(store.get_item("Merge")?.uid.as_deref(), Some("Merge"));
    assert_ne!(store.get_item("Merge")?.file.as_deref(), Some(old_file.as_str()));
    assert_eq!(
        fs::read_to_string(directory.0.join("profiles").join(old_file))?,
        DEFAULT_GLOBAL_MERGE
    );
    assert_eq!(serde_yaml_ng::to_value(store.get_item(uid)?)?, saved_base);
    assert_eq!(store.snapshot().items.unwrap().len(), 3);
    let source = "function main(c) { c.trace=(c.trace||'')+'G'; return c; }";
    let script = store.prepare_global(GlobalEnhancementKind::Script, source.into(), Some(uid))?;
    assert_eq!(
        execute(script.generation.clone().unwrap())?["trace"].as_str(),
        Some("GG")
    );
    store.begin_enhancement(script, None)?;
    store.publish_enhancement()?;
    store.recover_enhancement(None)?;
    assert_eq!(
        store.read_global(GlobalEnhancementKind::Script)?.yaml.as_deref(),
        Some(source)
    );
    let reset = store.prepare_global(GlobalEnhancementKind::Script, DEFAULT_GLOBAL_SCRIPT.into(), Some(uid))?;
    store.begin_enhancement(reset, None)?;
    store.publish_enhancement()?;
    store.recover_enhancement(None)?;
    assert_eq!(
        store.read_global(GlobalEnhancementKind::Script)?.yaml.as_deref(),
        Some(DEFAULT_GLOBAL_SCRIPT)
    );
    assert_eq!(
        fs::read_to_string(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,
        "mode: rule"
    );
    assert_eq!(serde_yaml_ng::to_value(store.get_item(uid)?)?, saved_base);
    Ok(())
}

#[test]
fn global_interruption_recovery_uses_runtime_commit_or_catalog_pointer() -> Result<()> {
    use headless_core::config::{profile_store::GlobalEnhancementKind, runtime::RuntimeStore};
    for kind in [GlobalEnhancementKind::Merge, GlobalEnhancementKind::Script] {
        for active in [false, true] {
            for phase in 0..=2 {
                let directory = Directory::new()?;
                let mut store = ProfileStore::open(&directory.0)?;
                store.ensure_global_defaults()?;
                let base = store.import_local("base", "mode: rule")?;
                let uid = base.uid.as_deref().unwrap();
                let old_file = store.get_item(kind.uid())?.file.clone();
                let mut runtime = RuntimeStore::open(&directory.0)?;
                if active {
                    let initial = runtime.stage(runtime::parse("mode: rule")?)?;
                    runtime.begin_profile(initial, Some(uid.into()))?;
                    runtime.commit()?;
                    store.set_current(Some(uid))?;
                }
                let source = if kind == GlobalEnhancementKind::Merge {
                    "mode: direct"
                } else {
                    "function main(c) { c.mode='direct'; return c; }"
                };
                let plan = store.prepare_global(kind, source.into(), active.then_some(uid))?;
                assert_eq!(plan.generation.is_some(), active);
                let revision = if active {
                    let revision = runtime.stage(runtime::parse("mode: direct")?)?;
                    runtime.begin_profile(revision.clone(), Some(uid.into()))?;
                    Some(revision)
                } else {
                    None
                };
                store.begin_enhancement(plan, revision)?;
                if phase >= 1 {
                    store.publish_enhancement()?;
                }
                if active && phase == 2 {
                    runtime.commit()?;
                }
                drop(runtime);
                let runtime = RuntimeStore::open(&directory.0)?;
                let mut reopened = ProfileStore::open(&directory.0)?;
                reopened.recover_enhancement(runtime.state().current.as_ref())?;
                let accepted = if active { phase == 2 } else { phase >= 1 };
                assert_eq!(reopened.get_item(kind.uid())?.file != old_file, accepted);
                assert_eq!(
                    reopened.read_global(kind)?.yaml.as_deref(),
                    Some(if accepted { source } else { kind.default_source() })
                );
                assert!(!directory.0.join("profile-merge.yaml").exists());
                reopened.ensure_global_defaults()?;
                assert_eq!(reopened.snapshot().items.unwrap().len(), 3);
            }
        }
    }
    Ok(())
}

#[test]
fn global_first_row_recovery_and_broken_source_repair_are_supported() -> Result<()> {
    use headless_core::config::profile_store::GlobalEnhancementKind;
    for published in [false, true] {
        let directory = Directory::new()?;
        let mut store = ProfileStore::open(&directory.0)?;
        let plan = store.prepare_global(GlobalEnhancementKind::Merge, "mode: direct".into(), None)?;
        store.begin_enhancement(plan, None)?;
        if published {
            store.publish_enhancement()?;
        }
        let mut reopened = ProfileStore::open(&directory.0)?;
        reopened.recover_enhancement(None)?;
        assert_eq!(reopened.get_item("Merge").is_ok(), published);
        reopened.ensure_global_defaults()?;
        let base = reopened.import_local("base", "mode: rule")?;
        replace(&directory, &reopened, "Script", " ")?;
        assert!(reopened.read_generation(base.uid.as_deref().unwrap()).is_err());
        let repair = reopened.prepare_global(
            GlobalEnhancementKind::Script,
            DEFAULT_GLOBAL_SCRIPT.into(),
            base.uid.as_deref(),
        )?;
        assert!(repair.generation.unwrap().static_mapping().is_ok());
    }
    Ok(())
}

#[test]
fn global_journals_reject_unrelated_changes_wrong_scope_and_unsafe_files() -> Result<()> {
    use headless_core::config::profile_store::GlobalEnhancementKind;
    for mutation in ["unrelated", "target", "kind", "path"] {
        let directory = Directory::new()?;
        let mut store = ProfileStore::open(&directory.0)?;
        store.ensure_global_defaults()?;
        let base = store.import_local("base", "mode: rule")?;
        let plan = store.prepare_global(GlobalEnhancementKind::Merge, "mode: direct".into(), None)?;
        store.begin_enhancement(plan, None)?;
        let path = directory.0.join("profile-merge.yaml");
        let mut journal: serde_yaml_ng::Value = serde_yaml_ng::from_str(&fs::read_to_string(&path)?)?;
        match mutation {
            "unrelated" => journal["candidate"]["items"][2]["name"] = "tampered".into(),
            "target" => journal["uid"] = base.uid.as_deref().unwrap().into(),
            "kind" => journal["kind"] = "rules".into(),
            "path" => journal["candidate"]["items"][0]["file"] = "../outside.yaml".into(),
            _ => unreachable!(),
        }
        fs::write(path, serde_yaml_ng::to_string(&journal)?)?;
        assert!(store.recover_enhancement(None).is_err());
        assert_eq!(
            store.read_global(GlobalEnhancementKind::Merge)?.yaml.as_deref(),
            Some(DEFAULT_GLOBAL_MERGE)
        );
    }
    Ok(())
}

#[test]
fn syntax_only_checks_never_execute_source_or_profile_dependent_main() -> Result<()> {
    for source in [
        "while(true) {} function main(c) { throw 'must not run'; }",
        "function main(c) { return c['proxy-groups'][0]; }",
    ] {
        let response = evaluate(ScriptRequest {
            source: source.into(),
            config: serde_yaml_ng::Mapping::new(),
            name: String::new(),
            check_only: true,
        });
        assert!(response.error.is_none(), "{:?}", response.error);
        assert!(response.config.unwrap().is_empty());
        assert!(response.logs.is_empty());
    }
    let response = evaluate(ScriptRequest {
        source: "function main( {".into(),
        config: serde_yaml_ng::Mapping::new(),
        name: String::new(),
        check_only: true,
    });
    assert!(response.error.unwrap().contains("syntax"));
    Ok(())
}
