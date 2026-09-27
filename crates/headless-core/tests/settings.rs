use anyhow::Result;
use headless_core::config::{
    runtime::{RuntimeStore, parse},
    settings::{MAX_SETTINGS_BYTES, Mode, ServiceSettings, SettingsStore},
};
use std::{fs, path::PathBuf};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-settings-{}-{stamp:x}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

#[test]
fn interrupted_settings_update_follows_runtime_commit_at_each_publication_phase() -> Result<()> {
    for published in [false, true] {
        for committed in [false, true] {
            let dir = Directory::new()?;
            let mut runtime = RuntimeStore::open(&dir.0)?;
            let first = runtime.stage(parse("mode: direct")?)?;
            runtime.begin(first)?;
            runtime.commit()?;
            let mut settings = SettingsStore::open(&dir.0)?;
            let prior = settings.snapshot();
            let mut candidate = prior.clone();
            candidate.runtime.mode = Some(Mode::Global);
            candidate.runtime.tcp_concurrent = Some(false);
            candidate.runtime.keep_alive_interval = Some(0);
            candidate.runtime.keep_alive_idle = Some(-1);
            candidate.runtime.disable_keep_alive = Some(false);
            candidate.runtime.find_process_mode = Some(headless_core::config::settings::FindProcessMode::Off);
            candidate.profile_dns.insert(
                "one".into(),
                headless_core::config::dns::ProfileDnsSettings { enabled: false },
            );
            candidate.runtime.dns = Some(serde_yaml_ng::from_str("enable: false\nnameserver: [1.1.1.1]")?);
            candidate.runtime.tun = Some(serde_yaml_ng::from_str("enable: false\nmtu: 1500")?);
            let next = runtime.stage(parse("mode: global")?)?;
            runtime.begin(next.clone())?;
            settings.begin(candidate.clone(), next)?;
            assert!(
                settings
                    .begin(candidate.clone(), runtime.state().pending.unwrap())
                    .is_err()
            );
            if published {
                settings.publish()?;
            }
            if committed {
                runtime.commit()?;
            }
            drop(settings);
            drop(runtime);
            let runtime = RuntimeStore::open(&dir.0)?;
            let mut settings = SettingsStore::open(&dir.0)?;
            settings.recover(runtime.state().current.as_ref())?;
            assert_eq!(settings.snapshot(), if committed { candidate } else { prior });
            assert!(!dir.0.join("settings-transaction.yaml").exists());
            settings.recover(runtime.state().current.as_ref())?;
        }
    }
    Ok(())
}

#[test]
fn settings_recovery_preserves_conflicting_files_and_rejects_unsafe_journals() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = SettingsStore::open(&dir.0)?;
    let original = store.snapshot();
    let mut candidate = original.clone();
    candidate.runtime.mode = Some(Mode::Global);
    let revision = headless_core::config::runtime::Revision {
        file: "rev-a.yaml".into(),
    };
    store.begin(candidate, revision)?;
    let mut conflicting = original.clone();
    conflicting.runtime.mode = Some(Mode::Direct);
    fs::write(dir.0.join("settings.yaml"), serde_yaml_ng::to_string(&conflicting)?)?;
    assert!(store.recover(None).is_err());
    assert_eq!(store.snapshot(), original);
    assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), conflicting);
    let journal = dir.0.join("settings-transaction.yaml");
    assert!(journal.is_file());
    fs::write(&journal, "schema_version: 99")?;
    assert!(store.recover(None).is_err());
    fs::write(&journal, " ".repeat(2 * MAX_SETTINGS_BYTES + 4097))?;
    assert!(store.recover(None).is_err());
    fs::remove_file(&journal)?;
    fs::create_dir(&journal)?;
    assert!(store.recover(None).is_err());
    Ok(())
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn default_is_persisted_without_changing_source_and_explicit_zero_false_survive_restart() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = SettingsStore::open(&dir.0)?;
    let base = parse("MODE: direct\nmixed-port: 4321\nallow-lan: true\ndns: {enable: false}")?;
    assert_eq!(store.snapshot().runtime.enforce(base.clone())?, base);
    let mut settings = store.snapshot();
    settings.runtime.mode = Some(Mode::Global);
    settings.runtime.mixed_port = Some(0);
    settings.runtime.allow_lan = Some(false);
    store.replace(settings.clone())?;
    assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), settings);
    let applied = settings.runtime.enforce(base)?;
    assert_eq!(applied["mode"].as_str(), Some("global"));
    assert_eq!(applied["mixed-port"].as_u64(), Some(0));
    assert_eq!(applied["allow-lan"].as_bool(), Some(false));
    assert_eq!(applied["dns"]["enable"].as_bool(), Some(false));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(dir.0.join("settings.yaml"))?.permissions().mode() & 0o777,
            0o600
        );
    }
    Ok(())
}

