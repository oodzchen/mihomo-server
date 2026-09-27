use anyhow::Result;
use headless_core::{
    config::settings::RuntimeSettings,
    enhance::tun::{ensure_fake_ip_range6, use_tun},
};
use serde_yaml_ng::Mapping;

fn mapping(yaml: &str) -> Result<Mapping> {
    Ok(serde_yaml_ng::from_str(yaml)?)
}

#[test]
fn enabled_tun_derives_fake_ip_defaults_from_top_level_ipv6() -> Result<()> {
    for ipv6 in [false, true] {
        let config = use_tun(
            mapping(&format!("ipv6: {ipv6}\ntun: {{mtu: 1500}}\ncustom: retained"))?,
            true,
        );
        assert_eq!(config["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(config["tun"]["mtu"].as_u64(), Some(1500));
        assert_eq!(config["dns"]["enable"].as_bool(), Some(true));
        assert_eq!(config["dns"]["ipv6"].as_bool(), Some(ipv6));
        assert_eq!(config["dns"]["enhanced-mode"].as_str(), Some("fake-ip"));
        assert_eq!(config["dns"]["fake-ip-range"].as_str(), Some("198.18.0.1/16"));
        assert_eq!(config["dns"].as_mapping().unwrap().contains_key("fake-ip-range6"), ipv6);
        assert_eq!(config["custom"].as_str(), Some("retained"));
    }
    Ok(())
}

#[test]
fn disabled_tun_and_redir_host_preserve_dns_and_existing_ranges() -> Result<()> {
    let source = mapping("ipv6: true\ndns: {enable: false, ipv6: false, enhanced-mode: redir-host}\ntun: {mtu: 1500}")?;
    for enable in [false, true] {
        let config = use_tun(source.clone(), enable);
        assert_eq!(config["dns"], source["dns"]);
        assert_eq!(config["tun"]["enable"].as_bool(), Some(enable));
    }
    let source = mapping(
        "ipv6: true\ndns: {fake-ip-range: 198.19.0.1/16, fake-ip-range6: '2001:db8::/64', nameserver: [1.1.1.1]}",
    )?;
    let config = use_tun(source.clone(), true);
    for field in ["fake-ip-range", "fake-ip-range6", "nameserver"] {
        assert_eq!(config["dns"][field], source["dns"][field]);
    }
    Ok(())
}

#[test]
fn malformed_sections_are_replaced_but_present_mode_and_ranges_follow_upstream() -> Result<()> {
    for yaml in ["tun: null\ndns: []", "tun: false\ndns: text"] {
        let config = use_tun(mapping(yaml)?, true);
        assert_eq!(config["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(config["dns"]["enhanced-mode"].as_str(), Some("fake-ip"));
    }
    let source = mapping("dns: {enhanced-mode: null, fake-ip-range: null, fake-ip-range6: null}")?;
    let config = use_tun(source.clone(), true);
    for field in ["enhanced-mode", "fake-ip-range", "fake-ip-range6"] {
        assert_eq!(config["dns"][field], source["dns"][field]);
    }
    Ok(())
}

#[test]
fn dns_page_repairs_missing_null_and_blank_ipv6_ranges_only_for_fake_ip() -> Result<()> {
    for range in [
        "",
        "\n  fake-ip-range6: null",
        "\n  fake-ip-range6: '  '",
        "\n  fake-ip-range6: []",
    ] {
        let mut config = mapping(&format!("dns:\n  ipv6: true{range}"))?;
        ensure_fake_ip_range6(&mut config);
        assert_eq!(config["dns"]["fake-ip-range6"].as_str(), Some("2001:2::0/64"));
    }
    for yaml in [
        "dns: {ipv6: false}",
        "dns: {ipv6: true, enhanced-mode: redir-host}",
        "dns: null",
        "dns: {ipv6: true, fake-ip-range6: '2001:db8::/64'}",
    ] {
        let mut config = mapping(yaml)?;
        let prior = config.clone();
        ensure_fake_ip_range6(&mut config);
        assert_eq!(config, prior);
    }
    Ok(())
}

#[test]
fn settings_prepare_orders_top_level_tun_and_dns_before_final_authority() -> Result<()> {
    let settings: RuntimeSettings = serde_yaml_ng::from_str(
        "ipv6: true\ntun: {enable: true}\ndns: {ipv6: true, enhanced-mode: redir-host, nameserver: [1.1.1.1]}",
    )?;
    let config = settings.prepare(mapping("ipv6: false\ndns: {enable: false}")?)?;
    // TUN derives fake-IP first, then the DNS page replaces its mode.
    assert_eq!(config["dns"]["enable"].as_bool(), Some(true));
    assert_eq!(config["dns"]["enhanced-mode"].as_str(), Some("redir-host"));
    assert_eq!(config["dns"]["fake-ip-range6"].as_str(), Some("2001:2::0/64"));
    let settings: RuntimeSettings =
        serde_yaml_ng::from_str("tun: {enable: true}\ndns: {ipv6: true, enhanced-mode: fake-ip}")?;
    let config = settings.prepare(mapping("dns: {enable: false, enhanced-mode: redir-host}")?)?;
    // A late DNS-page mode change never reruns TUN's enable decision.
    assert_eq!(config["dns"]["enable"].as_bool(), Some(false));
    assert_eq!(config["dns"]["fake-ip-range6"].as_str(), Some("2001:2::0/64"));
    assert!(!config["dns"].as_mapping().unwrap().contains_key("fake-ip-range"));
    let mut manual = config;
    manual
        .get_mut("dns")
        .unwrap()
        .as_mapping_mut()
        .unwrap()
        .remove("enable");
    let final_config = settings.enforce(manual)?;
    assert!(!final_config["dns"].as_mapping().unwrap().contains_key("enable"));
    Ok(())
}

#[test]
fn inheritance_does_not_derive_or_mutate_source_tun() -> Result<()> {
    let source = mapping("tun: {enable: true}\ndns: {enable: false}")?;
    for yaml in ["{}", "tun: {}", "tun: {mtu: 1500}", "tun: {enable: null}"] {
        let settings: RuntimeSettings = serde_yaml_ng::from_str(yaml)?;
        let config = settings.prepare(source.clone())?;
        assert_eq!(config["dns"], source["dns"]);
    }
    let settings: RuntimeSettings = serde_yaml_ng::from_str("tun: {enable: false}")?;
    let config = settings.prepare(source.clone())?;
    assert_eq!(config["dns"], source["dns"]);
    assert_eq!(config["tun"]["enable"].as_bool(), Some(false));
    Ok(())
}
