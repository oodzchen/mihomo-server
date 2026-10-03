use anyhow::Result;
use headless_core::{
    config::settings::{DnsSettings, RuntimeSettings},
    enhance::isolation::{Isolation, SLOTS},
};
use serde_yaml_ng::Mapping;

fn mapping(yaml: &str) -> Result<Mapping> {
    Ok(serde_yaml_ng::from_str(yaml)?)
}

const SUBSCRIPTION: &str = "
mixed-port: 7890
port: 7891
socks-port: 7892
redir-port: 0
custom: retained
dns:
  enable: true
  listen: 0.0.0.0:1053
  fake-ip-range: 198.18.0.1/16
tun:
  enable: true
  stack: mixed
  device: Meta
  auto-route: true
  auto-redirect: true
  include-uid-range: ['0:65535']
  exclude-uid: [7]
";

#[test]
fn slot_values_are_unique_and_within_host_limits() -> Result<()> {
    assert_eq!(Isolation::new(1000, 0)?.management_port(), 9090);
    assert_eq!(Isolation::new(1000, 0)?.mixed_port(), 7890);
    assert_eq!(Isolation::new(1000, 0)?.dns_listen(), "127.0.0.1:1053");
    assert_eq!(Isolation::new(1000, 1)?.management_port(), 20010);
    let mut ports = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    for slot in 0..SLOTS {
        let isolation = Isolation::new(4_294_967_294, slot)?;
        for port in [
            isolation.management_port(),
            isolation.mixed_port(),
            isolation.dns_listen().rsplit(':').next().unwrap().parse()?,
        ] {
            assert!(ports.insert(port), "duplicate port {port}");
        }
        assert!(isolation.tun_device().len() <= 15);
        assert!(isolation.rule_index() + 31 < 32766);
        assert!(seen.insert((
            isolation.management_port(),
            isolation.mixed_port(),
            isolation.rule_index(),
            isolation.table_index(),
            isolation.fake_ip_range(),
            isolation.tun_inet6_address(),
        )));
        let range = isolation.fake_ip_range();
        let (address, prefix) = range.split_once('/').unwrap();
        let octets = address.parse::<std::net::Ipv4Addr>()?.octets();
        assert!(octets[0] == 198 && octets[1] == 19, "{address}");
        assert_eq!(prefix, "22");
    }
    assert_eq!(Isolation::new(1000, 0)?.fake_ip_range(), "198.19.0.1/22");
    assert_eq!(Isolation::new(1000, 1)?.fake_ip_range(), "198.19.4.1/22");
    assert_eq!(Isolation::new(1000, SLOTS - 1)?.fake_ip_range(), "198.19.252.1/22");
    assert!(Isolation::new(1000, SLOTS).is_err());
    Ok(())
}

#[test]
fn subscription_listeners_and_tun_move_into_the_user_slot() -> Result<()> {
    let isolation = Isolation::new(1001, 3)?;
    let (config, changed) = isolation.apply(mapping(SUBSCRIPTION)?, &RuntimeSettings::default());
    assert_eq!(config["mixed-port"].as_u64(), Some(20031));
    assert!(!config.contains_key("port") && !config.contains_key("socks-port"));
    assert_eq!(config["redir-port"].as_u64(), Some(0));
    assert_eq!(config["custom"].as_str(), Some("retained"));
    assert_eq!(config["dns"]["enable"].as_bool(), Some(true));
    assert_eq!(config["dns"]["listen"].as_str(), Some("127.0.0.1:20032"));
    assert_eq!(config["dns"]["fake-ip-range"].as_str(), Some("198.19.12.1/22"));
    assert_eq!(config["dns"]["fake-ip-range6"].as_str(), Some("2001:2:0:3::1/64"));
    let tun = &config["tun"];
    assert_eq!(tun["enable"].as_bool(), Some(true));
    assert_eq!(tun["stack"].as_str(), Some("mixed"));
    assert_eq!(tun["device"].as_str(), Some("ms1001"));
    assert_eq!(tun["include-uid"].as_sequence().map(Vec::len), Some(1));
    assert_eq!(tun["include-uid"][0].as_u64(), Some(1001));
    assert!(tun.get("include-uid-range").is_none());
    assert_eq!(tun["exclude-uid"][0].as_u64(), Some(7));
    assert_eq!(tun["iproute2-table-index"].as_u64(), Some(10003));
    assert_eq!(tun["iproute2-rule-index"].as_u64(), Some(10096));
    assert_eq!(tun["inet6-address"][0].as_str(), Some("fdfe:dcba:9876:3::1/126"));
    assert_eq!(tun["auto-redirect"].as_bool(), Some(false));
    for field in [
        "mixed-port",
        "port",
        "socks-port",
        "dns.listen",
        "tun.device",
        "tun.auto-redirect",
    ] {
        assert!(
            changed.iter().any(|item| item == field),
            "{field} missing from {changed:?}"
        );
    }
    assert!(!changed.iter().any(|item| item == "redir-port"));

    // Applying twice is stable, so conformance is a no-change check.
    let (again, changed) = isolation.apply(config.clone(), &RuntimeSettings::default());
    assert_eq!(again, config);
    assert!(changed.is_empty(), "{changed:?}");
    Ok(())
}

