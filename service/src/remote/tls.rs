//! Pinned upstream TLS fallback policy; retries keep the selected proxy and total deadline.
use anyhow::Result;
use reqwest::ClientBuilder;

#[derive(Clone, Copy)]
pub(super) enum RootMode {
    Platform,
    Static,
}

pub(super) fn configure(builder: ClientBuilder, mode: RootMode, invalid: bool) -> Result<ClientBuilder> {
    let mut builder = builder;
    if matches!(mode, RootMode::Static) {
        // The already locked DER bundle represents the same Mozilla roots as
        // upstream's WebPKI trust-anchor bundle. Use Reqwest's public TLS API.
        let certificates = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .map(|cert| reqwest::Certificate::from_der(cert.as_ref()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        builder = builder.tls_certs_only(certificates);
    }
    if invalid {
        builder = builder
            .tls_danger_accept_invalid_certs(true)
            .tls_danger_accept_invalid_hostnames(true);
    }
    Ok(builder)
}

fn legacy(error: &(dyn std::error::Error + 'static)) -> bool {
    let detail = format!("{error:#?}").to_ascii_lowercase();
    detail.contains("protocolversion") || detail.contains("protocol version")
}

pub(super) fn should_retry(error: &anyhow::Error) -> bool {
    if error.chain().any(legacy) {
        return false;
    }
    error.chain().any(|error| {
        let message = error.to_string().to_ascii_lowercase();
        [
            "certificate",
            "cert",
            "tls",
            "ssl",
            "rustls",
            "webpki",
            "revocation",
            "ocsp",
            "crl",
            "issuer",
            "unknownissuer",
        ]
        .iter()
        .any(|keyword| message.contains(keyword))
    })
}

pub(super) fn transport_error(error: reqwest::Error, context: &'static str) -> anyhow::Error {
    let error = error.without_url();
    let old_protocol = std::iter::successors(Some(&error as &(dyn std::error::Error + 'static)), |error| {
        error.source()
    })
    .any(legacy);
    let error = anyhow::Error::new(error).context(context);
    if old_protocol {
        error.context("Subscription server uses legacy TLS; only TLS 1.2/1.3 is supported")
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_matches_upstream_tls_errors_and_excludes_other_failures() {
        for message in [
            "certificate verify failed",
            "TLS handshake failed",
            "unknownissuer",
            "OCSP failure",
            "SSL failure",
            "no CA certificates loaded",
        ] {
            assert!(should_retry(&anyhow::anyhow!(message)));
        }
        for message in [
            "connection refused",
            "subscription download timed out",
            "profile exceeds 8 MiB",
            "status 503",
            "invalid UTF-8",
            "too many redirects",
        ] {
            assert!(!should_retry(&anyhow::anyhow!(message)));
        }
    }
    #[test]
    fn protocol_errors_never_trigger_root_retry_even_in_tls_context() {
        let error = anyhow::Error::new(rustls::Error::AlertReceived(rustls::AlertDescription::ProtocolVersion))
            .context("TLS request failed");
        assert!(!should_retry(&error));
        assert!(!should_retry(&anyhow::anyhow!("TLS protocol version rejected")));
    }
    #[test]
    fn static_roots_are_nonempty_parseable_and_build_a_verified_rustls_client() -> Result<()> {
        assert!(!webpki_root_certs::TLS_SERVER_ROOT_CERTS.is_empty());
        configure(
            reqwest::Client::builder().tls_backend_rustls().no_proxy(),
            RootMode::Static,
            false,
        )?
        .build()?;
        Ok(())
    }
}