#[test]
fn malformed_unknown_unsafe_and_oversized_settings_are_rejected_without_replacement() -> Result<()> {
    let dir = Directory::new()?;
    for yaml in [
        "schema_version: 2",
        "schema_version: 1\nextra: true",
        "schema_version: 1\nruntime: {external-controller: '127.0.0.1:9999'}",
        "schema_version: 1\nruntime: {mixed-port: 65536}",
        "schema_version: 1\nruntime: {mixed-port: -1}",
        "schema_version: 1\nruntime: {mode: invalid}",
        "schema_version: 1\nruntime: {log-level: verbose}",
        "schema_version: 1\nruntime: {allow-lan: 'false'}",
        "schema_version: 1\nruntime: {tun: {enable: 'true'}}",
        "schema_version: 1\nruntime: {tun: {stack: invalid}}",
        "schema_version: 1\nruntime: {tun: {mtu: 0}}",
        "schema_version: 1\nruntime: {tun: {mtu: 65536}}",
        "schema_version: 1\nruntime: {tun: {device: ''}}",
        "schema_version: 1\nruntime: {tun: {unknown: false}}",
        "schema_version: 1\nruntime: {dns: {nameserver: '1.1.1.1'}}",
        "schema_version: 1\nruntime: {dns: {enhanced-mode: invalid}}",
        "schema_version: 1\nruntime: {dns: {nameserver-policy: {example.org: 1.1.1.1}}}",
        "[]",
    ] {
        fs::write(dir.0.join("settings.yaml"), yaml)?;
        assert!(SettingsStore::open(&dir.0).is_err(), "accepted {yaml}");
        assert_eq!(fs::read_to_string(dir.0.join("settings.yaml"))?, yaml);
    }
    fs::write(dir.0.join("settings.yaml"), " ".repeat(MAX_SETTINGS_BYTES + 1))?;
    assert!(SettingsStore::open(&dir.0).is_err());
    fs::remove_file(dir.0.join("settings.yaml"))?;
    fs::create_dir(dir.0.join("settings.yaml"))?;
    assert!(SettingsStore::open(&dir.0).is_err());
    Ok(())
}

