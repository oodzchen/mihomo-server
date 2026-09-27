//! Shared committed-settings comparison; callers provide only allowed actual leaves.
use headless_core::config::settings::RuntimeSettings;
use serde::Serialize;
use serde_json::Value;
use serde_yaml_ng::Mapping;

#[derive(Debug, Serialize)]
pub struct Field {
    pub key: &'static str,
    pub setting: Value,
    pub configured: Value,
    pub actual: Value,
    pub mismatch: bool,
}
#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub config_revision: Option<String>,
    pub running: bool,
    pub error: Option<&'static str>,
    pub fields: Vec<Field>,
}

pub(crate) fn snapshot(
    settings: &RuntimeSettings,
    config: Option<&Mapping>,
    revision: Option<String>,
    running: bool,
    actual: Option<&Value>,
    keys: &[&'static str],
    unavailable: &'static str,
) -> anyhow::Result<Snapshot> {
    let owned = serde_json::to_value(settings)?;
    let get = |value: &Value, key: &str| key.split('.').fold(value, |value, part| &value[part]).clone();
    Ok(Snapshot {
        config_revision: revision,
        running,
        error: (running && actual.is_none()).then_some(unavailable),
        fields: keys
            .iter()
            .copied()
            .map(|key| {
                let configured = if let Some(config) = config {
                    let mut parts = key.split('.');
                    let value = config.get(parts.next().unwrap()).and_then(|value| {
                        parts.try_fold(value, |value, part| value.as_mapping().and_then(|map| map.get(part)))
                    });
                    value.map(serde_json::to_value).transpose()?.unwrap_or(Value::Null)
                } else {
                    Value::Null
                };
                let actual = actual.as_ref().map(|value| get(value, key)).unwrap_or(Value::Null);
                let mismatch = !configured.is_null() && !actual.is_null() && configured != actual;
                Ok::<_, anyhow::Error>(Field {
                    key,
                    setting: get(&owned, key),
                    configured,
                    actual,
                    mismatch,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    })
}
