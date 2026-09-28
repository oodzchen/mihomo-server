use anyhow::Result;
use headless_core::config::{
    runtime::{RuntimeStore, parse},
    settings::{MAX_SETTINGS_BYTES, Mode, RuntimeSettings, ServiceSettings, SettingsStore},
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
            candidate.runtime.interface_name = Some(String::new());
            candidate.runtime.global_ua = Some(String::new());
            candidate.runtime.etag_support = Some(false);
            candidate.runtime.hosts = Some(serde_yaml_ng::from_str("{}")?);
            if cfg!(target_os = "linux") {
                candidate.runtime.routing_mark = Some(u32::MAX);
            }
            candidate.runtime.find_process_mode = Some(headless_core::config::settings::FindProcessMode::Off);
            candidate.profile_dns.insert(
                "one".into(),
                headless_core::config::dns::ProfileDnsSettings { enabled: false },
            );
            candidate.runtime.dns = Some(serde_yaml_ng::from_str(
                "enable: false\nuse-hosts: false\nuse-system-hosts: false\nnameserver: [1.1.1.1]",
            )?);
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
        "schema_version: 1\nruntime: {tun: {device: 'abcdefghijklmnop'}}",
        "schema_version: 1\nruntime: {tun: {device: 'tun:0'}}",
        "schema_version: 1\nruntime: {tun: {device: 'tun/0'}}",
        "schema_version: 1\nruntime: {tun: {unknown: false}}",
        "schema_version: 1\nruntime: {dns: {nameserver: '1.1.1.1'}}",
        "schema_version: 1\nruntime: {dns: {enhanced-mode: invalid}}",
        "schema_version: 1\nruntime: {dns: {nameserver-policy: {example.org: 123}}}",
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
fn resolver_policies_and_fallback_filter_keep_unowned_source_leaves() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let source = parse(
        "dns: {nameserver-policy: {source.test: 9.9.9.9}, proxy-server-nameserver-policy: {source.proxy: [1.1.1.1]}, proxy-server-nameserver: [9.9.9.9], direct-nameserver-follow-policy: true, fallback-filter: {geoip: true, geoip-code: CN, ipcidr: [240.0.0.0/4], domain: ['+.source.test']}, custom: retained}",
    )?;
    let runtime: RuntimeSettings = serde_yaml_ng::from_str(
        "dns: {nameserver-policy: {owned.test: [1.1.1.1, 'https://dns.example/dns-query']}, proxy-server-nameserver-policy: {node.test: 8.8.8.8}, proxy-server-nameserver: [1.1.1.1], direct-nameserver: [8.8.4.4], direct-nameserver-follow-policy: false, fallback-filter: {geoip: false, domain: ['+.owned.test']}}",
    )?;
    for applied in [runtime.prepare(source.clone())?, runtime.enforce(source.clone())?] {
        assert!(applied["dns"]["nameserver-policy"].get("source.test").is_none());
        assert_eq!(
            applied["dns"]["nameserver-policy"]["owned.test"][0].as_str(),
            Some("1.1.1.1")
        );
        assert_eq!(
            applied["dns"]["proxy-server-nameserver-policy"]["node.test"].as_str(),
            Some("8.8.8.8")
        );
        assert_eq!(applied["dns"]["proxy-server-nameserver"][0].as_str(), Some("1.1.1.1"));
        assert_eq!(applied["dns"]["direct-nameserver"][0].as_str(), Some("8.8.4.4"));
        assert_eq!(applied["dns"]["direct-nameserver-follow-policy"].as_bool(), Some(true));
        assert_eq!(applied["dns"]["fallback-filter"]["geoip"].as_bool(), Some(false));
        assert_eq!(applied["dns"]["fallback-filter"]["geoip-code"].as_str(), Some("CN"));
        assert_eq!(
            applied["dns"]["fallback-filter"]["ipcidr"][0].as_str(),
            Some("240.0.0.0/4")
        );
        assert_eq!(
            applied["dns"]["fallback-filter"]["domain"][0].as_str(),
            Some("+.owned.test")
        );
        assert_eq!(applied["dns"]["custom"].as_str(), Some("retained"));
        let changed = runtime.overridden_fields(&source, &applied)?;
        assert!(changed.contains(&"dns.nameserver-policy".into()));
        assert!(changed.contains(&"dns.fallback-filter.geoip".into()));
        assert!(changed.contains(&"dns.fallback-filter.domain".into()));
        assert!(!changed.contains(&"dns.fallback-filter.geoip-code".into()));
    }
    let empty: RuntimeSettings =
        serde_yaml_ng::from_str("dns: {nameserver-policy: {}, fallback-filter: {geoip-code: ' ', ipcidr: []}}")?;
    assert_eq!(empty.enforce(source.clone())?, source);
    assert_eq!(
        serde_yaml_ng::from_str::<RuntimeSettings>(&serde_yaml_ng::to_string(&runtime)?)?,
        runtime
    );
    let dir = Directory::new()?;
    let mut store = SettingsStore::open(&dir.0)?;
    let mut saved = store.snapshot();
    saved.runtime = runtime;
    store.replace(saved.clone())?;
    assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), saved);
    Ok(())
}