#[test]
fn failed_replace_retains_memory_and_cleans_temporary_file() -> Result<()> {
    let dir = Directory::new()?;
    let mut store = SettingsStore::open(&dir.0)?;
    let prior = store.snapshot();
    let mut candidate = prior.clone();
    candidate.schema_version = 2;
    assert!(store.replace(candidate).is_err());
    assert_eq!(store.snapshot(), prior);
    fs::remove_file(dir.0.join("settings.yaml"))?;
    fs::create_dir(dir.0.join("settings.yaml"))?;
    let mut candidate = prior.clone();
    candidate.runtime.mode = Some(Mode::Global);
    assert!(store.replace(candidate).is_err());
    assert_eq!(store.snapshot(), prior);
    assert_eq!(fs::read_dir(&dir.0)?.count(), 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlink_settings_are_rejected_without_touching_target() -> Result<()> {
    let dir = Directory::new()?;
    let target = dir.0.join("outside.yaml");
    fs::write(&target, "schema_version: 1")?;
    std::os::unix::fs::symlink(&target, dir.0.join("settings.yaml"))?;
    assert!(SettingsStore::open(&dir.0).is_err());
    assert_eq!(fs::read_to_string(target)?, "schema_version: 1");
    Ok(())
}

#[test]
fn explicit_authority_survives_removed_or_uppercase_overrides_only_for_owned_fields() -> Result<()> {
    let settings: ServiceSettings =
        serde_yaml_ng::from_str("schema_version: 1\nruntime: {mode: rule, ipv6: false, unified-delay: true}")?;
    let config = settings
        .runtime
        .enforce(parse("MODE: global\ndns: {ipv6: true}\ncustom: 7")?)?;
    assert_eq!(config["mode"].as_str(), Some("rule"));
    assert_eq!(config["ipv6"].as_bool(), Some(false));
    assert_eq!(config["unified-delay"].as_bool(), Some(true));
    assert_eq!(config["dns"]["ipv6"].as_bool(), Some(true));
    assert_eq!(config["custom"].as_u64(), Some(7));
    // Controller input remains rejected by the existing boundary, never overridden.
    let unsafe_config = serde_yaml_ng::from_str("external-controller: '127.0.0.1:9999'")?;
    assert!(settings.runtime.enforce(unsafe_config).is_err());
    Ok(())
}

#[test]
fn nested_authority_is_shallow_preserves_provider_fields_and_reports_owned_leaves() -> Result<()> {
    let settings: ServiceSettings = serde_yaml_ng::from_str(
        "schema_version: 1\nruntime:\n  dns: {nameserver: [1.1.1.1], ipv6: true}\n  tun: {enable: false, dns-hijack: [], mtu: 1500}",
    )?;
    let base = parse(
        "dns: {nameserver: [9.9.9.9], ipv6: false, nameserver-policy: {example.org: 8.8.8.8}, fallback: [8.8.4.4]}\ntun: {enable: true, dns-hijack: ['any:53'], mtu: 9000, custom: retained}",
    )?;
    let applied = settings.runtime.enforce(base.clone())?;
    assert_eq!(applied["dns"]["nameserver"], serde_yaml_ng::to_value(["1.1.1.1"])?);
    assert_eq!(applied["dns"]["nameserver-policy"], base["dns"]["nameserver-policy"]);
    assert_eq!(applied["dns"]["fallback"], base["dns"]["fallback"]);
    assert_eq!(applied["tun"]["enable"].as_bool(), Some(false));
    assert!(applied["tun"]["dns-hijack"].as_sequence().unwrap().is_empty());
    assert_eq!(applied["tun"]["custom"].as_str(), Some("retained"));
    let changed = settings.runtime.overridden_fields(&base, &applied)?;
    for key in ["dns.nameserver", "dns.ipv6", "tun.enable", "tun.dns-hijack", "tun.mtu"] {
        assert!(changed.contains(&key.to_owned()));
    }
    assert_eq!(changed.len(), 5);
    assert!(settings.runtime.overridden_fields(&applied, &applied)?.is_empty());
    for removed in [
        "{}",
        "dns: null\ntun: []",
        "DNS: {nameserver: [9.9.9.9]}\nTUN: {enable: true}",
    ] {
        let restored = settings.runtime.enforce(parse(removed)?)?;
        assert_eq!(restored["dns"]["nameserver"], applied["dns"]["nameserver"]);
        assert_eq!(restored["tun"]["enable"].as_bool(), Some(false));
    }
    Ok(())
}

#[test]
fn dns_page_empty_and_false_values_inherit_while_tun_false_and_empty_lists_are_owned() -> Result<()> {
    let settings: ServiceSettings = serde_yaml_ng::from_str(
        "schema_version: 1\nruntime:\n  dns: {enable: false, ipv6: false, listen: '  ', nameserver: [], fallback: null}\n  tun: {enable: false, auto-route: false, route-exclude-address: []}",
    )?;
    let base = parse(
        "dns: {enable: true, ipv6: true, listen: '127.0.0.1:5353', nameserver: [9.9.9.9]}\ntun: {enable: true, auto-route: true, route-exclude-address: [10.0.0.0/8]}",
    )?;
    let applied = settings.runtime.enforce(base.clone())?;
    assert_eq!(applied["dns"], base["dns"]);
    assert_eq!(applied["tun"]["auto-route"].as_bool(), Some(false));
    assert!(
        applied["tun"]["route-exclude-address"]
            .as_sequence()
            .unwrap()
            .is_empty()
    );
    let empty: ServiceSettings = serde_yaml_ng::from_str("schema_version: 1\nruntime: {dns: {}, tun: {}}")?;
    assert_eq!(empty.runtime.enforce(base.clone())?, base);
    assert!(empty.runtime.enforce(parse("{}")?)?.is_empty());
    let dir = Directory::new()?;
    let mut store = SettingsStore::open(&dir.0)?;
    store.replace(settings.clone())?;
    assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), settings);
    Ok(())
}

