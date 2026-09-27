#![cfg(target_os = "linux")]
use anyhow::{Context as _, Result};
use headless_core::config::{
    runtime::parse,
    settings::{Mode, SettingsStore},
};
use mihomo_server::core_manager::{CoreManager, CoreOptions, CorePhase};
use std::{fs, path::PathBuf};

struct Directory(PathBuf);

#[tokio::test]
#[ignore = "requires real Mihomo and bounded script worker"]
async fn provider_dns_confirmation_is_scoped_rolls_back_and_expires_after_restart() -> Result<()> {
    use headless_core::config::dns::DnsOverrideOutcome;
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        manager.start().await?;
        manager.set_settings(serde_yaml_ng::from_str("dns: {nameserver: [1.1.1.1]}\ntun: {enable: false}")?).await?;
        let raw = "mode: direct\nmixed-port: 0\ndns: {enable: false, nameserver: [9.9.9.9], nameserver-policy: {example.org: 8.8.8.8}}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']";
        let a = manager.import_profile_yaml(raw.into(), "provider-a".into()).await?.uid.unwrap().to_string();
        let b = manager.import_profile_yaml(raw.into(), "provider-b".into()).await?.uid.unwrap().to_string();
        manager.select_profile(a.clone()).await?;
        assert!(!manager.settings().await?.profile_dns[&a].enabled);
        assert_eq!(manager.runtime_config().await?["dns"]["nameserver"], serde_yaml_ng::to_value(["9.9.9.9"])?);
        let source = manager.profile_dns(a.clone()).await?.source.unwrap();
        let other = manager.profile_dns(b.clone()).await?.source.unwrap();
        let before = manager.status();
        for confirmation in [None, Some("stale".into()), Some(other)] {
            assert!(matches!(manager.set_profile_dns(a.clone(), true, confirmation).await?, DnsOverrideOutcome::ConfirmationRequired { .. }));
            assert_eq!(manager.status().config_revision, before.config_revision);
        }
        manager.set_profile_script(a.clone(), Some("function main(c) { if(c.dns.nameserver[0]==='1.1.1.1') c.rules=['INVALID,DIRECT']; return c; }".into())).await?;
        let before = manager.status();
        let settings = manager.settings().await?;
        assert!(manager.set_profile_dns(a.clone(), true, Some(source.clone())).await.is_err());
        assert_eq!(manager.settings().await?, settings);
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert!(!manager.profile_dns(a.clone()).await?.enabled);
        manager.set_profile_script(a.clone(), None).await?;
        assert!(matches!(manager.set_profile_dns(a.clone(), true, Some(source.clone())).await?, DnsOverrideOutcome::Applied { state } if state.enabled));
        assert_eq!(manager.runtime_config().await?["dns"]["nameserver"], serde_yaml_ng::to_value(["1.1.1.1"])?);
        manager.select_profile(b.clone()).await?;
        assert!(!manager.settings().await?.profile_dns[&b].enabled);
        manager.select_profile(a.clone()).await?;
        assert!(manager.profile_dns(a.clone()).await?.enabled);
        manager.set_profile_dns(a.clone(), false, None).await?;
        assert!(matches!(manager.set_profile_dns(a.clone(), true, None).await?, DnsOverrideOutcome::ConfirmationRequired { .. }));
        manager.set_profile_dns(a.clone(), true, Some(source.clone())).await?;
        let saved = fs::read_to_string(dir.0.join("settings.yaml"))?;
        assert!(!saved.contains("confirmation"));
        assert!(!saved.contains(&source));
        assert!(manager.logs().iter().any(|l| l.stream == "settings" && l.message.contains("auto-disabled")));
        Ok::<_, anyhow::Error>((a, b, manager.runtime_config().await?))
    }.await;
    let cleanup = manager.shutdown().await;
    let (a, b, committed) = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(restored.runtime_config().await?, committed);
        assert!(restored.profile_dns(a.clone()).await?.requested);
        assert!(!restored.profile_dns(a.clone()).await?.enabled);
        restored.select_profile(a.clone()).await?;
        assert!(!restored.settings().await?.profile_dns[&a].enabled);
        assert!(!restored.settings().await?.profile_dns[&b].enabled);
        assert_eq!(
            restored.runtime_config().await?["dns"]["nameserver"],
            serde_yaml_ng::to_value(["9.9.9.9"])?
        );
        restored.set_settings(serde_yaml_ng::from_str("mode: direct")?).await?;
        assert!(!restored.settings().await?.profile_dns[&a].enabled);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
