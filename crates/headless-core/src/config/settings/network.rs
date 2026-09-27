//! Typed subset of upstream DNS settings and saved TUN dialog fields.
//! TUN activation does not change host DNS or grant service privileges.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DnsMode {
    FakeIp,
    RedirHost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunStack {
    Gvisor,
    System,
    Mixed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DnsSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhanced_mode: Option<DnsMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fake_ip_range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fake_ip_range6: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_hosts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_system_hosts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_nameserver: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nameserver: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fake_ip_filter: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TunSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<TunStack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_route: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_exclude_address: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_redirect: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_detect_interface: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_hijack: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict_route: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u16>,
}

impl TunSettings {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            cfg!(target_os = "linux") || self.auto_redirect.is_none(),
            "tun.auto-redirect is supported only on Linux"
        );
        ensure!(self.mtu != Some(0), "tun.mtu must be greater than zero");
        ensure!(
            self.device.as_ref().is_none_or(|v| !v.is_empty()),
            "tun.device must not be empty"
        );
        Ok(())
    }
}