#[test]
fn resolver_policies_and_fallback_filter_reject_invalid_saved_shapes() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    for invalid in [
        "dns: {nameserver-policy: []}",
        "dns: {nameserver-policy: {example.test: []}}",
        "dns: {nameserver-policy: {example.test: 123}}",
        "dns: {nameserver-policy: {example.test: [1.1.1.1, 123]}}",
        "dns: {proxy-server-nameserver-policy: {example.test: false}}",
        "dns: {fallback-filter: {geoip: 'false'}}",
        "dns: {fallback-filter: {geoip-code: 123}}",
        "dns: {fallback-filter: {unknown: true}}",
        "dns: {fallback-filter: {domain: [123]}}",
    ] {
        assert!(
            serde_yaml_ng::from_str::<RuntimeSettings>(invalid).is_err(),
            "accepted {invalid}"
        );
    }
    Ok(())
}

#[test]
fn remaining_dns_page_switches_follow_false_inheritance_and_enum_authority() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let source = parse(
        "dns: {enable: true, enhanced-mode: fake-ip, fake-ip-filter-mode: whitelist, prefer-h3: true, respect-rules: true, nameserver: [9.9.9.9]}",
    )?;
    let false_switches: RuntimeSettings =
        serde_yaml_ng::from_str("dns: {fake-ip-filter-mode: blacklist, prefer-h3: false, respect-rules: false}")?;
    let applied = false_switches.prepare(source.clone())?;
    assert_eq!(applied["dns"]["fake-ip-filter-mode"].as_str(), Some("blacklist"));
    assert_eq!(applied["dns"]["prefer-h3"].as_bool(), Some(true));
    assert_eq!(applied["dns"]["respect-rules"].as_bool(), Some(true));
    assert_eq!(applied["dns"]["nameserver"], source["dns"]["nameserver"]);
    assert_eq!(
        false_switches.overridden_fields(&source, &applied)?,
        ["dns.fake-ip-filter-mode"]
    );
    let true_switches: RuntimeSettings =
        serde_yaml_ng::from_str("dns: {fake-ip-filter-mode: whitelist, prefer-h3: true, respect-rules: true}")?;
    let disabled_source = parse("dns: {fake-ip-filter-mode: blacklist, prefer-h3: false, respect-rules: false}")?;
    let applied = true_switches.enforce(disabled_source)?;
    for key in ["prefer-h3", "respect-rules"] {
        assert_eq!(applied["dns"][key].as_bool(), Some(true));
    }
    assert_eq!(applied["dns"]["fake-ip-filter-mode"].as_str(), Some("whitelist"));
    for value in ["invalid", "BLACKLIST", "", "false", "123"] {
        assert!(
            serde_yaml_ng::from_str::<RuntimeSettings>(&format!("dns: {{fake-ip-filter-mode: '{value}'}}")).is_err()
        );
    }
    for invalid in ["dns: {prefer-h3: 'true'}", "dns: {respect-rules: 1}"] {
        assert!(serde_yaml_ng::from_str::<RuntimeSettings>(invalid).is_err());
    }
    let dir = Directory::new()?;
    let mut store = SettingsStore::open(&dir.0)?;
    let mut saved = store.snapshot();
    saved.runtime = true_switches;
    store.replace(saved.clone())?;
    assert_eq!(SettingsStore::open(&dir.0)?.snapshot(), saved);
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