#[ignore = "requires real Mihomo and loopback subscription server"]
async fn provider_dns_refresh_commits_disable_with_raw_content_and_recovers_both_journals() -> Result<()> {
    use headless_core::config::dns::DnsOverrideOutcome;
    let body = std::sync::Arc::new(std::sync::RwLock::new("proxies: []\nmode: direct\nmixed-port: 0\ndns: {enable: false, nameserver: [9.9.9.9], nameserver-policy: {example.org: 8.8.8.8}}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']".to_owned()));
    let app = axum::Router::new().route("/profile", axum::routing::get(|axum::extract::State(body): axum::extract::State<std::sync::Arc<std::sync::RwLock<String>>>| async move { body.read().unwrap().clone() })).with_state(body.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/profile", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        manager.start().await?;
        manager
            .set_settings(serde_yaml_ng::from_str(
                "dns: {nameserver: [1.1.1.1]}\ntun: {enable: false}",
            )?)
            .await?;
        let profile = manager.import_remote_profile(url, None, Default::default()).await?;
        let uid = profile.uid.unwrap().to_string();
        let raw_path = dir.0.join("profiles").join(profile.file.unwrap().as_str());
        manager.select_profile(uid.clone()).await?;
        let source = manager.profile_dns(uid.clone()).await?.source.unwrap();
        manager.set_profile_dns(uid.clone(), true, Some(source.clone())).await?;
        let before = manager.status();
        let previous_raw = fs::read(&raw_path)?;
        let settings_path = dir.0.join("settings.yaml");
        let previous_settings = fs::read(&settings_path)?;
        {
            let mut content = body.write().unwrap();
            *content = content.replace("example.org: 8.8.8.8", "example.org: 8.8.4.4");
        }
        // Fail settings publication after refreshed raw content was published.
        fs::remove_file(&settings_path)?;
        fs::create_dir(&settings_path)?;
        assert!(manager.refresh_profile(uid.clone()).await.is_err());
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(fs::read(&raw_path)?, previous_raw);
        assert!(dir.0.join("settings-transaction.yaml").exists());
        assert!(manager.settings().await.is_err());
        fs::remove_dir(&settings_path)?;
        fs::write(&settings_path, previous_settings)?;
        assert!(manager.settings().await?.profile_dns[&uid].enabled);
        assert!(manager.profile_dns(uid.clone()).await?.enabled);
        assert!(!dir.0.join("settings-transaction.yaml").exists());
        let refreshed = manager.refresh_profile(uid.clone()).await?;
        assert!(!manager.settings().await?.profile_dns[&uid].enabled);
        let raw_path = dir.0.join("profiles").join(refreshed.file.unwrap().as_str());
        assert_eq!(
            parse(&fs::read_to_string(raw_path)?)?["dns"]["nameserver-policy"]["example.org"].as_str(),
            Some("8.8.4.4")
        );
        let changed = manager.profile_dns(uid.clone()).await?.source.unwrap();
        assert_ne!(source, changed);
        assert!(matches!(
            manager.set_profile_dns(uid.clone(), true, Some(source)).await?,
            DnsOverrideOutcome::ConfirmationRequired { .. }
        ));
        manager.set_profile_dns(uid.clone(), true, Some(changed)).await?;
        assert!(manager.profile_dns(uid).await?.enabled);
        assert_eq!(
            manager.runtime_config().await?["dns"]["nameserver"],
            serde_yaml_ng::to_value(["1.1.1.1"])?
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = manager.shutdown().await;
    server.abort();
    let _ = server.await;
    result?;
    cleanup
}

#[tokio::test]
#[ignore = "requires real Mihomo and bounded script worker; enables TUN only in stopped candidates"]
async fn tun_dns_derivation_runs_before_scripts_once_and_retains_committed_snapshots() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        manager.start().await?;
        manager.stop().await?;
        let runtime: RuntimeSettings = serde_yaml_ng::from_str("ipv6: true\ntun: {enable: true, auto-route: false}\ndns: {nameserver: [1.1.1.1], listen: '127.0.0.1:0'}")?;
        manager.set_settings(runtime.clone()).await?;
        let config = manager.runtime_config().await?;
        assert_eq!(config["dns"]["enable"].as_bool(), Some(true));
        assert_eq!(config["dns"]["fake-ip-range6"].as_str(), Some("2001:2::0/64"));
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        assert!(manager.status().pid.is_none());
        let raw = "mode: direct\nmixed-port: 0\nipv6: false\ndns: {enable: false, nameserver: [9.9.9.9]}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']";
        let profile = manager.import_profile_yaml(raw.into(), "tun-derivation".into()).await?;
        let uid = profile.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.set_global_script(Some("function main(c) { c['derived-enable']=c.dns.enable; c['derived-range6']=c.dns['fake-ip-range6']; return c; }".into())).await?;
        manager.set_profile_script(uid.clone(), Some("function main(c) { c.dns.enable=false; c.dns.ipv6=false; c.dns['fake-ip-range6']='2001:db8:2::/64'; c.tun.enable=false; return c; }".into())).await?;
        let config = manager.runtime_config().await?;
        assert_eq!(config["derived-enable"].as_bool(), Some(true));
        assert_eq!(config["derived-range6"].as_str(), Some("2001:2::0/64"));
        // Derived DNS fields are unowned; final staging must preserve scripts.
        assert_eq!(config["dns"]["enable"].as_bool(), Some(false));
        assert_eq!(config["dns"]["ipv6"].as_bool(), Some(false));
        assert_eq!(config["dns"]["fake-ip-range6"].as_str(), Some("2001:db8:2::/64"));
        assert_eq!(config["tun"]["enable"].as_bool(), Some(true));
        let before = manager.status();
        let saved = manager.settings().await?;
        let mut invalid = runtime.clone();
        invalid.dns.as_mut().unwrap().enable = Some(true);
        invalid.dns.as_mut().unwrap().nameserver = Some(vec!["https://[".into()]);
        assert!(manager.set_settings(invalid).await.is_err());
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.settings().await?, saved);
        assert_eq!(manager.runtime_config().await?, config);
        let file = manager.profiles().items.unwrap().into_iter().find(|p| p.uid.as_deref() == Some(&uid)).unwrap().file.unwrap();
        assert_eq!(fs::read_to_string(dir.0.join("profiles").join(file.as_str()))?, raw);
        assert!(!dir.0.join("settings-transaction.yaml").exists());
        Ok::<_, anyhow::Error>((runtime, config))
    }.await;
    let cleanup = manager.shutdown().await;
    let (mut runtime, committed) = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        // Do not start with enabled TUN: this verifies -t/generation, not native routing.
        assert_eq!(restored.runtime_config().await?, committed);
        assert_eq!(restored.settings().await?.runtime, runtime);
        runtime.tun.as_mut().unwrap().enable = Some(false);
        restored.set_settings(runtime).await?;
        assert_eq!(restored.status().phase, CorePhase::Stopped);
        restored.start().await?;
        assert_eq!(restored.status().phase, CorePhase::Running);
        assert_eq!(restored.runtime_config().await?["tun"]["enable"].as_bool(), Some(false));
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
#[ignore = "requires real Mihomo and bounded script worker"]
async fn network_settings_preserve_unowned_fields_enforce_every_entry_and_rollback_invalid_candidates() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        manager.start().await?;
        let runtime: RuntimeSettings = serde_yaml_ng::from_str("dns: {nameserver: [1.1.1.1], enhanced-mode: redir-host}\ntun: {enable: false, auto-route: false, mtu: 1500}")?;
        manager.set_settings(runtime.clone()).await?;
        let profile = manager.import_profile_yaml("mode: direct\nmixed-port: 0\ndns: {enable: false, nameserver: [9.9.9.9], use-hosts: true, fallback-filter: {geoip: false}}\ntun: {enable: false, mtu: 9000, auto-detect-interface: true}\nrules: ['MATCH,DIRECT']".into(), "network".into()).await?;
        let uid = profile.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.set_global_script(Some("function main(c) { c['entry-dns']=c.dns.nameserver[0]; c.dns.nameserver=['8.8.8.8']; c.tun.enable=true; return c; }".into())).await.context("network global script")?;
        manager.set_profile_merge(uid.clone(), Some("dns: {nameserver: [8.8.4.4]}\ntun: {mtu: 8000}".into())).await.context("network profile merge")?;
        manager.set_profile_script(uid.clone(), Some("function main(c) { delete c.dns.nameserver; delete c.tun.enable; c.dns.fallback=['8.8.4.4']; return c; }".into())).await.context("network profile script")?;
        let applied = manager.runtime_config().await?;
        assert_eq!(applied["entry-dns"].as_str(), Some("1.1.1.1"));
        assert_eq!(applied["dns"]["nameserver"], serde_yaml_ng::to_value(["1.1.1.1"])?);
        assert_eq!(applied["dns"]["use-hosts"].as_bool(), Some(true));
        assert_eq!(applied["dns"]["fallback"], serde_yaml_ng::to_value(["8.8.4.4"])?);
        assert_eq!(applied["tun"]["enable"].as_bool(), Some(false));
        assert_eq!(applied["tun"]["mtu"].as_u64(), Some(1500));
        assert_eq!(applied["tun"]["auto-detect-interface"].as_bool(), Some(true));
        for field in ["dns.nameserver", "tun.enable", "tun.mtu"] {
            assert!(manager.logs().iter().any(|l| l.stream == "settings" && l.message.contains(field)));
        }
        let before = manager.status();
        let saved = manager.settings().await?;
        // Mihomo semantic validation rejects malformed resolver URLs; no DNS/TUN
        // device is opened by this test, and no privilege/host routing changes occur.
        let mut invalid = runtime.clone();
        invalid.dns.as_mut().unwrap().nameserver = Some(vec!["https://[".into()]);
        invalid.dns.as_mut().unwrap().enable = Some(true);
        assert!(manager.set_settings(invalid).await.is_err());
        assert_eq!(manager.settings().await?, saved);
        assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), saved);
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(manager.runtime_config().await?, applied);
        manager.stop().await?;
        manager.apply_overlay(parse("dns: {nameserver: [8.8.8.8]}\ntun: {enable: true, mtu: 9000}")?).await.context("network stopped overlay")?;
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        assert_eq!(manager.runtime_config().await?["tun"]["enable"].as_bool(), Some(false));
        // Releasing authority while stopped regenerates the active subscription.
        manager.set_global_script(None).await?;
        manager.set_profile_script(uid.clone(), None).await?;
        manager.set_profile_merge(uid.clone(), None).await?;
        manager.select_profile(uid.clone()).await?;
        manager.set_settings(RuntimeSettings::default()).await?;
        assert_eq!(manager.runtime_config().await?["dns"]["nameserver"], serde_yaml_ng::to_value(["9.9.9.9"])?);
        assert_eq!(manager.runtime_config().await?["tun"]["mtu"].as_u64(), Some(9000));
        let saved = manager.set_settings(runtime).await?;
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        Ok::<_, anyhow::Error>(saved)
    }.await;
    let cleanup = manager.shutdown().await;
    let saved = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(restored.settings().await?, saved);
        assert_eq!(restored.runtime_config().await?["tun"]["mtu"].as_u64(), Some(1500));
        assert_eq!(
            restored.runtime_config().await?["dns"]["nameserver"],
            serde_yaml_ng::to_value(["1.1.1.1"])?
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
#[ignore = "requires real Mihomo and bounded script worker"]
async fn online_settings_apply_regenerate_rollback_and_persist_as_one_runtime_transaction() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        manager.start().await?;
        let mut runtime = RuntimeSettings {
            mode: Some(Mode::Global),
            mixed_port: Some(0),
            ..Default::default()
        };
        let saved = manager.set_settings(runtime.clone()).await?;
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("global"));
        assert_eq!(manager.status().phase, CorePhase::Running);
        assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), saved);
        let profile = manager
            .import_profile_yaml(
                "mode: direct\nmixed-port: 0\nrules: ['MATCH,DIRECT']".into(),
                "online".into(),
            )
            .await?;
        let uid = profile.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager
            .set_profile_script(
                uid.clone(),
                Some("function main(c) { c['settings-input']=c.mode; c.mode='direct'; return c; }".into()),
            )
            .await?;
        runtime.mode = Some(Mode::Rule);
        manager.set_settings(runtime.clone()).await?;
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("rule"));
        assert_eq!(manager.runtime_config().await?["settings-input"].as_str(), Some("rule"));
        manager.set_settings(RuntimeSettings::default()).await?;
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("direct"));
        manager
            .set_profile_script(
                uid.clone(),
                Some("function main(c) { if(c.mode==='rule') c.rules=['INVALID,DIRECT']; return c; }".into()),
            )
            .await?;
        let validation_prior = manager.status();
        assert!(manager.set_settings(runtime.clone()).await.is_err());
        assert_eq!(manager.settings().await?.runtime, RuntimeSettings::default());
        assert_eq!(manager.status().pid, validation_prior.pid);
        assert_eq!(manager.status().config_revision, validation_prior.config_revision);
        manager
            .set_profile_script(
                uid.clone(),
                Some("function main(c) { c['settings-input']=c.mode; c.mode='direct'; return c; }".into()),
            )
            .await?;
        // Generation failure cannot publish settings or advance runtime/PID.
        let catalog = manager.profiles();
        let script_uid = catalog
            .items
            .as_ref()
            .unwrap()
            .iter()
            .find(|p| p.uid.as_deref() == Some(&uid))
            .unwrap()
            .option
            .as_ref()
            .unwrap()
            .script
            .as_ref()
            .unwrap();
        let script_file = catalog
            .items
            .as_ref()
            .unwrap()
            .iter()
            .find(|p| p.uid.as_ref() == Some(script_uid))
            .unwrap()
            .file
            .as_ref()
            .unwrap();
        let script_path = dir.0.join("profiles").join(script_file.as_str());
        let original = fs::read_to_string(&script_path)?;
        fs::write(&script_path, "function main(c) { throw 'settings execution failure'; }")?;
        let prior = manager.settings().await?;
        let status = manager.status();
        assert!(manager.set_settings(runtime.clone()).await.is_err());
        assert_eq!(manager.settings().await?, prior);
        assert_eq!(manager.status().config_revision, status.config_revision);
        assert_eq!(manager.status().pid, status.pid);
        fs::write(script_path, original)?;
        // Settings publication failure after core apply rolls runtime back.
        let manifest = dir.0.join("config/state.yaml");
        let manifest_bytes = fs::read(&manifest)?;
        // Force publication failure while keeping generation and validation valid.
        let settings_path = dir.0.join("settings.yaml");
        fs::remove_file(&settings_path)?;
        fs::create_dir(&settings_path)?;
        assert!(manager.set_settings(runtime.clone()).await.is_err());
        assert!(manager.settings().await.is_err());
        assert_eq!(manager.status().config_revision, status.config_revision);
        assert_eq!(fs::read(&manifest)?, manifest_bytes);
        assert!(dir.0.join("settings-transaction.yaml").exists());
        fs::remove_dir(&settings_path)?;
        fs::write(&settings_path, serde_yaml_ng::to_string(&prior)?)?;
        assert_eq!(manager.settings().await?, prior);
        assert!(!dir.0.join("settings-transaction.yaml").exists());
        manager.stop().await?;
        let accepted = manager.set_settings(runtime).await?;
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        assert_eq!(manager.status().active_profile.as_deref(), Some(uid.as_str()));
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("rule"));
        assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), accepted);
        Ok::<_, anyhow::Error>(accepted)
    }
    .await;
    let cleanup = manager.shutdown().await;
    let saved = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(restored.settings().await?, saved);
        assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("rule"));
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
async fn startup_repairs_settings_publication_using_the_committed_runtime_revision() -> Result<()> {
    use headless_core::config::runtime::RuntimeStore;
    for committed in [false, true] {
        let dir = Directory::new()?;
        let mut runtime = RuntimeStore::open(&dir.0)?;
        let prior = runtime.stage(parse("mode: direct")?)?;
        runtime.begin(prior)?;
        runtime.commit()?;
        let mut settings = SettingsStore::open(&dir.0)?;
        let old = settings.snapshot();
        let mut new = old.clone();
        new.runtime.mode = Some(Mode::Global);
        let revision = runtime.stage(parse("mode: global")?)?;
        runtime.begin(revision.clone())?;
        settings.begin(new.clone(), revision)?;
        if committed {
            runtime.commit()?;
        } else {
            settings.publish()?;
        }
        drop(runtime);
        drop(settings);
        let manager = CoreManager::spawn(CoreOptions::new(
            "missing".into(),
            dir.0.clone(),
            dir.0.join("missing.yaml"),
        ))?;
        let result = async {
            assert_eq!(manager.settings().await?, if committed { new } else { old });
            assert_eq!(
                manager.runtime_config().await?["mode"].as_str(),
                Some(if committed { "global" } else { "direct" })
            );
            assert!(!dir.0.join("settings-transaction.yaml").exists());
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = manager.shutdown().await;
        result?;
        cleanup?;
    }
    Ok(())
}
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-settings-core-{}-{stamp:x}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
    fn options(&self) -> Result<CoreOptions> {
        let source = self.0.join("bootstrap.yaml");
        fs::write(
            &source,
            "mode: direct\nmixed-port: 0\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']",
        )?;
        let binary = std::env::var_os("MIHOMO_TEST_BINARY").context("set MIHOMO_TEST_BINARY")?;
        let mut options = CoreOptions::new(binary.into(), self.0.clone(), source);
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
async fn malformed_settings_fail_before_actor_spawn_and_release_directory_lock() -> Result<()> {
    let dir = Directory::new()?;
    let source = dir.0.join("missing.yaml");
    fs::write(dir.0.join("settings.yaml"), "schema_version: 999")?;
    assert!(CoreManager::spawn(CoreOptions::new("missing".into(), dir.0.clone(), source.clone())).is_err());
    fs::remove_file(dir.0.join("settings.yaml"))?;
    let manager = CoreManager::spawn(CoreOptions::new("missing".into(), dir.0.clone(), source))?;
    manager.shutdown().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires real Mihomo and bounded script worker"]
async fn settings_control_bootstrap_all_enhancements_manual_edits_and_restore_committed_snapshot() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = SettingsStore::open(&dir.0)?;
    let mut settings = store.snapshot();
    settings.runtime.mode = Some(Mode::Global);
    settings.runtime.mixed_port = Some(0);
    settings.runtime.allow_lan = Some(false);
    store.replace(settings.clone())?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        assert_eq!(manager.start().await?.phase, CorePhase::Running);
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("global"));
        let profile = manager.import_profile_yaml("mode: direct\nmixed-port: 1\nrules: ['MATCH,DIRECT']\ndns: {enable: false}".into(), "settings-base".into()).await?;
        let uid = profile.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.set_global_script(Some("function main(c) { c['settings-entry']=c.mode; return c; }".into())).await?;
        assert_eq!(manager.runtime_config().await?["settings-entry"].as_str(), Some("global"));
        manager.set_global_merge(Some("mode: rule\nallow-lan: true\ncustom: merge".into())).await?;
        manager.set_global_script(Some("function main(c) { c['stage-mode']=c.mode; c.mode='direct'; c['mixed-port']=1; return c; }".into())).await?;
        manager.set_profile_merge(uid.clone(), Some("mode: rule\ncustom: profile".into())).await?;
        manager.set_profile_script(uid.clone(), Some("function main(c) { delete c.mode; delete c['allow-lan']; c['mixed-port']=1; c.custom='script'; return c; }".into())).await?;
        let config = manager.runtime_config().await?;
        assert_eq!(config["mode"].as_str(), Some("global"));
        assert_eq!(config["mixed-port"].as_u64(), Some(0));
        assert_eq!(config["allow-lan"].as_bool(), Some(false));
        assert_eq!(config["custom"].as_str(), Some("script"));
        assert_eq!(config["stage-mode"].as_str(), Some("rule"));
        let raw_file = manager.profiles().items.unwrap().into_iter().find(|p| p.uid.as_deref()==Some(&uid)).unwrap().file.unwrap().to_string();
        assert!(fs::read_to_string(dir.0.join("profiles").join(raw_file))?.contains("mixed-port: 1"));
        assert!(manager.logs().iter().any(|l| l.stream == "settings" && l.message.contains("mode")));
        let old = manager.status();
        assert!(manager.set_profile_merge(uid.clone(), Some("rules: ['INVALID,DIRECT']".into())).await.is_err());
        assert_eq!(manager.status().pid, old.pid);
        assert_eq!(manager.status().config_revision, old.config_revision);
        manager.stop().await?;
        manager.apply_overlay(parse("MODE: direct\nallow-lan: true\ncustom: stopped")?).await?;
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        assert_eq!(manager.runtime_config().await?["mode"].as_str(), Some("global"));
        assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), settings);
        Ok::<_, anyhow::Error>(uid)
    }.await;
    let cleanup = manager.shutdown().await;
    let uid = result?;
    cleanup?;
    // Offline edits become generation inputs, never silently regenerate committed
    // startup snapshots or rerun scripts during recovery.
    settings.runtime.mode = Some(Mode::Rule);
    store.replace(settings)?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("global"));
        restored.select_profile(uid).await?;
        assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("rule"));
        restored.apply_config(parse("mode: direct")?).await?;
        assert_eq!(restored.runtime_config().await?["mode"].as_str(), Some("rule"));
        assert!(restored.status().active_profile.is_none());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
