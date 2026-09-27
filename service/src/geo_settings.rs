//! Geo settings/config/core comparison without implying resource validity.
use headless_core::config::settings::RuntimeSettings;
use mihomo_client::models::BaseConfig;
use serde_yaml_ng::Mapping;

const KEYS: [&str; 9] = [
    "geodata-mode",
    "geodata-loader",
    "geo-auto-update",
    "geo-update-interval",
    "geox-url.geoip",
    "geox-url.geosite",
    "geox-url.mmdb",
    "geox-url.asn",
    "geosite-matcher",
];
pub use crate::settings_readback::{Field, Snapshot};
pub(crate) fn snapshot(
    settings: &RuntimeSettings,
    config: Option<&Mapping>,
    revision: Option<String>,
    running: bool,
    actual: Option<&BaseConfig>,
) -> anyhow::Result<Snapshot> {
    let actual = actual.map(|core| serde_json::json!({
        "geodata-mode":core.geodata_mode, "geodata-loader":core.geodata_loader,
        "geo-auto-update":core.geo_auto_update, "geo-update-interval":core.geo_update_interval,
        "geosite-matcher": (!core.geosite_matcher.is_empty()).then_some(&core.geosite_matcher),
        "geox-url":{"geoip":core.geox_url.geo_ip,"geosite":core.geox_url.geo_site,"mmdb":core.geox_url.mmdb,"asn":core.geox_url.asn}
    }));
    crate::settings_readback::snapshot(
        settings,
        config,
        revision,
        running,
        actual.as_ref(),
        &KEYS,
        "Geo core readback unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geosite_matcher_readback_distinguishes_mismatch_inheritance_and_missing_core_field() -> anyhow::Result<()> {
        let runtime: RuntimeSettings = serde_yaml_ng::from_str("geosite-matcher: mph")?;
        let config: Mapping = serde_yaml_ng::from_str("geosite-matcher: mph")?;
        let core: BaseConfig = serde_json::from_value(serde_json::json!({"geosite-matcher":"succinct"}))?;
        let readback = snapshot(&runtime, Some(&config), None, true, Some(&core))?;
        let field = &readback.fields[8];
        assert_eq!(field.key, "geosite-matcher");
        assert_eq!(field.setting, "mph");
        assert_eq!(field.configured, "mph");
        assert_eq!(field.actual, "succinct");
        assert!(field.mismatch);
        let inherited = snapshot(
            &RuntimeSettings::default(),
            Some(&Mapping::new()),
            None,
            true,
            Some(&core),
        )?;
        assert!(inherited.fields[8].setting.is_null() && inherited.fields[8].configured.is_null());
        assert_eq!(inherited.fields[8].actual, "succinct");
        assert!(!inherited.fields[8].mismatch);
        let old_core: BaseConfig = serde_json::from_value(serde_json::json!({}))?;
        let unknown = snapshot(&runtime, Some(&config), None, true, Some(&old_core))?;
        assert!(unknown.fields[8].actual.is_null() && !unknown.fields[8].mismatch);
        Ok(())
    }
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
