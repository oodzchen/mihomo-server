//! DNS policy shapes retained from the upstream settings page.
use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ResolverPolicyValue {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ResolverPolicy(pub BTreeMap<String, ResolverPolicyValue>);

impl<'de> Deserialize<'de> for ResolverPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let Value::Mapping(mapping) = Value::deserialize(deserializer)? else {
            return Err(serde::de::Error::custom("resolver policy must be an object"));
        };
        let mut entries = BTreeMap::new();
        for (key, value) in mapping {
            let Value::String(key) = key else {
                return Err(serde::de::Error::custom("resolver policy keys must be strings"));
            };
            let value = match value {
                Value::String(value) => ResolverPolicyValue::Single(value),
                Value::Sequence(values) => ResolverPolicyValue::Multiple(
                    values
                        .into_iter()
                        .map(|value| match value {
                            Value::String(value) => Ok(value),
                            _ => Err(serde::de::Error::custom("resolver policy lists must contain strings")),
                        })
                        .collect::<std::result::Result<_, D::Error>>()?,
                ),
                _ => {
                    return Err(serde::de::Error::custom(
                        "resolver policy values must be strings or string lists",
                    ));
                }
            };
            entries.insert(key, value);
        }
        let policy = Self(entries);
        policy.validate().map_err(serde::de::Error::custom)?;
        Ok(policy)
    }
}

impl ResolverPolicy {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(self.0.len() <= 256, "resolver policy accepts at most 256 entries");
        for (key, value) in &self.0 {
            ensure!(
                (1..=512).contains(&key.len()) && !key.trim().is_empty() && !key.chars().any(char::is_control),
                "resolver policy keys must be nonempty and at most 512 bytes"
            );
            let valid = |value: &String| {
                !value.trim().is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
            };
            match value {
                ResolverPolicyValue::Single(value) => ensure!(valid(value), "resolver policy server must be nonempty"),
                ResolverPolicyValue::Multiple(values) => ensure!(
                    (1..=32).contains(&values.len()) && values.iter().all(valid),
                    "resolver policy list must contain 1–32 nonempty servers"
                ),
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FallbackFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geoip: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_string"
    )]
    pub geoip_code: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_string_list"
    )]
    pub ipcidr: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_string_list"
    )]
    pub domain: Option<Vec<String>>,
}

impl FallbackFilter {
    pub(super) fn validate(&self) -> Result<()> {
        if let Some(code) = &self.geoip_code {
            ensure!(
                code.len() <= 32 && !code.chars().any(char::is_control),
                "dns.fallback-filter.geoip-code must be at most 32 bytes"
            );
        }
        for values in [&self.ipcidr, &self.domain].into_iter().flatten() {
            ensure!(
                values.len() <= 256
                    && values
                        .iter()
                        .all(|value| !value.trim().is_empty() && value.len() <= 512),
                "dns.fallback-filter lists must contain at most 256 nonempty entries"
            );
        }
        Ok(())
    }
}