#[ignore = "requires real Mihomo"]
async fn finalization_covers_bootstrap_standalone_overlay_and_committed_recovery() -> Result<()> {
    let dir = Directory::new()?;
    let mut options = dir.options()?;
    let raw = "mode: direct\nmixed-port: 0\nallow-lan: true\nbind-address: '127.1'\nproxy-groups: [{name: choose, type: select, proxies: [ghost, DIRECT]}]\nrules: ['MATCH,choose']\ndns: {enable: false}";
    options.config = dir.0.join("final-bootstrap.yaml");
    fs::write(&options.config, raw)?;
    let manager = CoreManager::spawn(options)?;
    let result = async {
        manager.start().await?;
        let config = manager.runtime_config().await?;
        assert_eq!(config["bind-address"].as_str(), Some("*"));
        assert_eq!(
            config["proxy-groups"][0]["proxies"],
            serde_yaml_ng::to_value(["DIRECT"])?
        );
        assert_eq!(fs::read_to_string(dir.0.join("final-bootstrap.yaml"))?, raw);
        manager
            .apply_config(parse(
                &raw.replace("allow-lan: true", "allow-lan: false")
                    .replace("127.1", "127.0.0.1"),
            )?)
            .await?;
        assert_eq!(
            manager.runtime_config().await?["bind-address"].as_str(),
            Some("127.0.0.1")
        );
        manager
            .set_settings(serde_yaml_ng::from_str("allow-lan: true")?)
            .await?;
        assert_eq!(manager.runtime_config().await?["bind-address"].as_str(), Some("*"));
        manager
            .apply_overlay(parse(
                "bind-address: '[::1]'\nproxy-groups: [{name: choose, type: select, proxies: [new-ghost, DIRECT]}]",
            )?)
            .await?;
        let config = manager.runtime_config().await?;
        assert_eq!(config["bind-address"].as_str(), Some("*"));
        assert_eq!(
            config["proxy-groups"][0]["proxies"],
            serde_yaml_ng::to_value(["DIRECT"])?
        );
        let before = manager.status();
        // Cleanup intentionally preserves malformed shapes for the core to reject.
        assert!(
            manager
                .apply_overlay(parse("proxy-groups: [{name: choose, type: select, proxies: invalid}]")?)
                .await
                .is_err()
        );
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(manager.runtime_config().await?, config);
        manager.stop().await?;
        manager.apply_overlay(parse("bind-address: localhost")?).await?;
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        assert!(manager.status().pid.is_none());
        let committed = manager.runtime_config().await?;
        assert_eq!(committed["bind-address"].as_str(), Some("*"));
        Ok::<_, anyhow::Error>(committed)
    }
    .await;
    let cleanup = manager.shutdown().await;
    let committed = result?;
    cleanup?;
    // Offline settings changes must not refinalize an already committed snapshot.
    let mut settings = SettingsStore::open(&dir.0)?;
    let mut next = settings.snapshot();
    next.runtime.allow_lan = Some(false);
    settings.replace(next)?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(
            serde_yaml_ng::to_string(&restored.runtime_config().await?)?,
            serde_yaml_ng::to_string(&committed)?
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
#[ignore = "requires real Mihomo and bounded script worker"]
async fn finalization_waits_for_all_scripts_and_restored_settings_authority() -> Result<()> {
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let result = async {
        manager.start().await?;
        manager.set_settings(serde_yaml_ng::from_str("allow-lan: true\nmixed-port: 0")?).await?;
        let raw = "mode: rule\nmixed-port: 0\nallow-lan: false\nbind-address: localhost\nproxies: []\nproxy-groups: [{name: choose, type: select, proxies: [late, ghost, DIRECT]}]\nrules: ['MATCH,choose']\ndns: {enable: false}";
        let profile = manager.import_profile_yaml(raw.into(), "final-order".into()).await?;
        let uid = profile.uid.unwrap().to_string();
        let raw_path = dir.0.join("profiles").join(profile.file.unwrap().as_str());
        manager.select_profile(uid.clone()).await?;
        assert_eq!(manager.runtime_config().await?["proxy-groups"][0]["proxies"], serde_yaml_ng::to_value(["DIRECT"])?);
        manager.set_global_script(Some("function main(c) { c['global-saw-late']=c['proxy-groups'][0].proxies.includes('late'); c['global-saw-ghost']=c['proxy-groups'][0].proxies.includes('ghost'); return c; }".into())).await?;
        manager.set_profile_script(uid.clone(), Some("function main(c) { c['profile-saw-late']=c['proxy-groups'][0].proxies.includes('late'); c.proxies=[{name:'late',type:'direct'}]; c['allow-lan']=false; c['bind-address']='[::1]'; return c; }".into())).await?;
        let config = manager.runtime_config().await?;
        for key in ["global-saw-late", "global-saw-ghost", "profile-saw-late"] { assert_eq!(config[key].as_bool(), Some(true), "{key}"); }
        assert_eq!(config["allow-lan"].as_bool(), Some(true));
        assert_eq!(config["bind-address"].as_str(), Some("*"));
        assert_eq!(config["proxy-groups"][0]["proxies"], serde_yaml_ng::to_value(["late", "DIRECT"])?);
        assert_eq!(fs::read_to_string(&raw_path)?, raw);
        let keys: Vec<_> = config.keys().filter_map(serde_yaml_ng::Value::as_str).collect();
        assert_eq!(&keys[keys.len()-3..], ["proxies", "proxy-groups", "rules"]);
        // Cleanup removes dangling references but leaves empty groups for -t validation.
        let prior = manager.status();
        let previous_script = manager.profile_script(uid.clone()).await?;
        assert!(manager.set_profile_script(uid.clone(), Some("function main(c) { c['proxy-groups'][0].proxies='invalid'; return c; }".into())).await.is_err());
        assert_eq!(manager.status().pid, prior.pid);
        assert_eq!(manager.status().config_revision, prior.config_revision);
        let unchanged = manager.profile_script(uid.clone()).await?;
        assert_eq!(unchanged.uid, previous_script.uid);
        assert_eq!(unchanged.source, previous_script.source);
        assert_eq!(manager.runtime_config().await?, config);
        manager.stop().await?;
        manager.set_settings(serde_yaml_ng::from_str("allow-lan: false\nmixed-port: 0")?).await?;
        let config = manager.runtime_config().await?;
        assert_eq!(config["allow-lan"].as_bool(), Some(false));
        assert_eq!(config["bind-address"].as_str(), Some("[::1]"));
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        manager.set_settings(serde_yaml_ng::from_str("allow-lan: true\nmixed-port: 0")?).await?;
        let committed = manager.runtime_config().await?;
        // This script would fail if recovery reran the pipeline.
        let item = manager.profiles().items.unwrap().into_iter().find(|p| p.uid.as_deref()==Some(uid.as_str())).unwrap();
        let script_uid = item.option.unwrap().script.unwrap();
        let file = manager.profiles().items.unwrap().into_iter().find(|p| p.uid.as_deref()==Some(script_uid.as_str())).unwrap().file.unwrap();
        fs::write(dir.0.join("profiles").join(file.as_str()), "function main(c) { throw Error('must not rerun'); }")?;
        Ok::<_, anyhow::Error>(committed)
    }.await;
    let cleanup = manager.shutdown().await;
    let committed = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(
            serde_yaml_ng::to_string(&restored.runtime_config().await?)?,
            serde_yaml_ng::to_string(&committed)?
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result?;
    cleanup
}

#[tokio::test]
#[ignore = "requires real Mihomo and bounded script worker"]
async fn geo_settings_enforce_scripts_rollback_failed_probe_and_survive_service_restart() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let runtime: RuntimeSettings = serde_yaml_ng::from_str(
        "geodata-mode: false\ngeodata-loader: standard\ngeosite-matcher: mph\ngeo-auto-update: false\ngeo-update-interval: 48\ngeox-url: {geoip: 'http://127.0.0.1:1/ip', geosite: 'http://127.0.0.1:1/site', mmdb: 'http://127.0.0.1:1/db', asn: 'http://127.0.0.1:1/asn'}",
    )?;
    let result = async {
        manager.set_settings(runtime.clone()).await?;
        let source = "mode: direct\ngeosite-matcher: succinct\nmixed-port: 0\ngeodata-mode: false\ngeo-auto-update: false\ngeo-update-interval: 24\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']";
        let profile = manager.import_profile_yaml(source.into(), "geo".into()).await?;
        let uid = profile.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.set_profile_script(uid.clone(), Some("function main(c) { c['geo-input']=c['geodata-mode']; c['matcher-input']=c['geosite-matcher']; c['geosite-matcher']='succinct'; if(c['geo-update-interval']===49) c.rules=['INVALID,DIRECT']; c['geodata-mode']=true; c['geo-auto-update']=true; c['geox-url']=c['geox-url']||{}; c['geox-url'].mmdb='http://127.0.0.1:1/script'; return c; }".into())).await?;
        let config = manager.runtime_config().await?;
        assert_eq!(config["geo-input"].as_bool(), Some(false));
        assert_eq!(config["matcher-input"].as_str(), Some("mph"));
        assert_eq!(config["geosite-matcher"].as_str(), Some("mph"));
        assert_eq!(config["geodata-mode"].as_bool(), Some(false));
        assert_eq!(config["geo-auto-update"].as_bool(), Some(false));
        assert_eq!(config["geox-url"]["mmdb"].as_str(), Some("http://127.0.0.1:1/db"));
        manager.start().await?;
        let readback = manager.geo_settings().await?;
        assert!(readback.running && readback.error.is_none());
        assert!(readback.fields.iter().all(|f| !f.mismatch && f.setting == f.configured && f.configured == f.actual));
        let mut succinct = runtime.clone();
        succinct.geosite_matcher = Some(headless_core::config::settings::GeositeMatcher::Succinct);
        manager.set_settings(succinct).await?;
        assert_eq!(manager.geo_settings().await?.fields[8].actual, "succinct");
        manager.set_settings(runtime.clone()).await?;
        assert_eq!(manager.geo_settings().await?.fields[8].actual, "mph");
        let before = manager.status(); let saved = manager.settings().await?;
        let mut invalid = runtime.clone(); invalid.geo_update_interval = Some(49); invalid.geosite_matcher = Some(headless_core::config::settings::GeositeMatcher::Succinct);
        assert!(manager.set_settings(invalid).await.is_err());
        assert_eq!(manager.settings().await?, saved);
        assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(manager.status().pid, before.pid);
        assert_eq!(manager.runtime_config().await?, config);
        assert_eq!(manager.geo_settings().await?.fields[8].actual, "mph");
        assert_eq!(manager.profile_raw(uid.clone()).await?.yaml, source);
        manager.stop().await?;
        manager.set_settings(RuntimeSettings::default()).await?;
        assert_eq!(manager.runtime_config().await?["geodata-mode"].as_bool(), Some(true));
        assert_eq!(manager.runtime_config().await?["geosite-matcher"].as_str(), Some("succinct"));
        assert_eq!(manager.runtime_config().await?["geox-url"]["mmdb"].as_str(), Some("http://127.0.0.1:1/script"));
        let saved = manager.set_settings(runtime.clone()).await?;
        assert_eq!(manager.status().phase, CorePhase::Stopped);
        assert!(manager.geo_settings().await?.fields.iter().all(|f| f.actual.is_null()));
        assert_eq!(manager.profile_raw(uid).await?.yaml, source);
        Ok::<_, anyhow::Error>(saved)
    }.await;
    let cleanup = manager.shutdown().await;
    let saved = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(restored.settings().await?, saved);
        let actual = restored.geo_settings().await?;
        assert!(actual.running && actual.error.is_none());
        assert!(
            actual
                .fields
                .iter()
                .all(|f| !f.mismatch && f.setting == f.configured && f.configured == f.actual)
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result.and(cleanup)
}

#[tokio::test]
#[ignore = "requires real Mihomo and bounded script worker"]
async fn connection_settings_authority_switching_rollback_inheritance_and_restart() -> Result<()> {
    use headless_core::config::settings::{FindProcessMode, RuntimeSettings};
    let dir = Directory::new()?;
    let manager = CoreManager::spawn(dir.options()?)?;
    let runtime: RuntimeSettings = serde_yaml_ng::from_str(
        "tcp-concurrent: false\nfind-process-mode: off\nkeep-alive-interval: 0\nkeep-alive-idle: -1\ndisable-keep-alive: false\nipv6: false",
    )?;
    let result = async {
        manager.set_settings(runtime.clone()).await?;
        let source = "mode: direct\nmixed-port: 0\ntcp-concurrent: true\nfind-process-mode: strict\nkeep-alive-interval: 15\nkeep-alive-idle: 30\ndisable-keep-alive: true\nipv6: false\ndns: {enable: false}\ntun: {enable: false}\nrules: ['MATCH,DIRECT']";
        let uid = manager.import_profile_yaml(source.into(), "connection settings".into()).await?.uid.unwrap().to_string();
        manager.select_profile(uid.clone()).await?;
        manager.set_profile_script(uid.clone(), Some("function main(c) { c['tcp-input']=c['tcp-concurrent']; c['process-input']=c['find-process-mode']; c['interval-input']=c['keep-alive-interval']; c['idle-input']=c['keep-alive-idle']; c['disable-input']=c['disable-keep-alive']; c['keep-alive-interval']=15; c['keep-alive-idle']=30; c['disable-keep-alive']=true; c['tcp-concurrent']=true; c['find-process-mode']='always'; if(c.ipv6) c.rules=['INVALID,DIRECT']; return c; }".into())).await?;
        let config = manager.runtime_config().await?;
        assert_eq!(config["tcp-input"].as_bool(), Some(false));
        assert_eq!(config["process-input"].as_str(), Some("off"));
        assert_eq!(config["interval-input"].as_i64(), Some(0));
        assert_eq!(config["idle-input"].as_i64(), Some(-1));
        assert_eq!(config["disable-input"].as_bool(), Some(false));
        manager.start().await?;
        assert!(manager.connection_settings().await?.fields.iter().all(|f| f.actual == f.setting && f.configured == f.setting && !f.mismatch));
        for (mode, text) in [(FindProcessMode::Strict, "strict"), (FindProcessMode::Always, "always"), (FindProcessMode::Off, "off")] {
            let mut changed = runtime.clone(); changed.keep_alive_interval = Some(20); changed.keep_alive_idle = Some(40); changed.disable_keep_alive = Some(true); changed.tcp_concurrent = Some(true); changed.find_process_mode = Some(mode);
            manager.set_settings(changed).await?;
            let actual = manager.connection_settings().await?;
            assert_eq!(actual.fields[0].actual, true); assert_eq!(actual.fields[1].actual, text);
            assert_eq!(actual.fields[2].actual, 20); assert_eq!(actual.fields[3].actual, 40); assert_eq!(actual.fields[4].actual, true);
            assert!(actual.fields.iter().all(|f| !f.mismatch));
        }
        let saved = manager.set_settings(runtime.clone()).await?;
        let before = manager.status();
        let mut invalid = runtime.clone(); invalid.keep_alive_interval = Some(60); invalid.keep_alive_idle = Some(120); invalid.disable_keep_alive = Some(true); invalid.ipv6 = Some(true); invalid.tcp_concurrent = Some(true); invalid.find_process_mode = Some(FindProcessMode::Always);
        assert!(manager.set_settings(invalid).await.is_err());
        assert_eq!(manager.status().pid, before.pid); assert_eq!(manager.status().config_revision, before.config_revision);
        assert_eq!(manager.settings().await?, saved); assert_eq!(manager.runtime_config().await?, config);
        assert_eq!(manager.connection_settings().await?.fields[0].actual, false);
        assert_eq!(manager.connection_settings().await?.fields[1].actual, "off");
        assert!(manager.connection_settings().await?.fields.iter().all(|f| f.setting == f.actual && !f.mismatch));
        manager.stop().await?;
        manager.set_settings(RuntimeSettings::default()).await?;
        let inherited = manager.connection_settings().await?;
        assert_eq!(inherited.fields[0].configured, true); assert_eq!(inherited.fields[1].configured, "always");
        assert_eq!(inherited.fields[2].configured, 15); assert_eq!(inherited.fields[3].configured, 30); assert_eq!(inherited.fields[4].configured, true);
        assert!(inherited.fields.iter().all(|f| f.setting.is_null() && f.actual.is_null()));
        manager.set_settings(runtime).await?;
        assert_eq!(manager.profile_raw(uid).await?.yaml, source);
        Ok::<_, anyhow::Error>(saved)
    }.await;
    let cleanup = manager.shutdown().await;
    let saved = result?;
    cleanup?;
    let restored = CoreManager::spawn(dir.options()?)?;
    let result = async {
        restored.start().await?;
        assert_eq!(restored.settings().await?, saved);
        let actual = restored.connection_settings().await?;
        assert!(actual.running && actual.error.is_none());
        assert!(
            actual
                .fields
                .iter()
                .all(|f| f.setting == f.configured && f.configured == f.actual && !f.mismatch)
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = restored.shutdown().await;
    result.and(cleanup)
}
