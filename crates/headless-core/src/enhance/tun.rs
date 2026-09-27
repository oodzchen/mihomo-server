//! Pure portion of upstream enhance/tun.rs; host DNS actions are not migrated.
use serde_yaml_ng::{Mapping, Value};

pub fn use_tun(mut config: Mapping, enable: bool) -> Mapping {
    let mut tun = config
        .get("tun")
        .and_then(Value::as_mapping)
        .cloned()
        .unwrap_or_default();
    if enable {
        let mut dns = config
            .get("dns")
            .and_then(Value::as_mapping)
            .cloned()
            .unwrap_or_default();
        let ipv6 = config.get("ipv6").and_then(Value::as_bool).unwrap_or(false);
        let mode = dns.get("enhanced-mode").and_then(Value::as_str).unwrap_or("fake-ip");
        if mode == "fake-ip" || !dns.contains_key("enhanced-mode") {
            dns.insert("enable".into(), true.into());
            dns.insert("ipv6".into(), ipv6.into());
            dns.entry(Value::from("enhanced-mode"))
                .or_insert_with(|| "fake-ip".into());
            dns.entry(Value::from("fake-ip-range"))
                .or_insert_with(|| "198.18.0.1/16".into());
            if ipv6 {
                dns.entry(Value::from("fake-ip-range6"))
                    .or_insert_with(|| "2001:2::0/64".into());
            }
        }
        config.insert("dns".into(), dns.into());
    }
    tun.insert("enable".into(), enable.into());
    config.insert("tun".into(), tun.into());
    config
}

/// Called after the DNS settings page, before manual enhancement stages.
pub fn ensure_fake_ip_range6(config: &mut Mapping) {
    if let Some(dns) = config.get_mut("dns").and_then(Value::as_mapping_mut)
        && dns.get("ipv6").and_then(Value::as_bool) == Some(true)
        && dns.get("enhanced-mode").and_then(Value::as_str).unwrap_or("fake-ip") == "fake-ip"
        && dns
            .get("fake-ip-range6")
            .and_then(Value::as_str)
            .is_none_or(|s| s.trim().is_empty())
    {
        dns.insert("fake-ip-range6".into(), "2001:2::0/64".into());
    }
}
