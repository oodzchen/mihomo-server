//! Pure URL/body/header processing adapted from upstream PrfItem::from_url.
use super::{PrfExtra, PrfOption, runtime::MAX_CONFIG_BYTES};
use anyhow::{Context as _, Result, ensure};
use serde_yaml_ng::Mapping;
use std::str::FromStr;
use url::{Url, form_urlencoded};

#[derive(Debug, Clone)]
pub struct RemoteProfile {
    pub url: String,
    pub name: String,
    pub yaml: String,
    pub extra: Option<PrfExtra>,
    pub home: Option<String>,
    pub option: PrfOption,
}

/// Upstream dirty query repair; never include the secret URL in parse errors.
pub fn subscription_url(input: &str) -> Result<Url> {
    ensure!(input.len() <= 8192, "subscription URL exceeds 8 KiB");
    let mut url = Url::parse(input).context("invalid subscription URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "subscription URL must use HTTP or HTTPS"
    );
    if url.query().is_none() && url.path().contains('&') {
        let path = url.path().to_string();
        if let Some((clean_path, dirty_params)) = path.split_once('&') {
            url.set_path(clean_path);
            url.query_pairs_mut()
                .extend_pairs(form_urlencoded::parse(dirty_params.as_bytes()));
        }
    }
    url.set_fragment(None);
    Ok(url)
}

pub fn validate_yaml(body: &str) -> Result<&str> {
    ensure!(body.len() <= MAX_CONFIG_BYTES, "profile exceeds 8 MiB");
    let body = body.trim_start_matches('\u{feff}');
    let mapping: Mapping = serde_yaml_ng::from_str(body).context("remote profile must be a YAML mapping")?;
    ensure!(
        mapping.contains_key("proxies") || mapping.contains_key("proxy-providers"),
        "profile does not contain `proxies` or `proxy-providers`"
    );
    Ok(body)
}

// Upstream utils::help::parse_str, unchanged.
fn parse_str<T: FromStr>(target: &str, key: &str) -> Option<T> {
    target.split(';').map(str::trim).find_map(|s| {
        let mut parts = s.splitn(2, '=');
        match (parts.next(), parts.next()) {
            (Some(k), Some(v)) if k == key => v.parse::<T>().ok(),
            _ => None,
        }
    })
}

pub fn from_response(
    url: &Url,
    name: Option<&str>,
    headers: &[(String, String)],
    body: &str,
    mut option: PrfOption,
) -> Result<RemoteProfile> {
    let yaml = validate_yaml(body)?.to_owned();
    let header = |key: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    };
    let extra = headers.iter().find_map(|(key, value)| {
        key.to_ascii_lowercase()
            .strip_suffix("subscription-userinfo")
            .filter(|prefix| prefix.is_empty() || prefix.ends_with('-'))
            .map(|_| PrfExtra {
                upload: parse_str(value, "upload").unwrap_or(0),
                download: parse_str(value, "download").unwrap_or(0),
                total: parse_str(value, "total").unwrap_or(0),
                expire: parse_str(value, "expire").unwrap_or(0),
            })
    });
    let filename = header("content-disposition").and_then(|value| {
        if let Some(encoded) = parse_str::<String>(value, "filename*") {
            percent_encoding::percent_decode_str(&encoded)
                .decode_utf8()
                .ok()
                .and_then(|decoded| decoded.split("''").last().map(str::to_owned))
        } else {
            parse_str::<String>(value, "filename").map(|filename| filename.trim_matches('"').to_owned())
        }
    });
    let fallback = || {
        percent_encoding::percent_decode_str(url.path().rsplit('/').next().unwrap_or(""))
            .decode_utf8_lossy()
            .into_owned()
    };
    let name = name
        .map(str::to_owned)
        .unwrap_or_else(|| filename.unwrap_or_else(fallback));
    let name = if name.trim().is_empty() {
        "Remote File".to_owned()
    } else {
        name
    };
    ensure!(name.len() <= 256, "profile name must be 1..256 bytes");
    if option.update_interval.is_none() {
        option.update_interval = header("profile-update-interval")
            .and_then(|value| value.parse::<u64>().ok())
            .and_then(|hours| hours.checked_mul(60));
    }
    option.allow_auto_update = Some(option.allow_auto_update.unwrap_or(true));
    // Upstream normalize_profile_home_url, with ordinary url::Url instead of Tauri.
    let home = header("profile-web-page-url").and_then(|raw| {
        let url = Url::parse(raw.trim()).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        url.host_str()?;
        Some(url.to_string())
    });
    Ok(RemoteProfile {
        url: url.to_string(),
        name,
        yaml,
        extra,
        home,
        option,
    })
}
