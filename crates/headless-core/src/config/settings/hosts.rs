//! Service-owned hosts mapping; scalar aliases and IP lists retain their shape.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    net::IpAddr,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum HostValue {
    Single(String),
    Addresses(Vec<String>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Hosts(pub BTreeMap<String, HostValue>);

impl<'de> Deserialize<'de> for Hosts {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let Value::Mapping(map) = Value::deserialize(d)? else {
            return Err(serde::de::Error::custom("hosts must be a mapping"));
        };
        let mut hosts = BTreeMap::new();
        for (key, value) in map {
            let Value::String(key) = key else {
                return Err(serde::de::Error::custom("hosts keys must be strings"));
            };
            let value = match value {
                Value::String(v) => HostValue::Single(v),
                Value::Sequence(v) => HostValue::Addresses(
                    v.into_iter()
                        .map(|v| match v {
                            Value::String(v) => Ok(v),
                            _ => Err(serde::de::Error::custom("hosts address lists must contain strings")),
                        })
                        .collect::<std::result::Result<_, D::Error>>()?,
                ),
                _ => {
                    return Err(serde::de::Error::custom(
                        "hosts values must be strings or IP string lists",
                    ));
                }
            };
            hosts.insert(key, value);
        }
        let hosts = Self(hosts);
        hosts.validate().map_err(serde::de::Error::custom)?;
        Ok(hosts)
    }
}

// Bounded ASCII subset of Mihomo domain patterns; IDNs can use punycode.
fn domain(value: &str, pattern: bool) -> bool {
    if value.is_empty() || value.len() > 253 {
        return false;
    }
    let parts: Vec<_> = value.split('.').collect();
    parts.iter().enumerate().all(|(i, part)| {
        if pattern && (*part == "*" || (i == 0 && parts.len() > 1 && matches!(*part, "" | "+"))) {
            return true;
        }
        !part.is_empty()
            && part.len() <= 63
            && !part.starts_with('-')
            && !part.ends_with('-')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    })
}

impl Hosts {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(self.0.len() <= 1024, "hosts accepts at most 1024 entries");
        let mut keys = BTreeSet::new();
        let mut aliases = BTreeMap::new();
        for (key, value) in &self.0 {
            ensure!(domain(key, true), "hosts keys must be bounded ASCII domain patterns");
            let key = key.to_ascii_lowercase();
            ensure!(
                keys.insert(key.clone()),
                "hosts keys must be unique ignoring ASCII case"
            );
            match value {
                HostValue::Single(v) if v == "lan" || v.parse::<IpAddr>().is_ok() => {}
                HostValue::Single(v) => {
                    ensure!(
                        v.contains('.') && domain(v, false),
                        "hosts scalar must be an IP, lan or ASCII domain alias"
                    );
                    aliases.insert(key, v.to_ascii_lowercase());
                }
                HostValue::Addresses(values) => ensure!(
                    (1..=64).contains(&values.len()) && values.iter().all(|v| v.parse::<IpAddr>().is_ok()),
                    "hosts lists must contain 1–64 IP address strings"
                ),
            }
        }
        // Reject potential cycles through wildcard aliases as well as exact ones.
        // Conservatively include every matching alias, even if another entry shadows it.
        let names: Vec<_> = aliases.keys().collect();
        let mut incoming = vec![0usize; names.len()];
        let mut edges = vec![Vec::new(); names.len()];
        for (i, key) in names.iter().enumerate() {
            for (j, pattern) in names.iter().enumerate() {
                if matches_domain(pattern, &aliases[*key]) {
                    edges[i].push(j);
                    incoming[j] += 1;
                }
            }
        }
        let mut ready: VecDeque<_> = incoming
            .iter()
            .enumerate()
            .filter_map(|(i, n)| (*n == 0).then_some(i))
            .collect();
        let mut visited = 0;
        while let Some(i) = ready.pop_front() {
            visited += 1;
            for j in &edges[i] {
                incoming[*j] -= 1;
                if incoming[*j] == 0 {
                    ready.push_back(*j);
                }
            }
        }
        ensure!(visited == names.len(), "hosts contains a potential domain alias cycle");
        Ok(())
    }
}

fn matches_domain(pattern: &str, name: &str) -> bool {
    let suffix = pattern.starts_with('.') || pattern.starts_with("+.");
    let parts: Vec<_> = pattern
        .trim_start_matches('.')
        .trim_start_matches("+.")
        .split('.')
        .collect();
    let labels: Vec<_> = name.split('.').collect();
    if suffix {
        if labels.len() < parts.len() || (pattern.starts_with('.') && labels.len() == parts.len()) {
            return false;
        }
    } else if labels.len() != parts.len() {
        return false;
    }
    parts
        .iter()
        .rev()
        .zip(labels.iter().rev())
        .all(|(p, n)| *p == "*" || p == n)
}
