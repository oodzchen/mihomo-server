#![cfg(target_os = "linux")]
use anyhow::{Context as _, Result};
use headless_core::config::profile_store::ProfileStore;
use mihomo_server::core_manager::{CoreManager, CoreOptions, CorePhase};
use std::{fs, path::PathBuf};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-raw-core-{}-{stamp:x}", std::process::id()));
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
#[ignore = "requires real Mihomo and bounded script worker"]
async fn active_and_inactive_raw_edits_validate_preserve_metadata_and_reject_stale_drafts() -> Result<()> {
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result=async{
        manager.start().await?;
        let raw="# original\nproxies: []\nmode: direct\nmixed-port: 0\nallow-lan: false\nbind-address: localhost\ndns: {enable: false}\nrules: ['MATCH,DIRECT']\n";
        let a=manager.import_profile_yaml(raw.into(),"a".into()).await?.uid.unwrap().to_string();
        let b=manager.import_profile_yaml(raw.into(),"b".into()).await?.uid.unwrap().to_string();
        manager.select_profile(a.clone()).await?;
        manager.set_settings(serde_yaml_ng::from_str("allow-lan: true\nmixed-port: 0")?).await?;
        manager.set_profile_script(a.clone(),Some("function main(c) { c['raw-mode']=c.mode; if(c.mode==='global')c.rules=['INVALID,DIRECT'];return c; }".into())).await?;
        let before=manager.status();let original=manager.profile_raw(a.clone()).await?;
        // A valid raw config can still fail the enhanced runtime validation.
        assert!(manager.set_profile_raw(a.clone(),original.revision.clone(),raw.replace("mode: direct","mode: global")).await.is_err());
        assert_eq!(manager.profile_raw(a.clone()).await?,original);assert_eq!(manager.status().pid,before.pid);assert_eq!(manager.status().config_revision,before.config_revision);
        // Conversely, manual enhancements must not hide invalid original YAML.
        assert!(manager.set_profile_raw(a.clone(),original.revision.clone(),raw.replace("MATCH,DIRECT","INVALID,DIRECT")).await.is_err());
        assert_eq!(manager.profile_raw(a.clone()).await?,original);
        let edited=raw.replace("# original","# exact edit").replace("mode: direct","mode: rule");
        let accepted=manager.set_profile_raw(a.clone(),original.revision.clone(),edited.clone()).await?;
        assert_eq!(accepted.yaml,edited);assert_ne!(accepted.revision,original.revision);
        assert_eq!(fs::read_to_string(dir.0.join("profiles").join(&original.revision))?,raw);
        let config=manager.runtime_config().await?;
        assert_eq!(config["raw-mode"].as_str(),Some("rule"));assert_eq!(config["bind-address"].as_str(),Some("*"));
        assert_eq!(manager.status().active_profile.as_deref(),Some(a.as_str()));
        assert!(manager.set_profile_raw(a.clone(),original.revision,"mode: direct".into()).await.is_err());
        assert_eq!(manager.profile_raw(a.clone()).await?,accepted);
        let prior=manager.status();let inactive=manager.profile_raw(b.clone()).await?;
        assert!(manager.set_profile_raw(b.clone(),inactive.revision.clone(),raw.replace("MATCH,DIRECT","INVALID,DIRECT")).await.is_err());
        assert_eq!(manager.profile_raw(b.clone()).await?,inactive);
        let saved=manager.set_profile_raw(b.clone(),inactive.revision,raw.replace("# original","# inactive")).await?;
        assert!(saved.yaml.contains("# inactive"));assert_eq!(manager.status().pid,prior.pid);assert_eq!(manager.status().config_revision,prior.config_revision);
        assert_eq!(manager.runtime_config().await?,config);
        manager.stop().await?;
        let saved=manager.set_profile_raw(a.clone(),accepted.revision,edited.replace("# exact edit","# stopped")).await?;
        assert_eq!(manager.status().phase,CorePhase::Stopped);assert!(manager.status().pid.is_none());
        Ok::<_,anyhow::Error>((saved,manager.runtime_config().await?))
    }.await;
    let cleanup = manager.shutdown().await;
    let (saved, committed) = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(restored.profile_raw(saved.uid.clone()).await?, saved);
        assert_eq!(restored.runtime_config().await?, committed);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
#[ignore = "requires real Mihomo"]
async fn raw_dns_source_edit_coordinates_preference_disable_and_failed_publication_recovery() -> Result<()> {
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result=async{
        manager.start().await?;
        manager.set_settings(serde_yaml_ng::from_str("dns: {nameserver: [1.1.1.1]}\nmixed-port: 0")?).await?;
        let raw="proxies: []\nmode: direct\nmixed-port: 0\ndns: {enable: false, nameserver: [9.9.9.9], nameserver-policy: {example.org: 8.8.8.8}}\nrules: ['MATCH,DIRECT']";
        let uid=manager.import_profile_yaml(raw.into(),"provider".into()).await?.uid.unwrap().to_string();manager.select_profile(uid.clone()).await?;
        let source=manager.profile_dns(uid.clone()).await?.source.unwrap();manager.set_profile_dns(uid.clone(),true,Some(source)).await?;
        let content=manager.profile_raw(uid.clone()).await?;let prior=manager.status();let previous=manager.runtime_config().await?;
        let settings=dir.0.join("settings.yaml");let bytes=fs::read(&settings)?;
        fs::remove_file(&settings)?;fs::create_dir(&settings)?;
        let edited=raw.replace("8.8.8.8","8.8.4.4");
        assert!(manager.set_profile_raw(uid.clone(),content.revision.clone(),edited.clone()).await.is_err());
        assert_eq!(manager.status().config_revision,prior.config_revision);
        assert_eq!(ProfileStore::open(&dir.0)?.read_raw(&uid)?,content);
        assert!(dir.0.join("settings-transaction.yaml").exists());
        fs::remove_dir(&settings)?;fs::write(&settings,bytes)?;
        assert_eq!(manager.profile_raw(uid.clone()).await?,content);assert!(manager.profile_dns(uid.clone()).await?.enabled);
        assert_eq!(manager.runtime_config().await?,previous);
        let saved=manager.set_profile_raw(uid.clone(),content.revision.clone(),edited.clone()).await?;
        assert_eq!(saved.yaml,edited);assert!(!manager.settings().await?.profile_dns[&uid].enabled);
        assert!(!manager.profile_dns(uid.clone()).await?.enabled);
        assert_eq!(manager.runtime_config().await?["dns"]["nameserver"][0].as_str(),Some("9.9.9.9"));
        assert!(!dir.0.join("settings-transaction.yaml").exists());assert!(!dir.0.join("profile-refresh.yaml").exists());
        assert!(manager.set_profile_raw(uid,content.revision,raw.into()).await.is_err());
        Ok::<_,anyhow::Error>(())
    }.await;
    let cleanup = manager.shutdown().await;
    result?;
    cleanup
}