#[test]
fn settings_page_listeners_are_kept() -> Result<()> {
    let runtime = RuntimeSettings {
        mixed_port: Some(7890),
        socks_port: Some(7892),
        dns: Some(DnsSettings {
            listen: Some("127.0.0.1:5353".into()),
            ..DnsSettings::default()
        }),
        ..RuntimeSettings::default()
    };
    let enforced = runtime.enforce(mapping(SUBSCRIPTION)?)?;
    let (config, changed) = Isolation::new(1001, 3)?.apply(enforced, &runtime);
    assert_eq!(config["mixed-port"].as_u64(), Some(7890));
    assert_eq!(config["socks-port"].as_u64(), Some(7892));
    assert!(!config.contains_key("port"));
    assert_eq!(config["dns"]["listen"].as_str(), Some("127.0.0.1:5353"));
    assert!(!changed.iter().any(|item| item == "mixed-port" || item == "dns.listen"));
    Ok(())
}

#[test]
fn configs_without_tun_or_dns_only_get_a_private_mixed_port() -> Result<()> {
    let (config, changed) =
        Isolation::new(1000, 0)?.apply(mapping("mode: rule\nmixed-port: 7891")?, &RuntimeSettings::default());
    assert_eq!(config["mixed-port"].as_u64(), Some(7890));
    assert!(!config.contains_key("dns") && !config.contains_key("tun"));
    assert_eq!(changed, ["mixed-port"]);
    Ok(())
}

#[test]
fn tun_without_dns_section_still_gets_a_unique_interface_address() -> Result<()> {
    let (config, _) = Isolation::new(1000, 5)?.apply(mapping("tun: {enable: true}")?, &RuntimeSettings::default());
    assert_eq!(config["dns"]["fake-ip-range"].as_str(), Some("198.19.20.1/22"));
    assert!(config["dns"].get("enable").is_none());
    Ok(())
}

#[test]
fn enabled_tun_sniffs_pure_ip_connections() -> Result<()> {
    let isolation = Isolation::new(1000, 0)?;
    let runtime = RuntimeSettings::default();

    // No sniffer: a default one is added because shared resolvers bypass TUN DNS.
    let (config, changed) = isolation.apply(mapping("tun: {enable: true}")?, &runtime);
    assert_eq!(config["sniffer"]["enable"].as_bool(), Some(true));
    assert_eq!(config["sniffer"]["parse-pure-ip"].as_bool(), Some(true));
    assert!(config["sniffer"]["sniff"]["TLS"]["ports"].is_sequence());
    assert!(changed.iter().any(|field| field == "sniffer"));

    // A subscription's sniffer keeps its settings but must parse pure IPs.
    let (config, changed) = isolation.apply(
        mapping("tun: {enable: true}\nsniffer: {enable: true, parse-pure-ip: false, skip-domain: [example.com]}")?,
        &runtime,
    );
    assert_eq!(config["sniffer"]["parse-pure-ip"].as_bool(), Some(true));
    assert_eq!(config["sniffer"]["skip-domain"][0].as_str(), Some("example.com"));
    assert!(config["sniffer"].get("sniff").is_none());
    assert_eq!(changed.iter().filter(|field| field.starts_with("sniffer")).count(), 1);

    // An explicitly disabled sniffer and a disabled TUN are left alone.
    let (config, _) = isolation.apply(mapping("tun: {enable: true}\nsniffer: {enable: false}")?, &runtime);
    assert_eq!(config["sniffer"]["enable"].as_bool(), Some(false));
    assert!(config["sniffer"].get("parse-pure-ip").is_none());
    let (config, _) = isolation.apply(mapping("tun: {enable: false}")?, &runtime);
    assert!(!config.contains_key("sniffer"));
    Ok(())
}
