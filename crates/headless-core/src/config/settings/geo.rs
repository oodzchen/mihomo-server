//! Typed Geo authority; URL leaves inherit independently of one another.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeodataLoader {
    Standard,
    MemConservative,
}

/// Canonical matcher names; Mihomo's legacy `hybrid` alias is left to source YAML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeositeMatcher {
    Succinct,
    Mph,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoUrls {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geoip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geosite: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmdb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asn: Option<String>,
}
impl GeoUrls {
    pub(super) fn validate(&self) -> Result<()> {
        for value in [&self.geoip, &self.geosite, &self.mmdb, &self.asn]
            .into_iter()
            .flatten()
        {
            ensure!(
                value.len() <= 8192
                    && !value.is_empty()
                    && value.trim() == value
                    && !value.chars().any(char::is_whitespace)
                    && !value.chars().any(char::is_control),
                "invalid Geo URL text (maximum 8192 bytes)"
            );
            let url = url::Url::parse(value).map_err(|_| anyhow::anyhow!("invalid Geo URL"))?;
            ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.fragment().is_none(),
                "Geo URLs require HTTP(S), a host, no user credentials and no fragment"
            );
        }
        Ok(())
    }
}
