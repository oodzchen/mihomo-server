#![cfg(target_os = "linux")]
use anyhow::{Context as _, Result};
use mihomo_server::core_manager::{CoreManager, CoreOptions};
use std::{fs, path::PathBuf};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-enh-core-{}-{stamp:x}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
    fn options(&self) -> Result<CoreOptions> {
        let source = self.0.join("bootstrap.yaml");
        fs::write(
            &source,
            "mode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']\n",
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
async fn enhancement_resource_transaction_rejects_invalid_providers_and_preserves_state() -> Result<()> {
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        manager.start().await?;
        let raw = "mode: direct\nmixed-port: 0\ndns: {enable: false}\nrules: ['MATCH,DIRECT']\n";
        let a = manager.import_profile_yaml(raw.into(), "active".into()).await?.uid.unwrap().to_string();
        let b = manager.import_profile_yaml(raw.into(), "inactive".into()).await?.uid.unwrap().to_string();
        manager.select_profile(a.clone()).await?;

        let initial_status = manager.status();
        let initial_config = manager.runtime_config().await?;

        // 1. Invalid provider type in profile merge
        let bad_type = "proxy-providers:\n  p1:\n    type: ftp\n    url: 'ftp://example.com'\n";
        assert!(manager.set_profile_merge(a.clone(), Some(bad_type.into())).await.is_err());
        assert_eq!(manager.status().config_revision, initial_status.config_revision);
        assert_eq!(manager.runtime_config().await?, initial_config);

        // 2. Invalid interval (> 2^31-1) in profile merge
        let bad_interval = "proxy-providers:\n  p1:\n    type: http\n    url: 'https://example.com'\n    interval: 3000000000\n";
        assert!(manager.set_profile_merge(a.clone(), Some(bad_interval.into())).await.is_err());
        assert_eq!(manager.status().config_revision, initial_status.config_revision);
        assert_eq!(manager.runtime_config().await?, initial_config);

        // 3. Geo asset collision path
        let geo_collision = "proxy-providers:\n  p1:\n    type: file\n    path: 'GeoIP.dat'\n";
        assert!(manager.set_profile_merge(a.clone(), Some(geo_collision.into())).await.is_err());
        assert_eq!(manager.status().config_revision, initial_status.config_revision);
        assert_eq!(manager.runtime_config().await?, initial_config);

        // 4. Inactive profile enhancement validation
        let bad_inactive = "proxy-providers:\n  p1:\n    type: unknown_type\n";
        assert!(manager.set_profile_merge(b.clone(), Some(bad_inactive.into())).await.is_err());

        // 5. Valid profile merge succeeds and allocates provider cache path
        let valid_merge = "proxy-providers:\n  my_provider:\n    type: http\n    url: 'https://example.com/subs.yaml'\n    interval: 3600\n";
        let updated = manager.set_profile_merge(a.clone(), Some(valid_merge.into())).await?;
        assert_eq!(updated.uid.as_deref(), Some(a.as_str()));

        let active_config = manager.runtime_config().await?;
        let provider = &active_config["proxy-providers"]["my_provider"];
        let path = provider["path"].as_str().expect("provider path should be assigned");
        assert!(path.starts_with("provider-cache/v1/"));
        assert_eq!(provider["url"].as_str(), Some("https://example.com/subs.yaml"));

        // 6. Global merge validation
        let bad_global = "rule-providers:\n  bad_rule:\n    type: file\n    behavior: invalid_behavior\n";
        assert!(manager.set_global_merge(Some(bad_global.into())).await.is_err());

        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    result?;
    cleanup
}
