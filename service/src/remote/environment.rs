//! Validate effective service proxy variables without exposing proxy URLs or credentials.
//! Matching, bypass and native-platform discovery remain owned by Reqwest.
use anyhow::{Result, ensure};
use std::ffi::OsString;

/// True means all system proxy discovery must be disabled for this request.
pub(super) fn bypass_all() -> Result<bool> {
    validate_values(|name| std::env::var_os(name))
}

fn validate_values(read: impl Fn(&str) -> Option<OsString>) -> Result<bool> {
    // The locked Reqwest/Hyper implementation disables system proxies in CGI contexts.
    if read("REQUEST_METHOD").is_some() {
        return Ok(true);
    }
    let mut bypass_all = false;
    for (upper, lower) in [
        ("HTTP_PROXY", "http_proxy"),
        ("HTTPS_PROXY", "https_proxy"),
        ("ALL_PROXY", "all_proxy"),
        ("NO_PROXY", "no_proxy"),
    ] {
        let selected = read(upper)
            .map(|value| (upper, value))
            .or_else(|| read(lower).map(|value| (lower, value)));
        let Some((name, value)) = selected else { continue };
        let value = value
            .to_str()
            .filter(|value| value.len() <= 8192 && !value.chars().any(char::is_control));
        ensure!(
            value.is_some(),
            "invalid service proxy environment {name}: expected UTF-8 without control characters, at most 8 KiB"
        );
        let value = value.unwrap();
        if upper == "NO_PROXY" {
            // Hyper-util 0.1.20 only matches its domain wildcard on domain names.
            // Preserve standard global bypass semantics for IP literals as well.
            bypass_all = value.split(',').any(|entry| entry.trim() == "*");
            continue;
        }
        if value.is_empty() {
            continue;
        }
        // Validate the same URI representation used by Hyper's system proxy matcher.
        // In particular, do not let a malformed configured proxy be silently ignored.
        let uri = value.parse::<axum::http::Uri>().ok();
        let valid = uri.as_ref().is_some_and(|uri| {
            matches!(uri.scheme_str(), None | Some("http" | "https"))
                && uri.authority().is_some_and(|authority| {
                    let scheme = uri.scheme_str().unwrap_or("http");
                    url::Url::parse(&format!("{scheme}://{authority}"))
                        .is_ok_and(|url| url.host_str().is_some() && url.port() != Some(0))
                })
        });
        ensure!(
            valid,
            "invalid service proxy environment {name}: expected an HTTP(S) proxy endpoint"
        );
    }
    Ok(bypass_all)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(values: &[(&str, &str)]) -> Result<()> {
        validate_values(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
        .map(|_| ())
    }
    #[test]
    fn accepts_http_https_credentials_authorities_bypass_and_empty_configuration() -> Result<()> {
        for value in [
            "http://user:secret@127.0.0.1:7890",
            "https://proxy.example:443",
            "127.0.0.1:7890",
            "http://[::1]:7890",
            "",
        ] {
            check(&[
                ("HTTP_PROXY", value),
                ("NO_PROXY", "localhost,.example.org,192.0.2.0/24"),
            ])?;
        }
        check(&[])
    }
    #[test]
    fn malformed_and_unsupported_values_fail_with_only_the_variable_name() {
        for value in [
            "file:///private-secret",
            "socks5://secret:password@proxy:1080",
            "http://secret:password@:7890",
            "http://proxy:0",
            "http://proxy:bad",
            " http://proxy:7890",
            "http://proxy\r\nsecret",
            &"s".repeat(8193),
        ] {
            let error = check(&[("HTTP_PROXY", value)]).unwrap_err().to_string();
            assert!(error.contains("HTTP_PROXY"));
            assert!(!error.contains("secret"));
            assert!(!error.contains("password"));
        }
    }
    #[test]
    fn uppercase_even_empty_takes_precedence_and_cgi_disables_discovery() -> Result<()> {
        check(&[("HTTP_PROXY", ""), ("http_proxy", "invalid secret")])?;
        check(&[("REQUEST_METHOD", "GET"), ("HTTP_PROXY", "invalid secret")])?;
        assert!(check(&[("http_proxy", "invalid secret")]).is_err());
        Ok(())
    }
    #[test]
    fn global_bypass_is_explicit_for_domains_and_ip_literals() -> Result<()> {
        assert!(validate_values(
            |name| (name == "NO_PROXY").then(|| OsString::from("localhost, *, .example.org"))
        )?);
        assert!(!validate_values(
            |name| (name == "NO_PROXY").then(|| OsString::from("localhost,127.0.0.1"))
        )?);
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn rejects_non_unicode_effective_proxy_variables() {
        use std::os::unix::ffi::OsStringExt as _;
        assert!(validate_values(|name| (name == "HTTP_PROXY").then(|| OsString::from_vec(vec![255]))).is_err());
    }
}
