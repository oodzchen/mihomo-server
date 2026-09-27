//! Final pure stages extracted from upstream enhance/mod.rs.
//! Run after all manual enhancements and authoritative settings, before validation.
use std::collections::HashSet;

use serde_yaml_ng::{Mapping, Value};

use super::field::use_sort;

pub fn finalize(config: Mapping) -> Mapping {
    use_sort(cleanup_proxy_groups(ensure_lan_bind_address(config)))
}

fn is_loopback_bind_address(addr: &str) -> bool {
    let addr = addr.trim();
    let addr = addr
        .strip_prefix('[')
        .and_then(|addr| addr.strip_suffix(']'))
        .unwrap_or(addr);

    addr.eq_ignore_ascii_case("localhost")
        || addr.parse::<std::net::IpAddr>().is_ok_and(|addr| addr.is_loopback())
        || is_ipv4_shorthand_loopback(addr)
}

fn is_ipv4_shorthand_loopback(addr: &str) -> bool {
    let parts = addr.split('.').map(str::parse::<u32>).collect::<Result<Vec<_>, _>>();

    let Ok(parts) = parts else {
        return false;
    };

    match parts.as_slice() {
        [first, rest] => *first == 127 && *rest <= 0x00ff_ffff,
        [first, second, rest] => *first == 127 && *second <= 0xff && *rest <= 0xffff,
        [first, second, third, fourth] => *first == 127 && *second <= 0xff && *third <= 0xff && *fourth <= 0xff,
        _ => false,
    }
}

fn ensure_lan_bind_address(mut config: Mapping) -> Mapping {
    let allow_lan = config.get("allow-lan").and_then(Value::as_bool).unwrap_or(false);

    if allow_lan
        && config
            .get("bind-address")
            .and_then(Value::as_str)
            .is_some_and(is_loopback_bind_address)
    {
        config.insert(Value::from("bind-address"), Value::from("*"));
    }

    config
}

fn cleanup_proxy_groups(mut config: Mapping) -> Mapping {
    // built-in proxies docs: https://wiki.metacubex.one/config/proxies/built-in
    const BUILTIN_POLICIES: &[&str] = &["DIRECT", "REJECT", "REJECT-DROP", "PASS", "PASS-RULE"];

    let proxy_names = config
        .get("proxies")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|item| match item {
                    Value::Mapping(map) => map.get("name").and_then(Value::as_str).map(str::to_owned),
                    Value::String(name) => Some(name.to_owned()),
                    _ => None,
                })
                .collect::<HashSet<String>>()
        })
        .unwrap_or_default();

    let group_names = config
        .get("proxy-groups")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|item| {
                    item.as_mapping()
                        .and_then(|map| map.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect::<HashSet<String>>()
        })
        .unwrap_or_default();

    let provider_names = config
        .get("proxy-providers")
        .and_then(Value::as_mapping)
        .map(|map| {
            map.keys()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<HashSet<String>>()
        })
        .unwrap_or_default();

    let mut allowed_names = proxy_names;
    allowed_names.extend(group_names);
    allowed_names.extend(provider_names.iter().cloned());
    allowed_names.extend(BUILTIN_POLICIES.iter().map(|p| (*p).to_owned()));

    if let Some(Value::Sequence(groups)) = config.get_mut("proxy-groups") {
        for group in groups {
            if let Some(group_map) = group.as_mapping_mut() {
                let mut has_valid_provider = false;

                if let Some(Value::Sequence(uses)) = group_map.get_mut("use") {
                    uses.retain(|provider| match provider {
                        Value::String(name) => {
                            let exists = provider_names.contains(name.as_str());
                            has_valid_provider = has_valid_provider || exists;
                            exists
                        }
                        _ => false,
                    });
                }

                if let Some(Value::Sequence(proxies)) = group_map.get_mut("proxies") {
                    proxies.retain(|proxy| match proxy {
                        Value::String(name) => allowed_names.contains(name.as_str()) || has_valid_provider,
                        _ => true,
                    });
                }
            }
        }
    }

    config
}
