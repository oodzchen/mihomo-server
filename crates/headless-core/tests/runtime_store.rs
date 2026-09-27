use std::path::PathBuf;

use anyhow::{Result, ensure};
use headless_core::config::runtime::{Revision, RuntimeStore, generate, parse};

struct Directory(PathBuf);

impl Directory {
    fn new() -> Result<Self> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = Self(std::env::temp_dir().join(format!("ms-store-{}-{unique:x}", std::process::id())));
        std::fs::create_dir_all(&directory.0)?;
        Ok(directory)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn interrupted_application_recovers_committed_revision_and_explicit_rollback_restores_it() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = RuntimeStore::open(&directory.0)?;
    let first = store.stage(parse("mode: rule\nrules: [MATCH,DIRECT]")?)?;
    store.begin_profile(first.clone(), Some("profile-one".into()))?;
    store.commit()?;
    let committed = store.state();
    let second = store.stage(parse("mode: direct")?)?;
    store.begin_profile(second.clone(), Some("profile-two".into()))?;
    drop(store); // Simulate process exit between apply and commit.
    let mut store = RuntimeStore::open(&directory.0)?;
    assert_eq!(store.state().current, Some(first.clone()));
    ensure!(store.state().pending.is_none());
    assert_eq!(store.state().active_profile.as_deref(), Some("profile-one"));
    ensure!(store.state().pending_profile.is_none());
    assert_eq!(store.read_current()?["mode"].as_str(), Some("rule"));
    store.begin_profile(second.clone(), Some("profile-two".into()))?;
    store.commit()?;
    assert_eq!(store.state().current, Some(second));
    assert_eq!(store.state().active_profile.as_deref(), Some("profile-two"));
    store.restore(committed)?;
    drop(store);
    let store = RuntimeStore::open(&directory.0)?;
    assert_eq!(store.state().current, Some(first));
    assert_eq!(store.state().active_profile.as_deref(), Some("profile-one"));
    assert_eq!(store.read_current()?["mode"].as_str(), Some("rule"));
    Ok(())
}

#[test]
fn malformed_manifests_and_paths_are_rejected_but_missing_config_can_be_repaired() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = RuntimeStore::open(&directory.0)?;
    ensure!(
        store
            .path(&Revision {
                file: "../../outside.yaml".into()
            })
            .is_err()
    );
    let revision = store.stage(parse("mode: rule")?)?;
    let path = store.path(&revision)?;
    store.begin(revision)?;
    store.commit()?;
    std::fs::remove_file(path)?;
    let mut repairable = RuntimeStore::open(&directory.0)?;
    ensure!(repairable.read_current().is_err());
    let replacement = repairable.stage(parse("mode: direct")?)?;
    repairable.begin(replacement)?;
    repairable.commit()?;
    assert_eq!(repairable.read_current()?["mode"].as_str(), Some("direct"));
    std::fs::write(
        directory.0.join("config/state.yaml"),
        "schema_version: 999\ncurrent: null\npending: null",
    )?;
    ensure!(RuntimeStore::open(&directory.0).is_err());
    Ok(())
}

#[test]
fn candidate_write_failure_preserves_memory_and_committed_bytes() -> Result<()> {
    let directory = Directory::new()?;
    let mut store = RuntimeStore::open(&directory.0)?;
    let revision = store.stage(parse("mode: rule")?)?;
    std::fs::create_dir(directory.0.join("config/state.yaml"))?;
    ensure!(store.begin(revision).is_err());
    ensure!(store.state().current.is_none() && store.state().pending.is_none());
    ensure!(store.read_current().is_err());
    Ok(())
}

#[test]
fn generation_retains_merge_semantics_and_controller_ownership() -> Result<()> {
    let base = parse(
        "dns: {enable: false, nameserver: [1.1.1.1], nameserver-policy: {old: 8.8.8.8}}\nhosts: {old: 127.0.0.1}",
    )?;
    let overlay =
        serde_yaml_ng::from_str("DNS: {enable: true, nameserver-policy: {new: 9.9.9.9}}\nhosts: {new: 127.0.0.2}")?;
    let generated = generate(base, &overlay)?;
    assert_eq!(generated["dns"]["enable"].as_bool(), Some(true));
    assert!(generated["dns"]["nameserver-policy"].get("old").is_none());
    assert!(generated["hosts"].get("old").is_none());
    assert!(generated["dns"].get("nameserver").is_some());
    for field in [
        "external-controller",
        "external-controller-tls",
        "external-controller-unix",
        "external-controller-pipe",
    ] {
        ensure!(parse(&format!("{field}: /outside")).is_err());
        ensure!(parse(&format!("{field}: ''")).is_ok());
    }
    ensure!(parse("- not-a-mapping").is_err());
    ensure!(parse("mode: [").is_err());
    Ok(())
}

#[test]
fn staging_archived_yaml_retains_exact_bytes_and_controller_boundary() -> Result<()> {
    let directory = Directory::new()?;
    let store = RuntimeStore::open(&directory.0)?;
    let yaml = "# archived manual edit\r\nmode: direct\r\nexternal-controller: ''\r\n";
    let revision = store.stage_yaml(yaml)?;
    assert_eq!(std::fs::read_to_string(store.path(&revision)?)?, yaml);
    assert!(store.stage_yaml("external-controller: '0.0.0.0:9090'\n").is_err());
    assert!(store.stage_yaml("- not-a-mapping\n").is_err());
    Ok(())
}