#[test]
fn outbound_settings_validate_names_and_preserve_empty_zero_and_inheritance() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let source = parse("interface-name: source0\nrouting-mark: 123\ncustom: retained")?;
    assert_eq!(RuntimeSettings::default().enforce(source.clone())?, source);
    let null: RuntimeSettings = serde_yaml_ng::from_str("interface-name: null\nrouting-mark: null")?;
    assert_eq!(null.enforce(source.clone())?, source);
    for name in ["", "lo", "eth0.2", "a-b_1", "abcdefghijklmno", "网卡接口名"] {
        let runtime = RuntimeSettings {
            interface_name: Some(name.into()),
            ..Default::default()
        };
        assert_eq!(runtime.prepare(source.clone())?["interface-name"].as_str(), Some(name));
        let final_config = runtime.enforce(source.clone())?;
        assert_eq!(final_config["interface-name"].as_str(), Some(name));
        assert_eq!(final_config["routing-mark"].as_u64(), Some(123));
        assert_eq!(final_config["custom"], source["custom"]);
        assert_eq!(
            serde_yaml_ng::from_str::<RuntimeSettings>(&serde_yaml_ng::to_string(&runtime)?)?,
            runtime
        );
    }
    for name in [
        ".",
        "..",
        "abcdefghijklmnop",
        "网卡接口名字",
        "eth 0",
        "eth/0",
        "eth:0",
        "eth\n0",
        "eth\0",
        "eth\u{0085}0",
    ] {
        let runtime = RuntimeSettings {
            interface_name: Some(name.into()),
            ..Default::default()
        };
        assert!(runtime.validate().is_err(), "{name:?}");
        assert!(runtime.enforce(source.clone()).is_err());
    }
    for mark in [0, 123, u32::MAX] {
        let runtime = RuntimeSettings {
            interface_name: Some(String::new()),
            routing_mark: Some(mark),
            ..Default::default()
        };
        if cfg!(target_os = "linux") {
            let mut enhanced = runtime.prepare(source.clone())?;
            enhanced.extend(source.clone());
            let final_config = runtime.enforce(enhanced)?;
            assert_eq!(final_config["interface-name"].as_str(), Some(""));
            assert_eq!(final_config["routing-mark"].as_u64(), Some(u64::from(mark)));
        } else {
            assert!(runtime.validate().is_err());
        }
    }
    for invalid in [
        "interface-name: 123",
        "interface-name: true",
        "routing-mark: -1",
        "routing-mark: 4294967296",
        "routing-mark: 1.5",
        "routing-mark: '1'",
        "routing-mark: true",
    ] {
        assert!(
            serde_yaml_ng::from_str::<RuntimeSettings>(invalid).is_err(),
            "{invalid}"
        );
    }
    Ok(())
}

