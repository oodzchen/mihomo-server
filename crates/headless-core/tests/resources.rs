use anyhow::Result;
use headless_core::config::resources::{
    AutoUpdateState, FileState, FreshnessState, GeoUpdatePolicy, Inventory,
    MAX_PROVIDERS, ProviderSettings, Resource, validate_resource_declarations,
};
use serde_yaml_ng::Mapping;
use std::path::PathBuf;

#[test]
fn test_validate_resource_declarations_valid() -> Result<()> {
    let yaml = r#"
proxy-providers:
  p1:
    type: http
    url: "https://example.com/subs"
    interval: 3600
    format: yaml
  p2:
    type: file
    path: "./local.yaml"
    interval: 0
  p3:
    type: inline
rule-providers:
  r1:
    type: http
    url: "https://example.com/rules"
    interval: 86400
    behavior: domain
    format: mrs
  r2:
    type: file
    path: "./rules.yaml"
    behavior: ipcidr
    format: text
"#;
    let config: Mapping = serde_yaml_ng::from_str(yaml)?;
    validate_resource_declarations(&config)?;
    Ok(())
}

#[test]
fn test_validate_resource_declarations_invalid_section() {
    let yaml = r#"
proxy-providers:
  - invalid_list_item
"#;
    let config: Mapping = serde_yaml_ng::from_str(yaml).unwrap();
    assert!(validate_resource_declarations(&config).is_err());
}

#[test]
fn test_validate_resource_declarations_provider_count_limit() {
    let mut config = Mapping::new();
    let mut providers = Mapping::new();
    for i in 0..=MAX_PROVIDERS {
        let mut p = Mapping::new();
        p.insert("type".into(), "file".into());
        providers.insert(format!("provider_{i}").into(), p.into());
    }
    config.insert("proxy-providers".into(), providers.into());
    let err = validate_resource_declarations(&config).unwrap_err();
    assert!(err.to_string().contains("exceeds 512 providers"));
}

#[test]
fn test_validate_resource_declarations_invalid_names() {
    let yaml = r#"
proxy-providers:
  "":
    type: file
"#;
    let config: Mapping = serde_yaml_ng::from_str(yaml).unwrap();
    assert!(validate_resource_declarations(&config).is_err());

    let long_name = "a".repeat(513);
    let yaml_long = format!("proxy-providers:\n  {long_name}:\n    type: file");
    let config_long: Mapping = serde_yaml_ng::from_str(&yaml_long).unwrap();
    assert!(validate_resource_declarations(&config_long).is_err());

    let yaml_ctrl = "proxy-providers:\n  \"bad\\nname\":\n    type: file";
    let config_ctrl: Mapping = serde_yaml_ng::from_str(yaml_ctrl).unwrap();
    assert!(validate_resource_declarations(&config_ctrl).is_err());
}

#[test]
fn test_validate_resource_declarations_invalid_fields() {
    let invalid_types = ["ftp", "custom", ""];
    for kind in invalid_types {
        let yaml = format!("proxy-providers:\n  p1:\n    type: {kind}");
        let config: Mapping = serde_yaml_ng::from_str(&yaml).unwrap();
        assert!(validate_resource_declarations(&config).is_err());
    }

    let invalid_intervals = ["-1", "2147483648", "invalid"];
    for interval in invalid_intervals {
        let yaml = format!("proxy-providers:\n  p1:\n    type: file\n    interval: {interval}");
        if let Ok(config) = serde_yaml_ng::from_str::<Mapping>(&yaml) {
            assert!(validate_resource_declarations(&config).is_err());
        }
    }

    let invalid_formats = ["xml", "tar", "binary"];
    for format in invalid_formats {
        let yaml = format!("proxy-providers:\n  p1:\n    type: file\n    format: {format}");
        let config: Mapping = serde_yaml_ng::from_str(&yaml).unwrap();
        assert!(validate_resource_declarations(&config).is_err());
    }

    let invalid_behaviors = ["other", "routing", "regex"];
    for behavior in invalid_behaviors {
        let yaml = format!("rule-providers:\n  r1:\n    type: file\n    behavior: {behavior}");
        let config: Mapping = serde_yaml_ng::from_str(&yaml).unwrap();
        assert!(validate_resource_declarations(&config).is_err());
    }
}

#[test]
fn test_provider_settings_validation() -> Result<()> {
    // Valid HTTP
    let http = ProviderSettings {
        provider_type: "http".into(),
        url: Some("https://example.com/subs.yaml".into()),
        path: Some("./subs.yaml".into()),
        interval: Some(3600),
        filter: Some("(?i)hk".into()),
        format: Some("yaml".into()),
        behavior: None,
    };
    http.validate()?;

    // Missing URL for HTTP
    let http_no_url = ProviderSettings {
        provider_type: "http".into(),
        url: None,
        path: None,
        interval: None,
        filter: None,
        format: None,
        behavior: None,
    };
    assert!(http_no_url.validate().is_err());

    // Bad scheme for HTTP
    let http_bad_scheme = ProviderSettings {
        provider_type: "http".into(),
        url: Some("ftp://example.com/file".into()),
        path: None,
        interval: None,
        filter: None,
        format: None,
        behavior: None,
    };
    assert!(http_bad_scheme.validate().is_err());

    // Valid File
    let file = ProviderSettings {
        provider_type: "file".into(),
        url: None,
        path: Some("./file.yaml".into()),
        interval: None,
        filter: None,
        format: Some("json".into()),
        behavior: Some("domain".into()),
    };
    file.validate()?;

    // Invalid interval
    let bad_interval = ProviderSettings {
        provider_type: "file".into(),
        url: None,
        path: None,
        interval: Some(i32::MAX as u64 + 1),
        filter: None,
        format: None,
        behavior: None,
    };
    assert!(bad_interval.validate().is_err());

    Ok(())
}