#[test]
fn geo_authority_is_per_url_leaf_false_is_explicit_and_empty_maps_inherit() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let base = parse(
        "geodata-mode: true\ngeodata-loader: memconservative\ngeo-auto-update: true\ngeo-update-interval: 24\ngeox-url: {geoip: 'https://source.invalid/ip', geosite: 'https://source.invalid/site', mmdb: 'https://source.invalid/db', future: preserved}\ncustom: unchanged",
    )?;
    let runtime: RuntimeSettings = serde_yaml_ng::from_str(
        "geodata-mode: false\ngeodata-loader: standard\ngeo-auto-update: false\ngeo-update-interval: 48\ngeox-url: {mmdb: 'http://127.0.0.1/db'}",
    )?;
    let initial = runtime.prepare(base.clone())?;
    assert_eq!(initial["geodata-mode"].as_bool(), Some(false));
    assert_eq!(initial["geo-auto-update"].as_bool(), Some(false));
    assert_eq!(initial["geo-update-interval"].as_u64(), Some(48));
    assert_eq!(initial["geox-url"]["geoip"], base["geox-url"]["geoip"]);
    assert_eq!(initial["geox-url"]["future"].as_str(), Some("preserved"));
    let enhanced = parse(
        "geodata-mode: true\ngeo-auto-update: true\ngeox-url: {mmdb: 'https://enhancement.invalid/db', geosite: 'https://enhancement.invalid/site', future: retained}",
    )?;
    let final_config = runtime.enforce(enhanced.clone())?;
    assert_eq!(final_config["geox-url"]["mmdb"].as_str(), Some("http://127.0.0.1/db"));
    assert_eq!(final_config["geox-url"]["geosite"], enhanced["geox-url"]["geosite"]);
    assert!(
        runtime
            .overridden_fields(&enhanced, &final_config)?
            .contains(&"geox-url.mmdb".into())
    );
    assert_eq!(
        serde_yaml_ng::from_str::<RuntimeSettings>("geox-url: {}")?.enforce(base.clone())?,
        base
    );
    assert_eq!(RuntimeSettings::default().enforce(final_config.clone())?, final_config);
    Ok(())
}

#[test]
fn geo_settings_validate_intervals_urls_and_enums_without_echoing_secrets() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    for yaml in [
        "geodata-loader: invalid",
        "geosite-matcher: invalid",
        "geosite-matcher: hybrid",
        "geosite-matcher: MPH",
        "geosite-matcher: true",
        "geodata-mode: string",
        "geo-auto-update: string",
        "geo-update-interval: -1",
        "geox-url: {unknown: 'https://example.org'}",
    ] {
        assert!(serde_yaml_ng::from_str::<RuntimeSettings>(yaml).is_err());
    }
    for yaml in [
        "geo-update-interval: 0",
        "geo-update-interval: 8761",
        "geox-url: {mmdb: ''}",
        "geox-url: {mmdb: 'file:///etc/passwd'}",
        "geox-url: {mmdb: 'https://secret:private@example.org/db'}",
        "geox-url: {mmdb: 'https://example.org/db#'}",
        "geox-url: {mmdb: ' https://example.org/db'}",
        "geox-url: {mmdb: 'https://example.org/db path'}",
    ] {
        let runtime = serde_yaml_ng::from_str::<RuntimeSettings>(yaml)?;
        let error = runtime.validate().unwrap_err().to_string();
        assert!(!error.contains("secret") && !error.contains("private") && !error.contains("passwd"));
    }
    let huge = format!("https://example.org/{}", "a".repeat(8192));
    let runtime: RuntimeSettings = serde_json::from_value(serde_json::json!({"geox-url":{"mmdb":huge}}))?;
    assert!(runtime.validate().is_err());
    for hours in [1, 8760] {
        let runtime: RuntimeSettings = serde_json::from_value(
            serde_json::json!({"geo-update-interval":hours,"geox-url":{"mmdb":"https://example.org/db?token=private"}}),
        )?;
        runtime.validate()?;
    }
    Ok(())
}