#[test]
fn download_settings_preserve_empty_false_and_reject_invalid_header_text() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let source = parse("global-ua: source/1\netag-support: true\ncustom: retained")?;
    assert_eq!(RuntimeSettings::default().enforce(source.clone())?, source);
    let null: RuntimeSettings = serde_yaml_ng::from_str("global-ua: null\netag-support: null")?;
    assert_eq!(null.enforce(source.clone())?, source);
    for agent in [String::new(), "agent/1.0 (Linux; x86_64)".into(), "x".repeat(1024)] {
        let runtime = RuntimeSettings {
            global_ua: Some(agent.clone()),
            etag_support: Some(false),
            ..Default::default()
        };
        assert_eq!(
            runtime.prepare(source.clone())?["global-ua"].as_str(),
            Some(agent.as_str())
        );
        let final_config = runtime.enforce(source.clone())?;
        assert_eq!(final_config["global-ua"].as_str(), Some(agent.as_str()));
        assert_eq!(final_config["etag-support"].as_bool(), Some(false));
        assert_eq!(final_config["custom"], source["custom"]);
        assert_eq!(
            serde_yaml_ng::from_str::<RuntimeSettings>(&serde_yaml_ng::to_string(&runtime)?)?,
            runtime
        );
    }
    for agent in [
        "x".repeat(1025),
        "bad\r\nheader".into(),
        "bad\tvalue".into(),
        "bad\0".into(),
        "bad\u{007f}".into(),
        "中文".into(),
    ] {
        let runtime = RuntimeSettings {
            global_ua: Some(agent),
            ..Default::default()
        };
        assert!(runtime.validate().is_err());
        assert!(runtime.enforce(source.clone()).is_err());
    }
    for invalid in [
        "global-ua: 123",
        "global-ua: true",
        "global-ua: []",
        "etag-support: 0",
        "etag-support: 'false'",
    ] {
        assert!(
            serde_yaml_ng::from_str::<RuntimeSettings>(invalid).is_err(),
            "{invalid}"
        );
    }
    Ok(())
}

#[test]
fn hosts_replace_the_whole_map_preserve_shapes_and_own_false_dns_switches() -> Result<()> {
    use headless_core::config::settings::RuntimeSettings;
    let source = parse(
        "hosts: {source.test: 192.0.2.1}\ndns: {use-hosts: true, use-system-hosts: true, ipv6: true, nameserver: [9.9.9.9]}",
    )?;
    let runtime: RuntimeSettings = serde_yaml_ng::from_str(
        "hosts: {'*.example.test': 192.0.2.2, ipv6.example.test: ['2001:db8::1', 192.0.2.3], alias.example.test: ipv6.example.test, lan.example.test: lan}\ndns: {use-hosts: false, use-system-hosts: false, ipv6: false}",
    )?;
    let owned = serde_yaml_ng::to_value(&runtime)?;
    for applied in [runtime.prepare(source.clone())?, runtime.enforce(source.clone())?] {
        assert_eq!(applied["hosts"], owned["hosts"]);
        assert_eq!(applied["dns"]["use-hosts"].as_bool(), Some(false));
        assert_eq!(applied["dns"]["use-system-hosts"].as_bool(), Some(false));
        assert_eq!(applied["dns"]["ipv6"].as_bool(), Some(true));
        assert_eq!(applied["dns"]["nameserver"], source["dns"]["nameserver"]);
        assert_eq!(
            runtime.overridden_fields(&source, &applied)?,
            ["dns.use-hosts", "dns.use-system-hosts", "hosts"]
        );
    }
    for data in [serde_yaml_ng::to_string(&runtime)?, serde_json::to_string(&runtime)?] {
        assert_eq!(serde_yaml_ng::from_str::<RuntimeSettings>(&data)?, runtime);
    }
    let empty: RuntimeSettings = serde_yaml_ng::from_str("hosts: {}")?;
    let cleared = empty.enforce(source.clone())?;
    assert!(cleared["hosts"].as_mapping().unwrap().is_empty());
    assert_eq!(empty.overridden_fields(&source, &cleared)?, ["hosts"]);
    let inherited: RuntimeSettings =
        serde_yaml_ng::from_str("hosts: null\ndns: {use-hosts: null, use-system-hosts: null}")?;
    assert_eq!(inherited.enforce(source.clone())?, source);
    Ok(())
}

