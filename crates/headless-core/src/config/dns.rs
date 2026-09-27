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