#[test]
fn geosite_matcher_authority_uses_canonical_values_and_absence_preserves_source() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let source: serde_yaml_ng::Mapping = serde_yaml_ng::from_str("geosite-matcher: hybrid\ncustom: unchanged")?;
    assert_eq!(RuntimeSettings::default().prepare(source.clone())?, source);
    for matcher in ["succinct", "mph"] {
        let runtime: RuntimeSettings = serde_yaml_ng::from_str(&format!("geosite-matcher: {matcher}"))?;
        let initial = runtime.prepare(source.clone())?;
        assert_eq!(initial["geosite-matcher"].as_str(), Some(matcher));
        let enhanced: serde_yaml_ng::Mapping = serde_yaml_ng::from_str("geosite-matcher: invalid\ncustom: unchanged")?;
        let final_config = runtime.enforce(enhanced.clone())?;
        assert_eq!(final_config["geosite-matcher"].as_str(), Some(matcher));
        assert_eq!(
            runtime.overridden_fields(&enhanced, &final_config)?,
            ["geosite-matcher"]
        );
        assert_eq!(final_config["custom"], source["custom"]);
    }
    let inherited: RuntimeSettings = serde_yaml_ng::from_str("geosite-matcher: null")?;
    assert_eq!(inherited.prepare(source.clone())?, source);
    assert!(serde_yaml_ng::to_string(&inherited)?.trim() == "{}");
    Ok(())
}

#[test]
fn geo_fields_persist_in_schema_one_and_follow_settings_transaction_recovery() -> Result<()> {
    for committed in [false, true] {
        let dir = Directory::new()?;
        let mut runtime = RuntimeStore::open(&dir.0)?;
        let prior = runtime.stage(parse("mode: direct")?)?;
        runtime.begin(prior)?;
        runtime.commit()?;
        let mut store = SettingsStore::open(&dir.0)?;
        let original = store.snapshot();
        let mut candidate = original.clone();
        candidate.runtime = serde_yaml_ng::from_str(
            "geodata-mode: false\ngeo-auto-update: false\ngeo-update-interval: 48\ngeodata-loader: standard\ngeosite-matcher: mph\ngeox-url: {geoip: 'http://127.0.0.1/ip', geosite: 'https://example.org/site', mmdb: 'https://example.org/db', asn: 'https://example.org/asn'}",
        )?;
        let next = runtime.stage(candidate.runtime.enforce(parse("mode: direct")?)?)?;
        runtime.begin(next.clone())?;
        store.begin(candidate.clone(), next)?;
        store.publish()?;
        if committed {
            runtime.commit()?;
        }
        drop(store);
        drop(runtime);
        let runtime = RuntimeStore::open(&dir.0)?;
        let mut store = SettingsStore::open(&dir.0)?;
        store.recover(runtime.state().current.as_ref())?;
        assert_eq!(store.snapshot(), if committed { candidate } else { original });
        assert!(!dir.0.join("settings-transaction.yaml").exists());
    }
    Ok(())
}

#[test]
fn oversized_combined_settings_are_rejected_before_a_transaction_journal_is_written() -> Result<()> {
    let dir = Directory::new()?;
    let store = SettingsStore::open(&dir.0)?;
    let original = store.snapshot();
    let mut candidate = original.clone();
    candidate.runtime.geox_url = Some(serde_json::from_value(
        serde_json::json!({"mmdb":format!("https://example.org/{}", "a".repeat(8000))}),
    )?);
    candidate.runtime.dns = Some(serde_json::from_value(
        serde_json::json!({"nameserver":["x".repeat(MAX_SETTINGS_BYTES-1000)]}),
    )?);
    assert!(candidate.runtime.validate().is_ok());
    assert!(candidate.validate().is_err());
    assert!(
        store
            .begin(
                candidate,
                headless_core::config::runtime::Revision {
                    file: "candidate.yaml".into()
                }
            )
            .is_err()
    );
    assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), original);
    assert!(!dir.0.join("settings-transaction.yaml").exists());
    Ok(())
}