#[test]
fn hosts_reject_coerced_types_invalid_patterns_lists_and_alias_cycles() -> Result<()> {
    use headless_core::config::settings::{HostValue, Hosts, RuntimeSettings};
    for invalid in [
        "hosts: []",
        "hosts: {1: 192.0.2.1}",
        "hosts: {a.test: 123}",
        "hosts: {a.test: true}",
        "hosts: {a.test: null}",
        "hosts: {a.test: []}",
        "hosts: {a.test: [192.0.2.1, true]}",
        "hosts: {a.test: [alias.test]}",
        "hosts: {'': 192.0.2.1}",
        "hosts: {'bad..test': 192.0.2.1}",
        "hosts: {'bad.*part.test': 192.0.2.1}",
        "hosts: {'a.+.test': 192.0.2.1}",
        "hosts: {'a.test.': 192.0.2.1}",
        "hosts: {'中文.test': 192.0.2.1}",
        "hosts: {a.test: bad/value}",
        "hosts: {a.test: 'bad alias.test'}",
        "hosts: {a.test: b.test, b.test: a.test}",
        "hosts: {a.test: a.test}",
        "hosts: {'*.test': a.test}",
        "hosts: {'+.test': b.example, '*.example': a.test}",
        "hosts: {a.test: 192.0.2.1, A.TEST: 192.0.2.2}",
        "dns: {use-system-hosts: 'false'}",
    ] {
        assert!(
            serde_yaml_ng::from_str::<RuntimeSettings>(invalid).is_err(),
            "{invalid}"
        );
    }
    let too_many = Hosts(
        (0..1025)
            .map(|i| (format!("h{i}.test"), HostValue::Single("192.0.2.1".into())))
            .collect(),
    );
    assert!(
        RuntimeSettings {
            hosts: Some(too_many),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    let addresses = Hosts([("a.test".into(), HostValue::Addresses(vec!["192.0.2.1".into(); 65]))].into());
    assert!(
        RuntimeSettings {
            hosts: Some(addresses),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    for valid in [
        "hosts: {'.example.test': 192.0.2.1}",
        "hosts: {'+.example.test': 192.0.2.1}",
        "hosts: {'a.*.test': 192.0.2.1}",
        "hosts: {a.test: '::ffff:192.0.2.1'}",
    ] {
        serde_yaml_ng::from_str::<RuntimeSettings>(valid)?;
    }
    Ok(())
}

#[test]
fn remaining_authoritative_settings_validate_and_enforce_authority() -> Result<()> {
    for valid in [
        "bind-address: '*'",
        "bind-address: '127.0.0.1'",
        "bind-address: '0.0.0.0'",
        "bind-address: '::1'",
        "bind-address: localhost",
        "bind-address: ''",
    ] {
        let settings: RuntimeSettings = serde_yaml_ng::from_str(valid)?;
        assert!(settings.validate().is_ok(), "{valid}");
    }
    for invalid in [
        "bind-address: '127.0.0.1:8080'",
        "bind-address: '256.0.0.1'",
        "bind-address: 'has space'",
    ] {
        let settings: RuntimeSettings = serde_yaml_ng::from_str(invalid)?;
        assert!(settings.validate().is_err(), "{invalid}");
    }

    for valid in [
        "authentication: ['admin:123456']",
        "authentication: ['alice:p1', 'bob:p2']",
        "authentication: []",
    ] {
        let settings: RuntimeSettings = serde_yaml_ng::from_str(valid)?;
        assert!(settings.validate().is_ok(), "{valid}");
    }
    for invalid in [
        "authentication: ['no_colon']",
        "authentication: [':no_user']",
        "authentication: ['']",
    ] {
        let settings: RuntimeSettings = serde_yaml_ng::from_str(invalid)?;
        assert!(settings.validate().is_err(), "{invalid}");
    }

    for valid in [
        "skip-auth-prefixes: ['127.0.0.1/8', '::1/128']",
        "lan-allowed-ips: ['192.168.0.0/16', '10.0.0.1']",
        "lan-disallowed-ips: ['192.168.1.100/32']",
        "skip-auth-prefixes: []\nlan-allowed-ips: []\nlan-disallowed-ips: []",
    ] {
        let settings: RuntimeSettings = serde_yaml_ng::from_str(valid)?;
        assert!(settings.validate().is_ok(), "{valid}");
    }
    for invalid in [
        "skip-auth-prefixes: ['invalid']",
        "lan-allowed-ips: ['192.168.0.1/33']",
        "lan-disallowed-ips: ['::1/129']",
        "lan-allowed-ips: ['']",
    ] {
        let settings: RuntimeSettings = serde_yaml_ng::from_str(invalid)?;
        assert!(settings.validate().is_err(), "{invalid}");
    }

    for valid in [
        "inbound-tfo: true\ninbound-mptcp: false\nsniffing: true",
        "inbound-tfo: false\ninbound-mptcp: true\nsniffing: false",
    ] {
        let settings: RuntimeSettings = serde_yaml_ng::from_str(valid)?;
        assert!(settings.validate().is_ok(), "{valid}");
    }

    let base = parse(
        "bind-address: 127.0.0.1\nauthentication: ['old:pass']\nskip-auth-prefixes: ['10.0.0.0/8']\nlan-allowed-ips: ['10.0.0.0/8']\nlan-disallowed-ips: ['10.0.0.1/32']\ninbound-tfo: true\ninbound-mptcp: true\nsniffing: true\ncustom: retained",
    )?;

    let runtime: RuntimeSettings = serde_yaml_ng::from_str(
        "bind-address: '*'\nauthentication: ['admin:new_pass']\nskip-auth-prefixes: ['192.168.0.0/16']\nlan-allowed-ips: ['192.168.0.0/16']\nlan-disallowed-ips: []\ninbound-tfo: false\ninbound-mptcp: false\nsniffing: false",
    )?;
    let applied = runtime.enforce(base.clone())?;
    assert_eq!(applied["bind-address"].as_str(), Some("*"));
    assert_eq!(applied["authentication"][0].as_str(), Some("admin:new_pass"));
    assert_eq!(applied["skip-auth-prefixes"][0].as_str(), Some("192.168.0.0/16"));
    assert_eq!(applied["lan-allowed-ips"][0].as_str(), Some("192.168.0.0/16"));
    assert!(applied["lan-disallowed-ips"].as_sequence().unwrap().is_empty());
    assert_eq!(applied["inbound-tfo"].as_bool(), Some(false));
    assert_eq!(applied["inbound-mptcp"].as_bool(), Some(false));
    assert_eq!(applied["sniffing"].as_bool(), Some(false));
    assert_eq!(applied["custom"].as_str(), Some("retained"));

    let changed = runtime.overridden_fields(&base, &applied)?;
    for key in [
        "bind-address",
        "authentication",
        "skip-auth-prefixes",
        "lan-allowed-ips",
        "lan-disallowed-ips",
        "inbound-tfo",
        "inbound-mptcp",
        "sniffing",
    ] {
        assert!(changed.contains(&key.to_owned()), "missing changed key {key}");
    }

    let clear_auth: RuntimeSettings = serde_yaml_ng::from_str("authentication: []")?;
    let cleared = clear_auth.enforce(base.clone())?;
    assert!(cleared["authentication"].as_sequence().unwrap().is_empty());
    assert_eq!(cleared["inbound-tfo"].as_bool(), Some(true));

    let empty = RuntimeSettings::default();
    let inherited = empty.enforce(base.clone())?;
    assert_eq!(inherited["bind-address"].as_str(), Some("127.0.0.1"));
    assert_eq!(inherited["authentication"][0].as_str(), Some("old:pass"));
    assert_eq!(inherited["inbound-tfo"].as_bool(), Some(true));
    assert_eq!(inherited["sniffing"].as_bool(), Some(true));
    assert!(empty.overridden_fields(&base, &inherited)?.is_empty());

    Ok(())
}

#[test]
fn geo_lifecycle_and_resource_settings_helpers_and_mode_derivation() -> Result<()> {
    use headless_core::config::settings::GeoUrls;

    // Test canonical key and asset mapping
    for (asset, expected_key) in [
        ("geoip.dat", "geoip"),
        ("geosite.dat", "geosite"),
        ("GeoSite.dat", "geosite"),
        ("Country.mmdb", "mmdb"),
        ("geoip.metadb", "mmdb"),
        ("ASN.mmdb", "asn"),
    ] {
        assert_eq!(GeoUrls::key_for_asset(asset), Some(expected_key));
    }
    assert_eq!(GeoUrls::key_for_asset("unknown.dat"), None);

    for (key, expected_asset) in [
        ("geoip", "geoip.dat"),
        ("geosite", "geosite.dat"),
        ("mmdb", "Country.mmdb"),
        ("asn", "ASN.mmdb"),
    ] {
        assert_eq!(GeoUrls::asset_for_key(key), Some(expected_asset));
    }
    assert_eq!(GeoUrls::asset_for_key("unknown"), None);

    // Test url_for_asset and is_empty
    let empty_urls = GeoUrls::default();
    assert!(empty_urls.is_empty());
    assert_eq!(empty_urls.url_for_asset("geoip.dat"), None);

    let configured_urls = GeoUrls {
        geoip: Some("https://example.org/geoip.dat".into()),
        geosite: Some("https://example.org/geosite.dat".into()),
        mmdb: Some("https://example.org/Country.mmdb".into()),
        asn: Some("https://example.org/ASN.mmdb".into()),
    };
    assert!(!configured_urls.is_empty());
    assert_eq!(
        configured_urls.url_for_asset("geoip.dat"),
        Some("https://example.org/geoip.dat")
    );
    assert_eq!(
        configured_urls.url_for_asset("geosite.dat"),
        Some("https://example.org/geosite.dat")
    );
    assert_eq!(
        configured_urls.url_for_asset("GeoSite.dat"),
        Some("https://example.org/geosite.dat")
    );
    assert_eq!(
        configured_urls.url_for_asset("Country.mmdb"),
        Some("https://example.org/Country.mmdb")
    );
    assert_eq!(
        configured_urls.url_for_asset("geoip.metadb"),
        Some("https://example.org/Country.mmdb")
    );
    assert_eq!(
        configured_urls.url_for_asset("ASN.mmdb"),
        Some("https://example.org/ASN.mmdb")
    );
    assert_eq!(configured_urls.url_for_asset("unknown.dat"), None);

    // Test RuntimeSettings expected_geo_assets
    let mut runtime = RuntimeSettings::default();
    assert_eq!(runtime.expected_geo_assets(), None);

    runtime.geodata_mode = Some(true); // DAT mode
    assert_eq!(runtime.expected_geo_assets(), Some(&["geoip.dat", "geosite.dat"][..]));

    runtime.geodata_mode = Some(false); // MMDB mode
    assert_eq!(
        runtime.expected_geo_assets(),
        Some(&["Country.mmdb", "ASN.mmdb", "geoip.metadb"][..])
    );

    // Test overridden_fields across all 9 Geo fields
    let before: serde_yaml_ng::Mapping = serde_yaml_ng::from_str(
        "geodata-mode: false\ngeodata-loader: standard\ngeosite-matcher: succinct\ngeo-auto-update: false\ngeo-update-interval: 24\ngeox-url: {geoip: 'https://old.invalid/ip', geosite: 'https://old.invalid/site', mmdb: 'https://old.invalid/db', asn: 'https://old.invalid/asn'}",
    )?;
    let settings_yaml = "geodata-mode: true\ngeodata-loader: memconservative\ngeosite-matcher: mph\ngeo-auto-update: true\ngeo-update-interval: 48\ngeox-url: {geoip: 'https://new.invalid/ip', geosite: 'https://new.invalid/site', mmdb: 'https://new.invalid/db', asn: 'https://new.invalid/asn'}";
    let candidate: RuntimeSettings = serde_yaml_ng::from_str(settings_yaml)?;
    let after = candidate.enforce(before.clone())?;

    let changed = candidate.overridden_fields(&before, &after)?;
    for expected in [
        "geodata-mode",
        "geodata-loader",
        "geosite-matcher",
        "geo-auto-update",
        "geo-update-interval",
        "geox-url.geoip",
        "geox-url.geosite",
        "geox-url.mmdb",
        "geox-url.asn",
    ] {
        assert!(
            changed.contains(&expected.to_owned()),
            "missing changed field {expected} in {changed:?}"
        );
    }

    Ok(())
}

