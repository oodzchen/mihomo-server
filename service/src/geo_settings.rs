//! Geo settings/config/core comparison without implying resource validity.
use headless_core::config::settings::RuntimeSettings;
use mihomo_client::models::BaseConfig;
use serde::Serialize;
use serde_json::Value;
use serde_yaml_ng::Mapping;

const KEYS: [&str; 8] = [
    "geodata-mode",
    "geodata-loader",
    "geo-auto-update",
    "geo-update-interval",
    "geox-url.geoip",
    "geox-url.geosite",
    "geox-url.mmdb",
    "geox-url.asn",
];
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
    actual: Option<&BaseConfig>,
) -> anyhow::Result<Snapshot> {
    let owned = serde_json::to_value(settings)?;
    let actual = actual.map(|core| serde_json::json!({
        "geodata-mode":core.geodata_mode, "geodata-loader":core.geodata_loader,
        "geo-auto-update":core.geo_auto_update, "geo-update-interval":core.geo_update_interval,
        "geox-url":{"geoip":core.geox_url.geo_ip,"geosite":core.geox_url.geo_site,"mmdb":core.geox_url.mmdb,"asn":core.geox_url.asn}
    }));
    let get = |value: &Value, key: &str| key.split('.').fold(value, |value, part| &value[part]).clone();
    Ok(Snapshot {
        config_revision: revision,
        running,
        error: (running && actual.is_none()).then_some("Geo core readback unavailable"),
        fields: KEYS
            .into_iter()
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_defaults_are_distinct_from_configured_mismatch_and_unavailable_core() -> anyhow::Result<()> {
        let runtime: RuntimeSettings =
            serde_yaml_ng::from_str("geodata-mode: false\ngeox-url: {mmdb: 'http://127.0.0.1/db'}")?;
        let config: Mapping = serde_yaml_ng::from_str("geodata-mode: true\ngeox-url: {mmdb: 'http://127.0.0.1/db'}")?;
        let core: BaseConfig = serde_json::from_value(
            serde_json::json!({"geodata-mode":false,"geox-url":{"mmdb":"http://127.0.0.1/db","geoip":"http://127.0.0.1/ip","geosite":"http://127.0.0.1/site"}}),
        )?;
        let value = snapshot(&runtime, Some(&config), Some("one.yaml".into()), true, Some(&core))?;
        assert!(value.fields[0].mismatch);
        assert_eq!(value.fields[0].setting, serde_json::json!(false));
        assert!(value.fields[4].configured.is_null());
        assert_eq!(value.fields[4].actual, "http://127.0.0.1/ip");
        assert!(!value.fields[4].mismatch);
        assert_eq!(value.fields[5].actual, "http://127.0.0.1/site");
        let unavailable = snapshot(&runtime, Some(&config), None, true, None)?;
        assert!(unavailable.error.is_some());
        assert!(unavailable.fields.iter().all(|f| f.actual.is_null() && !f.mismatch));
        let stopped = snapshot(&runtime, None, None, false, None)?;
        assert!(stopped.error.is_none());
        assert!(
            stopped
                .fields
                .iter()
                .all(|f| f.configured.is_null() && f.actual.is_null())
        );
        Ok(())
    }
}
