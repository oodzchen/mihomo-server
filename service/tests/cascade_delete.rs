#![cfg(target_os = "linux")]
use anyhow::{Context as _, Result};
use headless_core::config::profile_store::{ProfileStore, SequenceKind};
use mihomo_server::core_manager::{CoreManager, CoreOptions, CorePhase};
use std::{fs, path::PathBuf};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-cascade-core-{}-{stamp:x}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
    fn options(&self) -> Result<CoreOptions> {
        let source = self.0.join("bootstrap.yaml");
        fs::write(
            &source,
            "mode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']",
        )?;
        let mut options = CoreOptions::new(
            std::env::var_os("MIHOMO_TEST_BINARY")
                .context("set MIHOMO_TEST_BINARY")?
                .into(),
            self.0.clone(),
            source,
        );
        options.script_worker = Some(PathBuf::from(env!("CARGO_BIN_EXE_mihomo-server")));
        Ok(options)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
#[ignore = "requires real Mihomo and script worker"]
async fn actor_cascades_linked_profile_and_dns_preferences_without_changing_running_core() -> Result<()> {
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        manager.start().await?;
        let yaml = "mode: direct\nmixed-port: 0\ndns: {enable: false, nameserver-policy: {example.org: 8.8.8.8}}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']";
        let a = manager
            .import_profile_yaml(yaml.into(), "a".into())
            .await?
            .uid
            .unwrap()
            .to_string();
        let b = manager
            .import_profile_yaml(yaml.into(), "b".into())
            .await?
            .uid
            .unwrap()
            .to_string();
        manager.select_profile(a.clone()).await?;
        manager.set_settings(serde_yaml_ng::from_str("dns: {enable: false, nameserver: [8.8.8.8]}")?).await?;
        let source = manager.profile_dns(a.clone()).await?.source;
        manager.set_profile_dns(a.clone(), true, source).await?;
        manager
            .set_profile_merge(a.clone(), Some("mode: direct".into()))
            .await?;
        manager
            .set_profile_script(a.clone(), Some("function main(c) { return c; }".into()))
            .await?;
        for kind in [SequenceKind::Rules, SequenceKind::Proxies, SequenceKind::Groups] {
            manager
                .set_profile_sequence(a.clone(), kind, Some("prepend: []\nappend: []\ndelete: []".into()))
                .await?;
        }
        let store = ProfileStore::open(&dir.0)?;
        let item = store.get_item(&a)?.clone();
        let option = item.option.as_ref().unwrap();
        let ids: Vec<_> = [
            &option.merge,
            &option.script,
            &option.rules,
            &option.proxies,
            &option.groups,
        ]
        .into_iter()
        .map(|uid| uid.as_deref().unwrap().to_owned())
        .collect();
        let mut files = vec![item.file.unwrap().to_string()];
        files.extend(
            ids.iter()
                .map(|uid| store.get_item(uid).unwrap().file.clone().unwrap().to_string()),
        );
        let settings = manager.settings().await?;
        assert!(manager.delete_profile(a.clone()).await.is_err());
        assert_eq!(manager.settings().await?, settings);
        manager.select_profile(b.clone()).await?;
        manager.set_profile_dns(b.clone(), false, None).await?;
        let before = manager.status();
        let runtime = manager.runtime_config().await?;
        manager.delete_profile(a.clone()).await?;
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(manager.runtime_config().await?, runtime);
        assert!(!manager.settings().await?.profile_dns.contains_key(&a));
        assert!(manager.settings().await?.profile_dns.contains_key(&b));
        let store = ProfileStore::open(&dir.0)?;
        for uid in &ids {
            assert!(store.get_item(uid).is_err());
        }
        for file in &files {
            assert!(!dir.0.join("profiles").join(file).exists());
        }
        assert!(store.get_item("Merge").is_ok());
        assert!(store.get_item("Script").is_ok());
        Ok::<_, anyhow::Error>((a, b, runtime))
    }
    .await;
    let cleanup = manager.shutdown().await;
    let (a, b, runtime) = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(restored.runtime_config().await?, runtime);
        assert!(!restored.settings().await?.profile_dns.contains_key(&a));
        assert!(restored.settings().await?.profile_dns.contains_key(&b));
        assert!(restored.delete_profile(b).await.is_err());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
#[ignore = "requires real Mihomo"]
async fn actor_retries_post_commit_settings_cleanup_before_next_command_and_restart() -> Result<()> {
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        let yaml = "mode: direct\nmixed-port: 0\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']";
        let a = manager
            .import_profile_yaml(yaml.into(), "a".into())
            .await?
            .uid
            .unwrap()
            .to_string();
        let b = manager
            .import_profile_yaml(yaml.into(), "b".into())
            .await?
            .uid
            .unwrap()
            .to_string();
        manager.select_profile(a.clone()).await?;
        manager.set_profile_dns(a.clone(), false, None).await?;
        manager
            .set_profile_merge(a.clone(), Some("mode: direct".into()))
            .await?;
        let raw = manager.profile_raw(a.clone()).await?;
        manager.select_profile(b.clone()).await?;
        let before = manager.status();
        fs::rename(dir.0.join("settings.yaml"), dir.0.join("saved-settings.yaml"))?;
        fs::create_dir(dir.0.join("settings.yaml"))?;
        assert!(manager.delete_profile(a.clone()).await.is_err());
        assert!(ProfileStore::open(&dir.0)?.get_item(&a).is_err());
        assert!(dir.0.join("profile-delete.yaml").is_file());
        assert!(dir.0.join("profiles").join(&raw.revision).is_file());
        assert!(manager.settings().await.is_err());
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.status().config_revision, before.config_revision);
        fs::remove_dir(dir.0.join("settings.yaml"))?;
        fs::rename(dir.0.join("saved-settings.yaml"), dir.0.join("settings.yaml"))?;
        assert!(!manager.settings().await?.profile_dns.contains_key(&a));
        assert!(!dir.0.join("profile-delete.yaml").exists());
        assert!(!dir.0.join("profiles").join(&raw.revision).exists());
        Ok::<_, anyhow::Error>(a)
    }
    .await;
    let cleanup = manager.shutdown().await;
    let a = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        assert!(!restored.settings().await?.profile_dns.contains_key(&a));
        assert_eq!(restored.status().phase, CorePhase::Stopped);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}
