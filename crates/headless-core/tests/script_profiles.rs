use anyhow::Result;
use headless_core::{
    config::{
        profile_store::ProfileStore,
        runtime::{RuntimeStore, parse},
    },
    enhance::script::{ScriptRequest, evaluate},
};
use std::{fs, path::PathBuf};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-script-{}-{stamp:x}", std::process::id()));
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
fn script_main_receives_data_and_lowercases_output_without_host_apis() -> Result<()> {
    let name = "测试'\\\nname";
    let response=evaluate(ScriptRequest { check_only: false, source: "function main(config,name) { console.info(name); if (typeof process !== 'undefined' || typeof fetch !== 'undefined' || typeof require !== 'undefined') throw 'host API'; delete config.mode; config.MODE='global'; config['profile-name']=name; return config; }".into(), config: parse("MODE: rule")?, name: name.into() });
    assert!(response.error.is_none(), "{:?}", response.error);
    let config = response.config.unwrap();
    assert_eq!(config["mode"].as_str(), Some("global"));
    assert_eq!(config["profile-name"].as_str(), Some(name));
    assert_eq!(response.logs.len(), 1);
    Ok(())
}
#[test]
fn script_failures_keep_logs_and_reject_nonobjects_and_controller_escape() -> Result<()> {
    for source in [
        "function main(c) { console.warn('before failure'); throw Error('bad'); }",
        "function main(c) { console.warn('before failure'); return []; }",
        "function main(c) { console.warn('before failure'); return Promise.resolve(c); }",
        "function main(c) { console.warn('before failure'); c['external-controller']='0.0.0.0:1234'; return c; }",
        "function main(c) { console.warn('before failure'); c.self=c; return c; }",
    ] {
        let response = evaluate(ScriptRequest {
            check_only: false,
            source: source.into(),
            config: parse("mode: rule")?,
            name: "base".into(),
        });
        assert!(response.error.is_some());
        assert!(response.config.is_none());
        assert_eq!(response.logs.len(), 1);
    }
    assert!(
        evaluate(ScriptRequest {
            check_only: false,
            source: "function main( {".into(),
            config: parse("mode: rule")?,
            name: "base".into()
        })
        .error
        .is_some()
    );
    Ok(())
}
#[test]
fn script_storage_retains_base_and_shared_links_and_uses_js_schema() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = ProfileStore::open(&directory.0)?;
    let base = store.import_local("base", "mode: rule")?;
    let second = store.import_local("second", "mode: direct")?;
    let uid = base.uid.as_deref().unwrap();
    store.record_selection(uid, "Main", "DIRECT")?;
    for source in ["", " ", &"x".repeat(1024 * 1024 + 1)] {
        assert!(store.prepare_script(uid, Some(source.into())).is_err());
    }
    let source = "// preserved\nfunction main(config,name) { return config; }";
    let plan = store.prepare_script(uid, Some(source.into()))?;
    assert_eq!(plan.generation.as_ref().unwrap().script.as_deref(), Some(source));
    store.begin_enhancement(plan, None)?;
    store.publish_enhancement()?;
    store.recover_enhancement(None)?;
    let content = store.read_script(uid)?;
    assert!(content.uid.as_ref().unwrap().starts_with('s'));
    assert_eq!(content.source.as_deref(), Some(source));
    let auxiliary = store.get_item(content.uid.as_ref().unwrap())?;
    assert_eq!(auxiliary.itype.as_deref(), Some("script"));
    assert!(auxiliary.file.as_deref().unwrap().ends_with(".js"));
    let mut catalog = store.snapshot();
    catalog.items.as_mut().unwrap()[1].option = store.get_item(uid)?.option.clone();
    fs::write(directory.0.join("profiles.yaml"), serde_yaml_ng::to_string(&catalog)?)?;
    let mut store = ProfileStore::open(&directory.0)?;
    for target in [uid, second.uid.as_deref().unwrap()] {
        let plan = store.prepare_script(target, None)?;
        store.begin_enhancement(plan, None)?;
        store.publish_enhancement()?;
        store.recover_enhancement(None)?;
        assert_eq!(store.get_item(content.uid.as_ref().unwrap()).is_ok(), target == uid);
    }
    assert_eq!(store.selections(uid)?[0].now.as_deref(), Some("DIRECT"));
    assert_eq!(
        fs::read_to_string(directory.0.join("profiles").join(base.file.as_deref().unwrap()))?,
        "mode: rule"
    );
    Ok(())
}
#[test]
fn interrupted_script_edits_follow_runtime_or_catalog_commit() -> Result<()> {
    for active in [false, true] {
        for committed in [false, true] {
            let directory = Directory::new()?;
            let mut store = ProfileStore::open(&directory.0)?;
            let base = store.import_local("base", "mode: rule")?;
            let uid = base.uid.as_deref().unwrap();
            let mut runtime = RuntimeStore::open(&directory.0)?;
            if active {
                let initial = runtime.stage(parse("mode: rule")?)?;
                runtime.begin_profile(initial, Some(uid.into()))?;
                runtime.commit()?;
                store.set_current(Some(uid))?;
            }
            let plan = store.prepare_script(uid, Some("function main(c) { c.mode='direct'; return c; }".into()))?;
            let revision = if active {
                let revision = runtime.stage(parse("mode: direct")?)?;
                runtime.begin_profile(revision.clone(), Some(uid.into()))?;
                Some(revision)
            } else {
                None
            };
            store.begin_enhancement(plan, revision)?;
            if active || committed {
                store.publish_enhancement()?;
            }
            if active && committed {
                runtime.commit()?;
            }
            drop(runtime);
            let runtime = RuntimeStore::open(&directory.0)?;
            let mut store = ProfileStore::open(&directory.0)?;
            store.recover_enhancement(runtime.state().current.as_ref())?;
            assert_eq!(store.read_script(uid)?.source.is_some(), committed);
            assert!(!directory.0.join("profile-merge.yaml").exists());
        }
    }
    Ok(())
}
