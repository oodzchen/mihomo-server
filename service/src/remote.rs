//! Direct, system or managed HTTP proxy subscription downloads. Store writes remain owned by the lifecycle actor.
mod environment;

use anyhow::{Context as _, Result, ensure};
use headless_core::config::{
    PrfOption,
    remote::{RemoteProfile, from_response, subscription_url},
    runtime::MAX_CONFIG_BYTES,
};
use mihomo_client::models::BaseConfig;
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteOptions {
    pub with_proxy: Option<bool>,
    pub self_proxy: Option<bool>,
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
            option.danger_accept_invalid_certs != Some(true),
            "TLS bypass is not supported yet"
        );
        Ok(Self {
            with_proxy: option.with_proxy,
            self_proxy: option.self_proxy,
            user_agent: option.user_agent.as_ref().map(ToString::to_string),
            timeout_seconds: option.timeout_seconds,
            update_interval: option.update_interval,
            allow_auto_update: option.allow_auto_update,
        })
    }
}

/// A private route resolved from the running core, never from caller input or environment.
pub(crate) struct ManagedProxy {
    address: SocketAddr,
    authentication: Option<(String, String)>,
}

impl ManagedProxy {
    pub(crate) fn from_core(core: &BaseConfig, runtime: &serde_yaml_ng::Mapping) -> Result<Self> {
        let port = if core.mixed_port != 0 {
            core.mixed_port
        } else {
            core.port
        };
        ensure!(port != 0, "self_proxy requires a running HTTP or Mixed listener");
        let host = if !core.allow_lan || matches!(core.bind_address.as_str(), "" | "*" | "0.0.0.0" | "localhost") {
            IpAddr::from([127, 0, 0, 1])
        } else if core.bind_address == "::" {
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        } else {
            let host: IpAddr = core
                .bind_address
                .parse()
                .context("self_proxy requires a loopback-compatible bind address")?;
            ensure!(
                host.is_loopback(),
                "self_proxy requires a loopback-compatible bind address"
            );
            host
        };
        // Mihomo reports authentication usernames, intentionally omitting passwords.
        // Credentials come only from the committed private runtime snapshot.
        let mut users = Vec::new();
        let mut authentication = None;
        if let Some(values) = runtime.get("authentication") {
            let values = values.as_sequence().context("invalid managed proxy authentication")?;
            for value in values {
                let (user, password) = value
                    .as_str()
                    .and_then(|value| value.split_once(':'))
                    .context("invalid managed proxy authentication")?;
                users.push(user.to_owned());
                if authentication.is_none() {
                    authentication = Some((user.to_owned(), password.to_owned()));
                }
            }
        }
        users.sort();
        users.dedup();
        let mut actual = core.authentication.clone().unwrap_or_default();
        actual.sort();
        actual.dedup();
        ensure!(actual == users, "managed proxy authentication changed; retry");
        Ok(Self {
            address: SocketAddr::new(host, port),
            authentication,
        })
    }
}

pub async fn download(url: &str, name: Option<&str>, options: RemoteOptions) -> Result<RemoteProfile> {
    download_via(url, name, options, None).await
}

pub(crate) async fn download_via(
    url: &str,
    name: Option<&str>,
    options: RemoteOptions,
    proxy: Option<ManagedProxy>,
) -> Result<RemoteProfile> {
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
    ensure!(
        agent.len() <= 1024 && !agent.chars().any(char::is_control),
        "invalid subscription user agent"
    );
    ensure!(
        options.self_proxy == Some(true) || proxy.is_none(),
        "unexpected managed proxy route"
    );
    ensure!(
        options.self_proxy != Some(true) || proxy.is_some(),
        "self_proxy requires a running managed proxy"
    );
    let mut builder = reqwest::Client::builder()
        .user_agent(agent)
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(Duration::from_secs(seconds))
        .connect_timeout(Duration::from_secs(seconds.min(30)))
        .tcp_keepalive(Duration::from_secs(60))
        .pool_max_idle_per_host(0)
        .pool_idle_timeout(None);
    if options.with_proxy == Some(true) && options.self_proxy != Some(true) {
        if environment::bypass_all()? {
            builder = builder.no_proxy();
        }
        // Default Reqwest discovery handles environment variables, NO_PROXY and
        // native platform settings. Only an explicit with_proxy option enables it.
    } else {
        builder = builder.no_proxy();
    }
    if let Some(route) = proxy {
        let mut proxy = reqwest::Proxy::all(format!("http://{}", route.address))?;
        if let Some((user, password)) = route.authentication {
            proxy = proxy.basic_auth(&user, &password);
        }
        builder = builder.proxy(proxy);
    }
    let client = builder.build()?;
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
            with_proxy: options.with_proxy,
            self_proxy: options.self_proxy,
            user_agent: options.user_agent.map(Into::into),
            timeout_seconds: options.timeout_seconds,
            update_interval: options.update_interval,
            allow_auto_update: options.allow_auto_update,
            ..PrfOption::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_route_prefers_mixed_then_http_and_supports_loopback_binds() -> Result<()> {
        let mut core = BaseConfig {
            mixed_port: 7890,
            port: 7891,
            ..Default::default()
        };
        let runtime = serde_yaml_ng::Mapping::new();
        assert_eq!(
            ManagedProxy::from_core(&core, &runtime)?.address,
            "127.0.0.1:7890".parse()?
        );
        core.mixed_port = 0;
        assert_eq!(ManagedProxy::from_core(&core, &runtime)?.address.port(), 7891);
        core.allow_lan = true;
        core.bind_address = "::".into();
        assert_eq!(ManagedProxy::from_core(&core, &runtime)?.address, "[::1]:7891".parse()?);
        core.bind_address = "127.0.0.2".into();
        assert_eq!(
            ManagedProxy::from_core(&core, &runtime)?.address,
            "127.0.0.2:7891".parse()?
        );
        core.bind_address = "192.0.2.1".into();
        assert!(ManagedProxy::from_core(&core, &runtime).is_err());
        core.allow_lan = false;
        core.port = 0;
        core.socks_port = 7892;
        assert!(ManagedProxy::from_core(&core, &runtime).is_err());
        Ok(())
    }
    #[test]
    fn proxy_credentials_use_private_snapshot_and_reject_changed_authentication() -> Result<()> {
        let core = BaseConfig {
            mixed_port: 7890,
            authentication: Some(vec!["private-user".into()]),
            ..Default::default()
        };
        let runtime = headless_core::config::runtime::parse("authentication: ['private-user:pass:word']")?;
        let route = ManagedProxy::from_core(&core, &runtime)?;
        assert_eq!(route.authentication, Some(("private-user".into(), "pass:word".into())));
        let error = ManagedProxy::from_core(&core, &serde_yaml_ng::Mapping::new())
            .err()
            .unwrap();
        assert!(!error.to_string().contains("private-user"));
        assert!(!error.to_string().contains("pass:word"));
        Ok(())
    }
    #[tokio::test]
    async fn download_rejects_control_characters_and_missing_managed_route_before_network() {
        for options in [
            RemoteOptions {
                self_proxy: Some(true),
                ..Default::default()
            },
            RemoteOptions {
                user_agent: Some("bad\r\nheader".into()),
                ..Default::default()
            },
        ] {
            assert!(download("http://127.0.0.1:1", None, options).await.is_err());
        }
    }
}
