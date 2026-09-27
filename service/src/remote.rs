//! Direct subscription downloads. Store writes remain owned by the lifecycle actor.
use anyhow::{Context as _, Result, ensure};
use headless_core::config::{
    PrfOption,
    remote::{RemoteProfile, from_response, subscription_url},
    runtime::MAX_CONFIG_BYTES,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteOptions {
    pub user_agent: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub update_interval: Option<u64>,
    pub allow_auto_update: Option<bool>,
}

impl RemoteOptions {
    /// Refresh honors persisted options, including manual updates with auto-update disabled.
    pub fn from_profile(option: Option<&PrfOption>) -> Result<Self> {
        let Some(option) = option else {
            return Ok(Self::default());
        };
        ensure!(
            option.with_proxy != Some(true)
                && option.self_proxy != Some(true)
                && option.danger_accept_invalid_certs != Some(true),
            "profile proxy modes and TLS bypass are not supported yet"
        );
        Ok(Self {
            user_agent: option.user_agent.as_ref().map(ToString::to_string),
            timeout_seconds: option.timeout_seconds,
            update_interval: option.update_interval,
            allow_auto_update: option.allow_auto_update,
        })
    }
}

pub async fn download(url: &str, name: Option<&str>, options: RemoteOptions) -> Result<RemoteProfile> {
    let url = subscription_url(url)?;
    if let Some(name) = name {
        ensure!(
            !name.trim().is_empty() && name.len() <= 256,
            "profile name must be 1..256 bytes"
        );
    }
    let seconds = options.timeout_seconds.unwrap_or(20);
    ensure!(
        (1..=120).contains(&seconds),
        "subscription timeout must be 1..120 seconds"
    );
    let agent = options
        .user_agent
        .as_deref()
        .unwrap_or(concat!("clash-verge/v", env!("CARGO_PKG_VERSION")));
    ensure!(agent.len() <= 1024, "subscription user agent exceeds 1 KiB");
    let client = reqwest::Client::builder()
        .no_proxy()
        .user_agent(agent)
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(Duration::from_secs(seconds))
        .connect_timeout(Duration::from_secs(seconds.min(30)))
        .tcp_keepalive(Duration::from_secs(60))
        .pool_max_idle_per_host(0)
        .pool_idle_timeout(None)
        .build()?;
    let mut response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|error| error.without_url())
        .context("failed to fetch remote profile")?;
    ensure!(
        response.status().is_success(),
        "failed to fetch remote profile with status {}",
        response.status()
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= MAX_CONFIG_BYTES as u64),
        "profile exceeds 8 MiB"
    );
    let headers = response
        .headers()
        .iter()
        .filter_map(|(key, value)| value.to_str().ok().map(|v| (key.to_string(), v.to_owned())))
        .collect::<Vec<_>>();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| error.without_url())
        .context("failed to read remote profile")?
    {
        ensure!(chunk.len() <= MAX_CONFIG_BYTES - bytes.len(), "profile exceeds 8 MiB");
        bytes.extend_from_slice(&chunk);
    }
    let body = String::from_utf8(bytes).context("remote profile must be UTF-8")?;
    from_response(
        &url,
        name,
        &headers,
        &body,
        PrfOption {
            user_agent: options.user_agent.map(Into::into),
            timeout_seconds: options.timeout_seconds,
            update_interval: options.update_interval,
            allow_auto_update: options.allow_auto_update,
            ..PrfOption::default()
        },
    )
}