#[test]
fn connection_settings_preserve_inheritance_and_enforce_false_and_process_modes() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let source: serde_yaml_ng::Mapping =
        serde_yaml_ng::from_str("tcp-concurrent: true\nfind-process-mode: always\ncustom: preserved")?;
    assert_eq!(RuntimeSettings::default().prepare(source.clone())?, source);
    for mode in ["strict", "always", "off"] {
        let runtime: RuntimeSettings =
            serde_yaml_ng::from_str(&format!("tcp-concurrent: false\nfind-process-mode: {mode}"))?;
        let initial = runtime.prepare(source.clone())?;
        assert_eq!(initial["tcp-concurrent"].as_bool(), Some(false));
        assert_eq!(initial["find-process-mode"].as_str(), Some(mode));
        let final_config = runtime.enforce(source.clone())?;
        assert_eq!(final_config["tcp-concurrent"].as_bool(), Some(false));
        assert_eq!(final_config["find-process-mode"].as_str(), Some(mode));
        assert_eq!(final_config["custom"], source["custom"]);
        assert!(
            runtime
                .overridden_fields(&source, &final_config)?
                .contains(&"tcp-concurrent".to_owned())
        );
    }
    let null: RuntimeSettings = serde_yaml_ng::from_str("tcp-concurrent: null\nfind-process-mode: null")?;
    assert_eq!(null.enforce(source.clone())?, source);
    for invalid in [
        "tcp-concurrent: 'false'",
        "tcp-concurrent: 0",
        "find-process-mode: Strict",
        "find-process-mode: invalid",
        "find-process-mode: false",
    ] {
        assert!(serde_yaml_ng::from_str::<RuntimeSettings>(invalid).is_err());
    }
    Ok(())
}

#[test]
fn keep_alive_settings_preserve_signed_seconds_false_and_inheritance() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let source = parse("keep-alive-interval: 15\nkeep-alive-idle: 30\ndisable-keep-alive: true\ncustom: retained")?;
    assert_eq!(RuntimeSettings::default().enforce(source.clone())?, source);
    let null: RuntimeSettings =
        serde_yaml_ng::from_str("keep-alive-interval: null\nkeep-alive-idle: null\ndisable-keep-alive: null")?;
    assert_eq!(null.enforce(source.clone())?, source);
    for seconds in [i32::MIN, -1, 0, 15, i32::MAX] {
        let runtime = RuntimeSettings {
            keep_alive_interval: Some(seconds),
            keep_alive_idle: Some(seconds),
            disable_keep_alive: Some(false),
            ..Default::default()
        };
        let initial = runtime.prepare(source.clone())?;
        assert_eq!(initial["keep-alive-idle"].as_i64(), Some(i64::from(seconds)));
        let mut enhanced = initial;
        enhanced.extend(source.clone());
        let final_config = runtime.enforce(enhanced)?;
        assert_eq!(final_config["keep-alive-interval"].as_i64(), Some(i64::from(seconds)));
        assert_eq!(final_config["keep-alive-idle"].as_i64(), Some(i64::from(seconds)));
        assert_eq!(final_config["disable-keep-alive"].as_bool(), Some(false));
        assert_eq!(final_config["custom"], source["custom"]);
        let serialized = serde_yaml_ng::to_string(&runtime)?;
        assert_eq!(serde_yaml_ng::from_str::<RuntimeSettings>(&serialized)?, runtime);
    }
    for invalid in [
        "keep-alive-interval: 2147483648",
        "keep-alive-idle: -2147483649",
        "keep-alive-idle: 1.5",
        "keep-alive-interval: '15'",
        "keep-alive-idle: true",
        "disable-keep-alive: 0",
        "disable-keep-alive: 'false'",
    ] {
        assert!(
            serde_yaml_ng::from_str::<RuntimeSettings>(invalid).is_err(),
            "{invalid}"
        );
    }
    Ok(())
}
