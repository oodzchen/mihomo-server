//! Linux TUN admission, device validation, live-core readback, and route verification.
#[cfg(target_os = "linux")]
use std::path::Path;

use anyhow::{Context as _, Result, bail, ensure};
use mihomo_client::models::BaseConfig;
use serde_yaml_ng::Mapping;

fn enabled(config: &Mapping) -> bool {
    config
        .get("tun")
        .and_then(|tun| tun.get("enable"))
        .and_then(|enable| enable.as_bool())
        == Some(true)
}

pub(crate) fn validate_device_name(name: &str) -> Result<()> {
    ensure!(!name.is_empty(), "TUN device name must not be empty");
    ensure!(
        name.len() <= 15,
        "TUN device name '{name}' must be at most 15 bytes for Linux network interface compatibility"
    );
    ensure!(
        !name.contains('/') && !name.contains(':') && !name.chars().any(char::is_whitespace),
        "TUN device name '{name}' must not contain '/', ':', or whitespace"
    );
    Ok(())
}

pub(crate) fn preflight(config: &Mapping) -> Result<()> {
    if !enabled(config) {
        return Ok(());
    }
    if let Some(tun) = config.get("tun") {
        if let Some(device) = tun.get("device").and_then(|v| v.as_str()) {
            validate_device_name(device)?;
        }
        if let Some(mtu) = tun.get("mtu").and_then(|v| v.as_i64()) {
            ensure!(mtu > 0 && mtu <= 65535, "TUN MTU must be between 1 and 65535");
        }
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

#[cfg(target_os = "linux")]
pub(crate) fn check_interface_exists(name: &str) -> bool {
    if name.is_empty() || name.len() > 15 {
        return false;
    }
    Path::new("/sys/class/net").join(name).exists()
}

#[cfg(target_os = "linux")]
pub(crate) fn read_interface_flags(name: &str) -> Result<u32> {
    let path = Path::new("/sys/class/net").join(name).join("flags");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read flags for interface '{name}' at {}", path.display()))?;
    let trimmed = content.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    u32::from_str_radix(hex, 16)
        .with_context(|| format!("invalid hex flags '{trimmed}' for interface '{name}'"))
}

#[cfg(target_os = "linux")]
pub(crate) fn check_interface_up(name: &str) -> Result<bool> {
    const IFF_UP: u32 = 0x1;
    let flags = read_interface_flags(name)?;
    Ok((flags & IFF_UP) != 0)
}

#[cfg(target_os = "linux")]
pub(crate) fn read_interface_mtu(name: &str) -> Result<u32> {
    let path = Path::new("/sys/class/net").join(name).join("mtu");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read mtu for interface '{name}' at {}", path.display()))?;
    content.trim().parse::<u32>()
        .with_context(|| format!("invalid mtu '{content}' for interface '{name}'"))
}

#[cfg(target_os = "linux")]
pub(crate) fn verify_linux_interface_and_routes(device: &str, auto_route: bool) -> Result<()> {
    validate_device_name(device)?;
    ensure!(
        check_interface_exists(device),
        "TUN interface '{device}' was not found in host network interfaces"
    );
    ensure!(
        check_interface_up(device)?,
        "TUN interface '{device}' exists but is not UP"
    );
    let mtu = read_interface_mtu(device)?;
    ensure!(
        mtu > 0,
        "TUN interface '{device}' has invalid zero MTU"
    );
    if auto_route {
        let operstate_path = Path::new("/sys/class/net").join(device).join("operstate");
        if let Ok(operstate) = std::fs::read_to_string(&operstate_path) {
            let state = operstate.trim();
            ensure!(
                state != "down",
                "TUN interface '{device}' operstate is down, cannot route traffic"
            );
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn has_net_admin_capability() -> bool {
    if unsafe { libc::geteuid() } == 0 {
        return true;
    }
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("CapEff:") {
                let hex_str = rest.trim();
                if let Ok(caps) = u64::from_str_radix(hex_str, 16) {
                    const CAP_NET_ADMIN: u64 = 1 << 12;
                    return (caps & CAP_NET_ADMIN) != 0;
                }
            }
        }
    }
    false
}

pub(crate) fn verify(config: &Mapping, core: &BaseConfig) -> Result<()> {
    if let Some(enable) = config
        .get("tun")
        .and_then(|tun| tun.get("enable"))
        .and_then(|enable| enable.as_bool())
    {
        if enable && !core.tun.enable {
            #[cfg(target_os = "linux")]
            let cap_msg = if !has_net_admin_capability() {
                " (interface initialization failed: service lacks CAP_NET_ADMIN; grant capability or root privileges to Mihomo binary)"
            } else {
                " (interface initialization failed; check CAP_NET_ADMIN capability or root privileges for Mihomo)"
            };
            #[cfg(not(target_os = "linux"))]
            let cap_msg = " (interface initialization failed; check TUN privileges)";

            bail!("TUN enable mismatch: configured true, core reports false{cap_msg}");
        }
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
        #[cfg(target_os = "linux")]
        if core.tun.enable {
            verify_linux_interface_and_routes(&core.tun.device, core.tun.auto_route)?;
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

    #[test]
    fn device_name_validation_enforces_linux_constraints() -> Result<()> {
        assert!(validate_device_name("tun0").is_ok());
        assert!(validate_device_name("mihomo").is_ok());
        assert!(validate_device_name("utun").is_ok());
        assert!(validate_device_name("123456789012345").is_ok());

        assert!(validate_device_name("").is_err());
        assert!(validate_device_name("1234567890123456").is_err());
        assert!(validate_device_name("tun/0").is_err());
        assert!(validate_device_name("tun:0").is_err());
        assert!(validate_device_name("tun 0").is_err());
        Ok(())
    }

    #[test]
    fn preflight_rejects_invalid_tun_device_name_or_mtu() -> Result<()> {
        assert!(preflight(&from_str("tun: {enable: true, device: 'abcdefghijklmnop'}")?).is_err());
        assert!(preflight(&from_str("tun: {enable: true, device: 'tun:0'}")?).is_err());
        assert!(preflight(&from_str("tun: {enable: true, mtu: 0}")?).is_err());
        assert!(preflight(&from_str("tun: {enable: true, mtu: 70000}")?).is_err());
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

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_interface_checks_lo_device() -> Result<()> {
        assert!(check_interface_exists("lo"));
        assert!(!check_interface_exists("nonexistent_tun_99"));
        assert!(check_interface_up("lo")?);
        assert!(read_interface_mtu("lo")? > 0);
        assert!(verify_linux_interface_and_routes("lo", false).is_ok());
        assert!(verify_linux_interface_and_routes("nonexistent_tun_99", false).is_err());
        Ok(())
    }

    #[test]
    fn enabled_tun_requires_matching_live_core_report() -> Result<()> {
        let config = from_str("tun: {enable: true, device: lo, auto-route: false}")?;
        let mut core = BaseConfig::default();
        let err = verify(&config, &core).unwrap_err();
        assert!(format!("{err:#}").contains("CAP_NET_ADMIN"), "{err:#}");

        core.tun.enable = true;
        core.tun.device = "lo".into();
        core.tun.auto_route = false;
        verify(&config, &core)?;

        core.tun.auto_route = true;
        assert!(verify(&config, &core).is_err());

        core.tun.auto_route = false;
        core.tun.device = "other".into();
        assert!(verify(&config, &core).is_err());

        // Interface verification: nonexistent device fails when core reports enable: true
        let config_missing = from_str("tun: {enable: true, device: nonexistent_tun_99, auto-route: false}")?;
        core.tun.device = "nonexistent_tun_99".into();
        #[cfg(target_os = "linux")]
        assert!(verify(&config_missing, &core).is_err());

        assert!(verify(&from_str("tun: {enable: false}")?, &core).is_err());
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn capability_check_runs_without_panic() {
        let _ = has_net_admin_capability();
    }
}
