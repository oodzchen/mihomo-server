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
        let path = std::env::temp_dir().join(format!("ms-import-core-{}-{stamp:x}", std::process::id()));
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
async fn service_import_defaults_preserve_running_core_and_execute_global_script_once() -> Result<()> {
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result=async {
        manager.start().await?;
        let before=manager.status();let runtime=manager.runtime_config().await?;
        let yaml="# raw unchanged\nmode: direct\nmixed-port: 0\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']";
        let imported=manager.import_profile_yaml(yaml.into(),"defaults".into()).await?;
        let uid=imported.uid.as_deref().unwrap().to_owned();let op=imported.option.as_ref().unwrap();
        for link in [&op.merge,&op.script,&op.rules,&op.proxies,&op.groups] {assert!(link.is_some());}
        assert_eq!(manager.profiles().items.unwrap().len(),8);
        assert_eq!(manager.runtime_config().await?,runtime);assert_eq!(manager.status().pid,before.pid);assert_eq!(manager.status().config_revision,before.config_revision);assert_eq!(manager.status().active_profile,before.active_profile);
        manager.set_global_script(Some("function main(c) { c['global-count']=(c['global-count']||0)+1; return c; }".into())).await?;
        manager.select_profile(uid.clone()).await?;
        assert_eq!(manager.runtime_config().await?["global-count"].as_u64(),Some(1));
        assert_eq!(manager.profile_raw(uid.clone()).await?.yaml,yaml);
        manager.stop().await?;
        manager.set_profile_script(uid.clone(),None).await?;
        assert_eq!(manager.runtime_config().await?["global-count"].as_u64(),Some(2));
        assert_eq!(manager.status().phase,CorePhase::Stopped);
        let committed=manager.runtime_config().await?;
        Ok::<_,anyhow::Error>((uid,committed))
    }.await;
    let cleanup = manager.shutdown().await;
    let (uid, committed) = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(restored.runtime_config().await?, committed);
        assert!(restored.profile_merge(uid.clone()).await?.uid.is_some());
        assert!(restored.profile_script(uid).await?.uid.is_none());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
#[ignore = "requires real Mihomo"]
async fn service_startup_recovers_import_before_global_defaults_and_command_admission() -> Result<()> {
    for published in [false, true] {
        let dir = Directory::new()?;
        let mut store = ProfileStore::open(&dir.0)?;
        let yaml = "mode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']";
        let plan = store.prepare_local_import("interrupted", yaml)?;
        let uid = plan.profile().uid.as_deref().unwrap().to_owned();
        store.begin_import(plan)?;
        if published {
            store.publish_import()?;
        }
        drop(store);
        let manager = CoreManager::spawn(dir.options()?)?;
        let result = async {
            assert!(!dir.0.join("profile-import.yaml").exists());
            assert_eq!(manager.profiles().items.unwrap().len(), if published { 8 } else { 2 });
            assert_eq!(manager.profile_raw(uid.clone()).await.is_ok(), published);
            if published {
                manager.select_profile(uid.clone()).await?;
            }
            manager.start().await?;
            assert_eq!(manager.status().phase, CorePhase::Running);
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = manager.shutdown().await;
        result?;
        cleanup?;
    }
    Ok(())
}
