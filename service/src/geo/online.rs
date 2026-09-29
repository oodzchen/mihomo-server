//! Bounded direct downloads from a committed Geo URL; no caller-supplied destination or URL.
use super::{resources::Seed, validation};
use anyhow::{Context as _, Result, ensure};
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Mapping;
use std::{
    fs,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncWriteExt as _;

const MAX_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct Info {
    pub name: String,
    pub current_sha256: Option<String>,
    pub source_sha256: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub name: String,
    pub expected_current_sha256: Option<String>,
    pub expected_source_sha256: String,
    #[serde(default)]
    pub expected_download_sha256: Option<String>,
    #[serde(default)]
    pub accept_metadata_only: bool,
    #[serde(default)]
    pub route: RouteChoice,
    #[serde(default)]
    pub danger_accept_invalid_certs: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteChoice {
    #[default]
    Direct,
    System,
    Managed,
}

pub(crate) fn source(config: &Mapping, name: &str) -> Result<(url::Url, String)> {
    ensure!(
        matches!(
            name,
            "geoip.dat" | "geosite.dat" | "Country.mmdb" | "geoip.metadb" | "ASN.mmdb"
        ),
        "unsupported online Geo filename"
    );
    let key = headless_core::config::settings::GeoUrls::key_for_asset(name)
        .ok_or_else(|| anyhow::anyhow!("unsupported online Geo filename"))?;
    let text = config
        .get("geox-url")
        .and_then(serde_yaml_ng::Value::as_mapping)
        .and_then(|map| map.get(key))
        .and_then(serde_yaml_ng::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("committed configuration has no explicit Geo source for this file"))?;
    ensure!(
        text.len() <= 8192 && !text.is_empty() && text.trim() == text && !text.chars().any(char::is_control),
        "invalid committed Geo source"
    );
    let url = url::Url::parse(text).map_err(|_| anyhow::anyhow!("invalid committed Geo source"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "Geo source requires HTTP(S) without credentials or fragment"
    );
    let fingerprint = validation::sha256(text.as_bytes());
    Ok((url, fingerprint))
}

pub(crate) fn info(config: &Mapping, data: &Path, name: &str) -> Result<Info> {
    let (_, source_sha256) = source(config, name)?;
    let current_sha256 = validation::snapshot(data, name)?.map(|bytes| validation::sha256(&bytes));
    Ok(Info {
        name: name.into(),
        current_sha256,
        source_sha256,
    })
}

pub(crate) struct Download {
    pub(crate) directory: PathBuf,
    pub(crate) seed: Seed,
}
impl Drop for Download {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
impl Download {
    fn new() -> Result<Self> {
        let directory = std::env::temp_dir().join(format!(
            "ms-geo-online-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        Ok(Self {
            directory,
            seed: Seed {
                bytes: 0,
                sha256: String::new(),
            },
        })
    }
}

pub(crate) async fn fetch(
    url: &url::Url,
    name: &str,
    expected_download_sha256: Option<&str>,
    route: &crate::core_release::Route,
    danger_accept_invalid_certs: bool,
) -> Result<Download> {
    ensure!(
        validation::MMDB_FILES.contains(&name) || crate::geo::dat::DAT_FILES.contains(&name),
        "unsupported online Geo filename"
    );
    if let Some(hash) = expected_download_sha256 {
        ensure!(
            hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid expected Geo download SHA-256"
        );
    }
    route
        .run(async {
            tokio::time::timeout(Duration::from_secs(20), async {
                match fetch_once(
                    url,
                    name,
                    expected_download_sha256,
                    route,
                    danger_accept_invalid_certs,
                    crate::remote::tls::RootMode::Platform,
                )
                .await
                {
                    Ok(download) => Ok(download),
                    Err(error) if !danger_accept_invalid_certs && crate::remote::tls::should_retry(&error) => {
                        fetch_once(
                            url,
                            name,
                            expected_download_sha256,
                            route,
                            false,
                            crate::remote::tls::RootMode::Static,
                        )
                        .await
                        .context("static webpki roots fallback failed after platform TLS verifier failed")
                    }
                    Err(error) => Err(error),
                }
            })
            .await
            .context("Geo download timed out")?
        })
        .await
}

async fn fetch_once(
    url: &url::Url,
    name: &str,
    expected_download_sha256: Option<&str>,
    route: &crate::core_release::Route,
    danger_accept_invalid_certs: bool,
    roots: crate::remote::tls::RootMode,
) -> Result<Download> {
    let builder = reqwest::Client::builder()
        .tls_backend_rustls()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(0);
    let builder = crate::remote::tls::configure(builder, roots, danger_accept_invalid_certs)?;
    let client = route.configure(builder)?.build()?;
    let mut response = client.get(url.clone()).send().await.map_err(|error| {
        crate::remote::tls::transport_error_for(
            error,
            "Geo download transport failed",
            "Geo source uses legacy TLS; only TLS 1.2/1.3 is supported",
        )
    })?;
    ensure!(
        response.status().is_success(),
        "Geo download failed with HTTP status {}",
        response.status()
    );
    ensure!(
        response.content_length().is_none_or(|n| n <= MAX_BYTES),
        "Geo download exceeds 128 MiB"
    );
    let mut output = Download::new()?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output.directory.join(name))?;
    let mut file = tokio::fs::File::from_std(file);
    let mut digest = Context::new(&SHA256);
    let mut count = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        crate::remote::tls::transport_error_for(
            error,
            "Geo download body failed",
            "Geo source uses legacy TLS; only TLS 1.2/1.3 is supported",
        )
    })? {
        count = count
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("Geo download too large"))?;
        ensure!(count <= MAX_BYTES, "Geo download exceeds 128 MiB");
        digest.update(&chunk);
        file.write_all(&chunk).await?;
    }
    ensure!(count > 0, "Geo download is empty");
    file.sync_all().await?;
    let hash = digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if let Some(expected) = expected_download_sha256 {
        ensure!(
            expected.eq_ignore_ascii_case(&hash),
            "Geo download SHA-256 differs from expected pin"
        );
    }
    output.seed = Seed {
        bytes: count,
        sha256: hash,
    };
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{HeaderValue, StatusCode, header},
        routing::get,
    };
    #[test]
    fn committed_source_names_and_credentials_are_constrained_without_exposing_url() -> Result<()> {
        let config: Mapping = serde_yaml_ng::from_str(
            "geox-url: {geoip: 'https://example.test/private?token=value', geosite: 'http://127.0.0.1:1234/site', mmdb: 'https://example.test/db', asn: 'https://example.test/asn'}",
        )?;
        for name in ["geoip.dat", "geosite.dat", "Country.mmdb", "geoip.metadb", "ASN.mmdb"] {
            assert!(source(&config, name).is_ok());
        }
        assert!(source(&config, "GeoSite.dat").is_err());
        assert_eq!(source(&config, "Country.mmdb")?.1, source(&config, "geoip.metadb")?.1);
        for bad in [
            "file:///etc/passwd",
            "https://user:pass@example.test/db",
            "https://example.test/db#part",
        ] {
            let config: Mapping = serde_yaml_ng::from_str(&format!("geox-url: {{mmdb: '{bad}'}}"))?;
            assert!(source(&config, "Country.mmdb").is_err());
        }
        assert!(source(&Mapping::new(), "geosite.dat").is_err());
        Ok(())
    }

    #[tokio::test]
    async fn bounded_download_checks_status_redirects_length_and_optional_digest() -> Result<()> {
        let route = crate::core_release::Route::Direct;
        let app = Router::new()
            .route("/ok", get(|| async { "private geo fixture" }))
            .route(
                "/redirect",
                get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/ok")], Body::empty()) }),
            )
            .route(
                "/huge",
                get(|| async {
                    (
                        [(header::CONTENT_LENGTH, HeaderValue::from_static("134217729"))],
                        Body::empty(),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(axum::serve(listener, app).into_future());
        let good = url::Url::parse(&format!("{origin}/ok"))?;
        let hash = validation::sha256(b"private geo fixture");
        let downloaded = fetch(&good, "geosite.dat", Some(&hash), &route, false).await?;
        assert_eq!(downloaded.seed.bytes, 19);
        assert_eq!(downloaded.seed.sha256, hash);
        assert_eq!(
            fs::read(downloaded.directory.join("geosite.dat"))?,
            b"private geo fixture"
        );
        let directory = downloaded.directory.clone();
        drop(downloaded);
        assert!(!directory.exists());
        assert!(
            fetch(&good, "geosite.dat", Some(&"0".repeat(64)), &route, false)
                .await
                .is_err()
        );
        assert!(
            fetch(
                &url::Url::parse(&format!("{origin}/redirect"))?,
                "geosite.dat",
                None,
                &route,
                false
            )
            .await
            .is_err()
        );
        assert!(
            fetch(
                &url::Url::parse(&format!("{origin}/huge"))?,
                "geosite.dat",
                None,
                &route,
                false
            )
            .await
            .is_err()
        );
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn downloaded_mmdb_uses_existing_stopped_core_staging_and_preserves_old_on_stale_hash() -> Result<()> {
        let bytes = crate::geo::validation::tests::fixture_with_description(true);
        let app = Router::new().route(
            "/country",
            get({
                let bytes = bytes.clone();
                move || {
                    let bytes = bytes.clone();
                    async move { bytes }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = url::Url::parse(&format!("http://{}/country", listener.local_addr()?))?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let downloaded = fetch(
            &url,
            "Country.mmdb",
            Some(&validation::sha256(&bytes)),
            &crate::core_release::Route::Direct,
            false,
        )
        .await?;
        let data = tempfile_data()?;
        let path = data.join("Country.mmdb");
        fs::write(&path, b"old database")?;
        let mut request = crate::geo::update::InstallRequest {
            name: "Country.mmdb".into(),
            expected_current_sha256: Some("0".repeat(64)),
            expected_seed_sha256: downloaded.seed.sha256.clone(),
            accept_metadata_only: false,
        };
        assert!(crate::geo::update::prepare(&downloaded.directory, &data, &downloaded.seed, &request).is_err());
        assert_eq!(fs::read(&path)?, b"old database");
        request.expected_current_sha256 = Some(validation::sha256(b"old database"));
        let receipt =
            crate::geo::update::prepare(&downloaded.directory, &data, &downloaded.seed, &request)?.publish()?;
        assert!(receipt.changed && receipt.validation.verified);
        assert!(receipt.core_load_verified.is_none());
        assert_eq!(fs::read(&path)?, bytes);
        assert!(!data.join(".geo-seed").exists());
        fs::remove_dir_all(data)?;
        server.abort();
        Ok(())
    }

    fn tempfile_data() -> Result<PathBuf> {
        let data = std::env::temp_dir().join(format!(
            "ms-online-mmdb-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir(&data)?;
        Ok(data)
    }
}
