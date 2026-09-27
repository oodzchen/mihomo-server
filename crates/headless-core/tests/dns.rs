use anyhow::Result;
use headless_core::config::{
    dns::{DnsOverrideState, dns_override_source},
    settings::ServiceSettings,
};
use serde_yaml_ng::Mapping;

fn mapping(yaml: &str) -> Result<Mapping> {
    Ok(serde_yaml_ng::from_str(yaml)?)
}

#[test]
fn provider_source_detects_only_original_nonempty_resolver_and_policy_fields() -> Result<()> {
    for field in [
        "proxy-server-nameserver: [1.1.1.1]",
        "proxy-server-nameserver-policy: {example.org: 1.1.1.1}",
        "nameserver-policy: {example.org: [1.1.1.1]}",
    ] {
        let source = dns_override_source("one", &mapping(&format!("dns: {{{field}}}"))?)?.unwrap();
        assert_eq!(source.len(), 64);
        assert!(source.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(dns_override_source("one", &mapping(field)?)?.is_none());
    }
    for yaml in [
        "{}",
        "dns: null",
        "dns: {nameserver: [1.1.1.1]}",
        "dns: {proxy-server-nameserver: [], proxy-server-nameserver-policy: null, nameserver-policy: {}}",
        "dns: {nameserver-policy: '  ', proxy-server-nameserver: false}",
    ] {
        assert!(dns_override_source("one", &mapping(yaml)?)?.is_none());
    }
    Ok(())
}

#[test]
fn canonical_confirmation_is_bound_to_uid_source_and_session() -> Result<()> {
    let a = mapping("dns: {nameserver-policy: {a.example: 1.1.1.1, b.example: [8.8.8.8]}}")?;
    let reordered =
        mapping("dns:\n  nameserver-policy:\n    b.example: [8.8.8.8]\n    a.example: 1.1.1.1\nport: 7890")?;
    let source = dns_override_source("one", &a)?;
    assert_eq!(
        source.as_deref(),
        Some("8ef58598d6cf78da069e4eabed6fbf435ba5427718d3b54551db1ae8d0c9182c")
    );
    assert_eq!(source, dns_override_source("one", &reordered)?);
    assert!(DnsOverrideState::new("one", source.clone(), true, source.as_deref()).enabled);
    assert!(!DnsOverrideState::new("two", dns_override_source("two", &a)?, true, source.as_deref()).enabled);
    let changed = mapping("dns: {nameserver-policy: {a.example: 9.9.9.9, b.example: [8.8.8.8]}}")?;
    assert!(!DnsOverrideState::new("one", dns_override_source("one", &changed)?, true, source.as_deref()).enabled);
    assert!(!DnsOverrideState::new("one", source, true, None).enabled);
    assert!(DnsOverrideState::new("one", None, true, None).enabled);
    assert!(!DnsOverrideState::new("one", None, false, None).enabled);
    Ok(())
}

#[test]
fn preferences_are_strict_and_never_deserialize_confirmations() -> Result<()> {
    let settings: ServiceSettings =
        serde_yaml_ng::from_str("schema_version: 1\nprofile_dns: {one: {enabled: true}, two: {enabled: false}}")?;
    settings.validate()?;
    let serialized = serde_yaml_ng::to_string(&settings)?;
    assert!(!serialized.contains("confirmation"));
    assert_eq!(serde_yaml_ng::from_str::<ServiceSettings>(&serialized)?, settings);
    for yaml in [
        "schema_version: 1\nprofile_dns: {one: {enabled: true, confirmation: abc}}",
        "schema_version: 1\nprofile_dns: {one: {enabled: 'true'}}",
        "schema_version: 1\nprofile_dns: {one: {}}",
        "schema_version: 1\nprofile_dns: {'': {enabled: true}}",
    ] {
        assert!(
            serde_yaml_ng::from_str::<ServiceSettings>(yaml)
                .and_then(|s| s.validate().map_err(serde::de::Error::custom))
                .is_err()
        );
    }
    Ok(())
}
