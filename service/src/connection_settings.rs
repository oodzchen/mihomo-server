//! Connection-policy comparison, not proof of socket behavior or process discovery.
pub use crate::settings_readback::Snapshot;
use headless_core::config::settings::RuntimeSettings;
use mihomo_client::models::{ConnectionConfig, FindProcessMode};
use serde_yaml_ng::Mapping;

pub(crate) fn snapshot(
    settings: &RuntimeSettings,
    config: Option<&Mapping>,
    revision: Option<String>,
    running: bool,
    actual: Option<&ConnectionConfig>,
) -> anyhow::Result<Snapshot> {
    let actual = actual.map(|core| {
        serde_json::json!({
            "tcp-concurrent": core.tcp_concurrent,
            "find-process-mode": core.find_process_mode.as_ref().map(|mode| match mode {
                FindProcessMode::Strict => "strict", FindProcessMode::Always => "always", FindProcessMode::Off => "off"
            }),
            "keep-alive-interval": core.keep_alive_interval,
            "keep-alive-idle": core.keep_alive_idle,
            "disable-keep-alive": core.disable_keep_alive,
            "interface-name": core.interface_name,
            "routing-mark": core.routing_mark
        })
    });
    let mut read = crate::settings_readback::snapshot(
        settings,
        config,
        revision,
        running,
        actual.as_ref(),
        &[
            "tcp-concurrent",
            "find-process-mode",
            "keep-alive-interval",
            "keep-alive-idle",
            "disable-keep-alive",
            "interface-name",
            "routing-mark",
        ],
        "Connection settings core readback unavailable",
    )?;
    // Mihomo stores the global mark as signed int32 in some versions. Preserve
    // raw readback, but compare the same valid 32-bit mark representation.
    if let Some(mark) = read.fields.iter_mut().find(|field| field.key == "routing-mark") {
        let bits = |value: &serde_json::Value| {
            value
                .as_i64()
                .filter(|n| (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(n))
                .map(|n| n as u32)
        };
        if let (Some(configured), Some(actual)) = (bits(&mark.configured), bits(&mark.actual)) {
            mark.mismatch = configured != actual;
        }
    }
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comparison_preserves_false_missing_fields_inheritance_and_core_mode_case() -> anyhow::Result<()> {
        let runtime: RuntimeSettings = serde_yaml_ng::from_str(
            "tcp-concurrent: false\nfind-process-mode: off\nkeep-alive-interval: 0\nkeep-alive-idle: -1\ndisable-keep-alive: false\ninterface-name: ''\nrouting-mark: 0",
        )?;
        let config: Mapping = serde_yaml_ng::from_str(
            "tcp-concurrent: false\nfind-process-mode: off\nkeep-alive-interval: 0\nkeep-alive-idle: -1\ndisable-keep-alive: false\ninterface-name: ''\nrouting-mark: 0",
        )?;
        for mode in ["Off", "off"] {
            let core = serde_json::from_value(
                serde_json::json!({"tcp-concurrent":false,"find-process-mode":mode,"keep-alive-interval":0,"keep-alive-idle":-1,"disable-keep-alive":false,"interface-name":"","routing-mark":0}),
            )?;
            let read = snapshot(&runtime, Some(&config), None, true, Some(&core))?;
            assert!(
                read.fields
                    .iter()
                    .all(|f| f.actual == f.configured && f.setting == f.actual && !f.mismatch)
            );
        }
        // Mixed-version cores may report only some fields; do not invent others.
        let partial = serde_json::from_value(serde_json::json!({"keep-alive-interval":0,"disable-keep-alive":false}))?;
        let partial = snapshot(&runtime, Some(&config), None, true, Some(&partial))?;
        assert!(partial.fields[0].actual.is_null() && partial.fields[1].actual.is_null());
        assert_eq!(partial.fields[2].actual, 0);
        assert!(partial.fields[3].actual.is_null());
        assert_eq!(partial.fields[4].actual, false);
        assert!(partial.fields[5].actual.is_null() && partial.fields[6].actual.is_null());
        assert!(partial.fields.iter().all(|f| !f.mismatch));
        let missing = snapshot(&runtime, Some(&config), None, true, Some(&ConnectionConfig::default()))?;
        assert!(missing.error.is_none());
        assert!(missing.fields.iter().all(|f| f.actual.is_null() && !f.mismatch));
        let core = serde_json::from_value(
            serde_json::json!({"tcp-concurrent":true,"find-process-mode":"Always","keep-alive-interval":30,"keep-alive-idle":60,"disable-keep-alive":true,"interface-name":"eth0","routing-mark":123}),
        )?;
        assert!(
            snapshot(&runtime, Some(&config), None, true, Some(&core))?
                .fields
                .iter()
                .all(|f| f.mismatch)
        );
        let inherited = snapshot(
            &RuntimeSettings::default(),
            Some(&Mapping::new()),
            None,
            true,
            Some(&core),
        )?;
        assert!(
            inherited
                .fields
                .iter()
                .all(|f| f.setting.is_null() && f.configured.is_null() && !f.mismatch)
        );
        let failed = snapshot(&runtime, Some(&config), None, true, None)?;
        assert!(failed.error.is_some() && failed.fields.iter().all(|f| f.actual.is_null()));
        assert!(snapshot(&runtime, None, None, false, None)?.error.is_none());
        Ok(())
    }
    #[test]
    fn routing_mark_comparison_keeps_raw_signed_values_and_compares_only_valid_bits() -> anyhow::Result<()> {
        for (configured, actual, mismatch) in [
            (0, 0, false),
            (123, 123, false),
            (i64::from(u32::MAX), -1, false),
            (2147483648, i64::from(i32::MIN), false),
            (-1, i64::from(u32::MAX), false),
            (123, -1, true),
            (4294967296, 0, true),
            (-2147483649, 2147483647, true),
        ] {
            let mut config = Mapping::new();
            config.insert("routing-mark".into(), configured.into());
            let core = ConnectionConfig {
                routing_mark: Some(actual),
                ..Default::default()
            };
            let read = snapshot(&RuntimeSettings::default(), Some(&config), None, true, Some(&core))?;
            let mark = &read.fields[6];
            assert_eq!(mark.configured, configured);
            assert_eq!(mark.actual, actual);
            assert_eq!(mark.mismatch, mismatch, "{configured}/{actual}");
        }
        Ok(())
    }
}
