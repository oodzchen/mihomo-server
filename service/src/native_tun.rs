//! Linux TUN admission and live-core readback. Mihomo owns the interface and routes.
#[cfg(target_os = "linux")]
use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use mihomo_client::models::BaseConfig;
use serde_yaml_ng::Mapping;

fn enabled(config: &Mapping) -> bool {
    config
        .get("tun")
        .and_then(|tun| tun.get("enable"))
        .and_then(|enable| enable.as_bool())
        == Some(true)
}

pub(crate) fn preflight(config: &Mapping) -> Result<()> {
    if !enabled(config) {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    check_device(Path::new("/dev/net/tun"))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn check_device(path: &Path) -> Result<()> {
    use std::os::unix::fs::FileTypeExt as _;

    let metadata = path
        .metadata()
        .with_context(|| format!("TUN is enabled but {} is unavailable", path.display()))?;
    ensure!(
        metadata.file_type().is_char_device(),
        "TUN device {} is not a character device",
        path.display()
    );
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("TUN is enabled but {} cannot be opened by the service", path.display()))?;
    Ok(())
}

pub(crate) fn verify(config: &Mapping, core: &BaseConfig) -> Result<()> {
    if let Some(enable) = config
        .get("tun")
        .and_then(|tun| tun.get("enable"))
        .and_then(|enable| enable.as_bool())
    {
        ensure!(
            core.tun.enable == enable,
            "TUN enable mismatch: configured {enable}, core reports {}",
            core.tun.enable
        );
    }
    if enabled(config) {
        if let Some(device) = config
            .get("tun")
            .and_then(|tun| tun.get("device"))
            .and_then(|value| value.as_str())
        {
            ensure!(
                core.tun.device == device,
                "TUN device mismatch: configured {device}, core reports {}",
                core.tun.device
            );
        }
        if let Some(route) = config
            .get("tun")
            .and_then(|tun| tun.get("auto-route"))
            .and_then(|value| value.as_bool())
        {
            ensure!(
                core.tun.auto_route == route,
                "TUN auto-route mismatch: configured {route}, core reports {}",
                core.tun.auto_route
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml_ng::from_str;

    #[test]
    fn disabled_and_inherited_tun_skip_host_preflight() -> Result<()> {
        preflight(&from_str("tun: {enable: false}")?)?;
        preflight(&from_str("mode: direct")?)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn enabled_tun_rejects_missing_or_non_device_path() -> Result<()> {
        let root = std::env::temp_dir().join(format!("mihomo-tun-preflight-{}", std::process::id()));
        assert!(check_device(&root).is_err());
        std::fs::write(&root, b"not a device")?;
        assert!(format!("{:#}", check_device(&root).unwrap_err()).contains("not a character device"));
        std::fs::remove_file(root)?;
        Ok(())
    }

    #[test]
    fn enabled_tun_requires_matching_live_core_report() -> Result<()> {
        let config = from_str("tun: {enable: true, device: mihomo, auto-route: false}")?;
        let mut core = BaseConfig::default();
        assert!(verify(&config, &core).is_err());
        core.tun.enable = true;
        core.tun.device = "mihomo".into();
        core.tun.auto_route = false;
        verify(&config, &core)?;
        core.tun.auto_route = true;
        assert!(verify(&config, &core).is_err());
        core.tun.auto_route = false;
        core.tun.device = "other".into();
        assert!(verify(&config, &core).is_err());
        assert!(verify(&from_str("tun: {enable: false}")?, &core).is_err());
        Ok(())
    }
}
