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

    /// Return the committed URL for a specific Geo asset filename if configured.
    pub fn url_for_asset(&self, name: &str) -> Option<&str> {
        match name {
            "geoip.dat" => self.geoip.as_deref(),
            "geosite.dat" | "GeoSite.dat" => self.geosite.as_deref(),
            "Country.mmdb" | "geoip.metadb" => self.mmdb.as_deref(),
            "ASN.mmdb" => self.asn.as_deref(),
            _ => None,
        }
    }

    /// Map a config key to its standard Geo asset filename.
    pub fn asset_for_key(key: &str) -> Option<&'static str> {
        match key {
            "geoip" => Some("geoip.dat"),
            "geosite" => Some("geosite.dat"),
            "mmdb" => Some("Country.mmdb"),
            "asn" => Some("ASN.mmdb"),
            _ => None,
        }
    }

    /// Map a Geo asset filename to its config key in geox-url.
    pub fn key_for_asset(name: &str) -> Option<&'static str> {
        match name {
            "geoip.dat" => Some("geoip"),
            "geosite.dat" | "GeoSite.dat" => Some("geosite"),
            "Country.mmdb" | "geoip.metadb" => Some("mmdb"),
            "ASN.mmdb" => Some("asn"),
            _ => None,
        }
    }

    /// Returns true if all configured URLs are empty/None.
    pub fn is_empty(&self) -> bool {
        self.geoip.is_none() && self.geosite.is_none() && self.mmdb.is_none() && self.asn.is_none()
    }
}
