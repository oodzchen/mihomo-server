//! TCP concurrency and process-policy comparison, not proof of process discovery.
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
            })
        })
    });
    crate::settings_readback::snapshot(
        settings,
        config,
        revision,
        running,
        actual.as_ref(),
        &["tcp-concurrent", "find-process-mode"],
        "Connection settings core readback unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comparison_preserves_false_missing_fields_inheritance_and_core_mode_case() -> anyhow::Result<()> {
        let runtime: RuntimeSettings = serde_yaml_ng::from_str("tcp-concurrent: false\nfind-process-mode: off")?;
        let config: Mapping = serde_yaml_ng::from_str("tcp-concurrent: false\nfind-process-mode: off")?;
        for mode in ["Off", "off"] {
            let core = serde_json::from_value(serde_json::json!({"tcp-concurrent":false,"find-process-mode":mode}))?;
            let read = snapshot(&runtime, Some(&config), None, true, Some(&core))?;
            assert!(
                read.fields
                    .iter()
                    .all(|f| f.actual == f.configured && f.setting == f.actual && !f.mismatch)
            );
        }
        let missing = snapshot(&runtime, Some(&config), None, true, Some(&ConnectionConfig::default()))?;
        assert!(missing.error.is_none());
        assert!(missing.fields.iter().all(|f| f.actual.is_null() && !f.mismatch));
        let core = serde_json::from_value(serde_json::json!({"tcp-concurrent":true,"find-process-mode":"Always"}))?;
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
}
