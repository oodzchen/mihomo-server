//! Domestic bypass for an enabled TUN: CN destinations leave through the
//! host's own routes instead of entering Mihomo.
//!
//! Routing can only see addresses, so two pieces work together. DNS answers
//! domains in `geosite:cn` with real addresses (not fake IPs) from domestic
//! resolvers, and the TUN routes exclude CN address ranges. A CN domain that
//! resolves into those ranges is then never seen by Mihomo; anything else
//! still enters the TUN and follows the rules.
use serde_yaml_ng::{Mapping, Value};

/// Domestic resolvers for `geosite:cn`, so CDNs answer with nearby addresses.
pub const DOMESTIC_NAMESERVERS: [&str; 2] = ["223.5.5.5", "119.29.29.29"];
const GEOSITE: &str = "geosite:cn";

/// Whether `config` enables a TUN, the only case the bypass applies to.
pub fn tun_enabled(config: &Mapping) -> bool {
    config
        .get("tun")
        .and_then(|tun| tun.get("enable"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// Add `ranges` (CIDR text) to `tun.route-exclude-address` and keep
/// `geosite:cn` out of fake IPs, resolved by [`DOMESTIC_NAMESERVERS`] unless
/// the configuration already names resolvers for it. Existing entries stay.
pub fn bypass_domestic(mut config: Mapping, ranges: &[String]) -> Mapping {
    if !tun_enabled(&config) {
        return config;
    }
    if let Some(tun) = config.get_mut("tun").and_then(Value::as_mapping_mut) {
        let mut excluded = tun
            .get("route-exclude-address")
            .and_then(Value::as_sequence)
            .cloned()
            .unwrap_or_default();
        excluded.extend(
            ranges
                .iter()
                .map(|range| Value::from(range.as_str()))
                .filter(|range| !excluded.contains(range))
                .collect::<Vec<_>>(),
        );
        tun.insert("route-exclude-address".into(), excluded.into());
    }
    if let Some(dns) = config.get_mut("dns").and_then(Value::as_mapping_mut) {
        // A whitelist names the domains that do get fake IPs; others already get real ones.
        if dns
            .get("fake-ip-filter-mode")
            .and_then(Value::as_str)
            .unwrap_or("blacklist")
            == "blacklist"
        {
            let mut filter = dns
                .get("fake-ip-filter")
                .and_then(Value::as_sequence)
                .cloned()
                .unwrap_or_default();
            if !filter.iter().any(|entry| entry.as_str() == Some(GEOSITE)) {
                filter.push(GEOSITE.into());
            }
            dns.insert("fake-ip-filter".into(), filter.into());
        }
        let mut policy = dns
            .get("nameserver-policy")
            .and_then(Value::as_mapping)
            .cloned()
            .unwrap_or_default();
        policy
            .entry(GEOSITE.into())
            .or_insert_with(|| DOMESTIC_NAMESERVERS.iter().copied().map(Value::from).collect());
        dns.insert("nameserver-policy".into(), policy.into());
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(yaml: &str) -> Mapping {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    #[test]
    fn excludes_ranges_and_resolves_cn_domains_for_real() {
        let ranges = vec!["1.0.1.0/24".to_owned(), "10.0.0.0/8".to_owned()];
        let result = bypass_domestic(
            config(
                "tun: {enable: true, route-exclude-address: [10.0.0.0/8]}\n\
                 dns: {enhanced-mode: fake-ip, fake-ip-filter: ['*.lan'], \
                 nameserver-policy: {'+.corp': 10.1.1.1}}",
            ),
            &ranges,
        );
        assert_eq!(
            result,
            config(
                "tun: {enable: true, route-exclude-address: [10.0.0.0/8, 1.0.1.0/24]}\n\
                 dns: {enhanced-mode: fake-ip, fake-ip-filter: ['*.lan', 'geosite:cn'], \
                 nameserver-policy: {'+.corp': 10.1.1.1, 'geosite:cn': [223.5.5.5, 119.29.29.29]}}",
            )
        );
        // Idempotent, and a configured resolver for geosite:cn wins.
        assert_eq!(bypass_domestic(result.clone(), &ranges), result);
        let own = bypass_domestic(
            config("tun: {enable: true}\ndns: {nameserver-policy: {'geosite:cn': 1.2.4.8}}"),
            &ranges,
        );
        assert_eq!(own["dns"]["nameserver-policy"]["geosite:cn"], Value::from("1.2.4.8"));
    }

    #[test]
    fn leaves_whitelists_and_disabled_tun_alone() {
        let ranges = vec!["1.0.1.0/24".to_owned()];
        let whitelist = bypass_domestic(
            config("tun: {enable: true}\ndns: {fake-ip-filter-mode: whitelist, fake-ip-filter: [+.google.com]}"),
            &ranges,
        );
        assert_eq!(
            whitelist["dns"]["fake-ip-filter"],
            Value::from(vec![Value::from("+.google.com")])
        );
        let off = config("tun: {enable: false}\ndns: {enhanced-mode: fake-ip}");
        assert_eq!(bypass_domestic(off.clone(), &ranges), off);
    }
}
