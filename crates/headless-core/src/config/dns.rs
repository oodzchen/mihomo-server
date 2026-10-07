//! Provider DNS protection adapted from upstream config/dns.rs.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};

pub fn dns_override_source(uid: &str, config: &Mapping) -> Result<Option<String>> {
    let Some(dns) = config.get("dns").and_then(Value::as_mapping) else {
        return Ok(None);
    };
    let fields: Mapping = [
        "proxy-server-nameserver",
        "proxy-server-nameserver-policy",
        "nameserver-policy",
    ]
    .into_iter()
    .filter_map(|key| {
        let value = dns.get(key)?;
        let nonempty = match value {
            Value::Sequence(v) => !v.is_empty(),
            Value::Mapping(v) => !v.is_empty(),
            Value::String(v) => !v.trim().is_empty(),
            _ => false,
        };
        nonempty.then(|| (key.into(), value.clone()))
    })
    .collect();
    if fields.is_empty() {
        return Ok(None);
    }
    let mut canonical = serde_json::to_value(fields)?;
    canonical.sort_all_objects();
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    context.update(uid.as_bytes());
    context.update(&[0]);
    context.update(&serde_json::to_vec(&canonical)?);
    Ok(Some(
        context
            .finish()
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileDnsSettings {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DnsOverrideState {
    pub uid: String,
    pub source: Option<String>,
    pub requested: bool,
    pub enabled: bool,
}

impl DnsOverrideState {
    pub fn new(uid: &str, source: Option<String>, requested: bool, confirmation: Option<&str>) -> Self {
        let enabled = requested && (source.is_none() || source.as_deref() == confirmation);
        Self {
            uid: uid.into(),
            source,
            requested,
            enabled,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DnsOverrideOutcome {
    Applied { state: DnsOverrideState },
    ConfirmationRequired { source: String },
}

/// DNS fields whose servers may name the core's own listener.
const SERVER_LISTS: [&str; 5] = [
    "default-nameserver",
    "nameserver",
    "fallback",
    "proxy-server-nameserver",
    "direct-nameserver",
];
const SERVER_POLICIES: [&str; 2] = ["nameserver-policy", "proxy-server-nameserver-policy"];

/// Whether the nameserver entry `server` is the core's own DNS listener at
/// `listen`. Subscriptions use this to send node lookups through the listener
/// and the fake-IP filter it applies.
pub fn is_own_listener(server: &str, listen: &str) -> bool {
    if listen.trim().is_empty() {
        return false;
    }
    let (Some((_, address, _)), Some((listen_host, listen_port))) = (split_server(server), split_address(listen))
    else {
        return false;
    };
    let Some((host, port)) = split_address(address) else {
        return false;
    };
    let loopback =
        |host: &str| host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    let wildcard = listen_host.is_empty()
        || listen_host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_unspecified() || ip.is_loopback());
    port == listen_port && (host == listen_host || (wildcard && loopback(host)))
}

/// Points servers that named the listener at `from` to the one at `to` and
/// returns the changed field paths (`dns.<field>`).
pub fn retarget_own_listener(dns: &mut Mapping, from: &str, to: &str) -> Vec<String> {
    let retarget = |server: &mut Value| -> bool {
        let Some(text) = server.as_str() else {
            return false;
        };
        if !is_own_listener(text, from) {
            return false;
        }
        let (scheme, _, rest) = split_server(text).expect("own listener entries parse");
        let target = format!("{scheme}{to}{rest}");
        if target == text {
            return false;
        }
        *server = target.into();
        true
    };
    let mut changed = Vec::new();
    for field in SERVER_LISTS {
        let hit = match dns.get_mut(field) {
            Some(Value::Sequence(servers)) => servers.iter_mut().fold(false, |hit, server| retarget(server) | hit),
            Some(server) => retarget(server),
            None => false,
        };
        if hit {
            changed.push(format!("dns.{field}"));
        }
    }
    for field in SERVER_POLICIES {
        let Some(Value::Mapping(policy)) = dns.get_mut(field) else {
            continue;
        };
        let mut hit = false;
        for (_, servers) in policy.iter_mut() {
            hit |= match servers {
                Value::Sequence(servers) => servers.iter_mut().fold(false, |hit, server| retarget(server) | hit),
                server => retarget(server),
            };
        }
        if hit {
            changed.push(format!("dns.{field}"));
        }
    }
    changed
}

/// Splits a plain or `udp://`/`tcp://` nameserver into scheme, `host:port` and
/// the remainder (path, `#` options); other transports never reach a listener.
fn split_server(server: &str) -> Option<(&str, &str, &str)> {
    let server = server.trim();
    let scheme = ["udp://", "tcp://"]
        .into_iter()
        .find(|scheme| server.starts_with(scheme))
        .unwrap_or_default();
    let rest = &server[scheme.len()..];
    if rest.contains("://") {
        return None;
    }
    let end = rest.find(['/', '#', '?']).unwrap_or(rest.len());
    Some((scheme, &rest[..end], &rest[end..]))
}

/// `host:port`, `[v6]:port` or a bare host (port 53); the host may be empty.
fn split_address(address: &str) -> Option<(&str, u16)> {
    let address = address.trim();
    if let Some(v6) = address.strip_prefix('[') {
        let (host, port) = v6.split_once(']')?;
        let port = match port.strip_prefix(':') {
            Some(port) => port.parse().ok()?,
            None if port.is_empty() => 53,
            None => return None,
        };
        return Some((host, port));
    }
    match address.rsplit_once(':') {
        Some((host, _)) if host.contains(':') => Some((address, 53)),
        Some((host, port)) => Some((host, port.parse().ok()?)),
        None => Some((address, 53)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_listener_matches_loopback_entries_on_its_port() {
        for server in [
            "udp://127.0.0.1:7874",
            "127.0.0.1:7874",
            "tcp://localhost:7874",
            "[::1]:7874",
        ] {
            assert!(is_own_listener(server, "0.0.0.0:7874"), "{server}");
            assert!(is_own_listener(server, ":7874"), "{server}");
        }
        assert!(is_own_listener(
            "udp://127.0.0.1:7874#disable-ipv6=true",
            "127.0.0.1:7874"
        ));
        assert!(is_own_listener("192.168.1.2:53", "192.168.1.2"));
        for server in [
            "udp://127.0.0.1:7875",
            "https://127.0.0.1:7874/dns-query",
            "tls://127.0.0.1:7874",
            "223.5.5.5:7874",
            "system",
            "dhcp://en0",
        ] {
            assert!(!is_own_listener(server, "0.0.0.0:7874"), "{server}");
        }
        assert!(!is_own_listener("127.0.0.1:7874", "192.168.1.2:7874"));
        assert!(!is_own_listener("127.0.0.1:7874", ""));
    }

    #[test]
    fn retargeting_rewrites_every_server_field_and_keeps_options() {
        let mut dns: Mapping = serde_yaml_ng::from_str(
            "listen: 127.0.0.1:1053\nnameserver: [https://doh.example/dns-query, '127.0.0.1:7874']\n\
             proxy-server-nameserver: ['udp://127.0.0.1:7874#disable-ipv6=true']\n\
             nameserver-policy: {'+.lan': 'udp://127.0.0.1:7874', 'geosite:cn': [223.5.5.5]}\n\
             default-nameserver: [223.5.5.5]",
        )
        .unwrap();
        let changed = retarget_own_listener(&mut dns, "0.0.0.0:7874", "127.0.0.1:1053");
        assert_eq!(
            changed,
            ["dns.nameserver", "dns.proxy-server-nameserver", "dns.nameserver-policy"]
        );
        let expected: Mapping = serde_yaml_ng::from_str(
            "listen: 127.0.0.1:1053\nnameserver: [https://doh.example/dns-query, '127.0.0.1:1053']\n\
             proxy-server-nameserver: ['udp://127.0.0.1:1053#disable-ipv6=true']\n\
             nameserver-policy: {'+.lan': 'udp://127.0.0.1:1053', 'geosite:cn': [223.5.5.5]}\n\
             default-nameserver: [223.5.5.5]",
        )
        .unwrap();
        assert_eq!(dns, expected);
        assert!(retarget_own_listener(&mut dns, "0.0.0.0:7874", "127.0.0.1:1053").is_empty());
    }
}