#[test]
fn test_geo_update_policy_evaluation() {
    // Stopped when core not running
    let p1 = GeoUpdatePolicy::evaluate(Some(true), Some(24), Some(true), Some(24), false, false);
    assert_eq!(p1.auto_update_state, AutoUpdateState::Stopped);
    assert!(!p1.mismatch);

    // Readback error
    let p2 = GeoUpdatePolicy::evaluate(Some(true), Some(24), None, None, true, true);
    assert_eq!(p2.auto_update_state, AutoUpdateState::Indeterminate);
    assert!(p2.readback_error);

    // Active
    let p3 = GeoUpdatePolicy::evaluate(Some(true), Some(24), Some(true), Some(24), true, false);
    assert_eq!(p3.auto_update_state, AutoUpdateState::Active);
    assert!(!p3.mismatch);

    // Disabled when interval <= 0
    let p4 = GeoUpdatePolicy::evaluate(Some(true), Some(0), Some(true), Some(0), true, false);
    assert_eq!(p4.auto_update_state, AutoUpdateState::Disabled);

    // Disabled when effective_enabled is false
    let p5 = GeoUpdatePolicy::evaluate(Some(true), Some(24), Some(false), Some(24), true, false);
    assert_eq!(p5.auto_update_state, AutoUpdateState::Disabled);
    assert!(p5.mismatch); // configured true, effective false

    // Fallback to configured when effective_enabled is None but effective interval is present
    let p6 = GeoUpdatePolicy::evaluate(Some(true), Some(48), None, Some(48), true, false);
    assert_eq!(p6.auto_update_state, AutoUpdateState::Active);

    // from_config_and_actual
    let mut config = Mapping::new();
    config.insert("geo-auto-update".into(), true.into());
    config.insert("geo-update-interval".into(), 12.into());
    let p7 = GeoUpdatePolicy::from_config_and_actual(&config, true, Some(true), Some(12), false);
    assert_eq!(p7.auto_update_state, AutoUpdateState::Active);
    assert_eq!(p7.configured_interval_hours, Some(12));
    assert_eq!(p7.effective_interval_hours, Some(12));
    assert!(!p7.mismatch);
}

#[test]
fn test_freshness_evaluation() {
    assert_eq!(
        FreshnessState::evaluate_age(100, 300),
        FreshnessState::Fresh
    );
    assert_eq!(
        FreshnessState::evaluate_age(300, 300),
        FreshnessState::Fresh
    );
    assert_eq!(
        FreshnessState::evaluate_age(301, 300),
        FreshnessState::Stale
    );
    assert_eq!(
        FreshnessState::evaluate_age(100, 0),
        FreshnessState::Indeterminate
    );
}

#[test]
fn test_inventory_serialization_roundtrip() -> Result<()> {
    let inventory = Inventory {
        data_dir: PathBuf::from("/data"),
        bundle_dir: Some(PathBuf::from("/bundle")),
        config_revision: Some("rev-123".into()),
        geo_update: GeoUpdatePolicy::evaluate(
            Some(true),
            Some(24),
            Some(true),
            Some(24),
            true,
            false,
        ),
        geo: vec![Resource {
            section: "geo".into(),
            name: "GeoIP.dat".into(),
            provider_type: None,
            path: Some("GeoIP.dat".into()),
            state: FileState::Available,
            bytes: Some(1024),
            modified_unix_seconds: Some(1700000000),
            age_seconds: Some(3600),
            freshness: FreshnessState::Fresh,
            conflict: false,
        }],
        providers: vec![Resource {
            section: "proxy-providers".into(),
            name: "provider1".into(),
            provider_type: Some("http".into()),
            path: Some("provider-cache/v1/hash.yaml".into()),
            state: FileState::Available,
            bytes: Some(2048),
            modified_unix_seconds: Some(1700000000),
            age_seconds: Some(1800),
            freshness: FreshnessState::Fresh,
            conflict: false,
        }],
    };

    let json = serde_json::to_string(&inventory)?;
    let parsed: Inventory = serde_json::from_str(&json)?;
    assert_eq!(inventory, parsed);

    let yaml = serde_yaml_ng::to_string(&inventory)?;
    let parsed_yaml: Inventory = serde_yaml_ng::from_str(&yaml)?;
    assert_eq!(inventory, parsed_yaml);
    Ok(())
}
