//! Region-restriction ("unlock") tests: where common overseas services and IP
//! location databases place the proxy, and whether anonymous sample media is reachable.
//!
//! Every request goes through one node, in an isolated core that has no rules
//! and bypasses the running core's TUN and listeners (see `proxy_probe`), so the
//! results describe that node alone: neither the active rules, which may send a
//! service to another group or DIRECT, nor the host's own proxying change them.
//! Each check is one small, independent test; the page runs several at once.
//!
//! Public location/currency lookups report only those facts. Playback checks
//! require a usable anonymous sample stream; a homepage, login error or unknown
//! response never proves access. Services requiring sign-in or complex device
//! sessions are deliberately omitted.
use anyhow::{Context as _, Result, bail, ensure};
use mihomo_client::models::ClashMode;
use reqwest::{Client, RequestBuilder, StatusCode, Url, header, redirect};
use serde::Serialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

use crate::core_manager::{CoreManager, CorePhase};

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
/// Google's consent pre-acceptance; EU addresses otherwise get the consent page.
const GOOGLE_CONSENT: &str =
    "SOCS=CAISNQgDEitib3FfaWRlbnRpdHlmcm9udGVuZHVpc2VydmVyXzIwMjMwODI5LjA3X3AxGgJlbiACGgYIgLC_pwY";
/// One request; a check makes at most a few, and is cut off as a whole after `CHECK_TIMEOUT`.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CHECK_TIMEOUT: Duration = Duration::from_secs(25);
/// Larger or truncated pages cannot establish a result.
const BODY_LIMIT: usize = 4 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// IP location databases and what large platforms think the location is.
    Location,
    Streaming,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Service {
    pub id: &'static str,
    pub name: &'static str,
    pub category: Category,
}

const fn service(id: &'static str, name: &'static str, category: Category) -> Service {
    Service { id, name, category }
}

pub const SERVICES: &[Service] = &[
    service("cloudflare", "Cloudflare", Category::Location),
    service("google", "Google", Category::Location),
    service("ipinfo", "IPinfo", Category::Location),
    service("ip_api", "ip-api.com", Category::Location),
    service("ip_sb", "IP.SB", Category::Location),
    service("ipwhois", "ipwho.is", Category::Location),
    service("apple", "Apple", Category::Location),
    service("bing", "Microsoft Bing", Category::Location),
    service("bilibili_hk_mo_tw", "Bilibili HK/MO/TW sample", Category::Streaming),
    service("bilibili_tw", "Bilibili TW sample", Category::Streaming),
    service("steam", "Steam", Category::Other),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Served (for location services: answered).
    Yes,
    No,
    /// The test itself failed: network error, timeout or unrecognized answer.
    Error,
}

/// Why a service is limited or refused, for the page to explain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Note {
    /// The sample's player explicitly reports a region restriction.
    RegionUnsupported,
    /// An IP database classifies the address as hosting/data center.
    Hosting,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct Finding {
    verdict: Verdict,
    /// ISO 3166-1 alpha-2 code of the region the service sees.
    region: Option<String>,
    note: Option<Note>,
    ip: Option<String>,
    /// Untranslated facts the service reported: city, network, currency.
    detail: Option<String>,
}

impl Finding {
    fn yes(region: Option<String>) -> Self {
        Self::of(Verdict::Yes, region, None)
    }

    fn no(region: Option<String>, note: Option<Note>) -> Self {
        Self::of(Verdict::No, region, note)
    }

    fn of(verdict: Verdict, region: Option<String>, note: Option<Note>) -> Self {
        Self {
            verdict,
            region,
            note,
            ip: None,
            detail: None,
        }
    }

    fn detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail.filter(|detail| !detail.is_empty());
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub id: &'static str,
    pub verdict: Verdict,
    pub region: Option<String>,
    pub note: Option<Note>,
    pub ip: Option<String>,
    pub detail: Option<String>,
    pub error: Option<String>,
    /// Milliseconds the whole check took.
    pub elapsed: u32,
}

/// The catalog the page lists, and the node tests use unless another is chosen.
pub async fn services(manager: &CoreManager) -> Result<Value> {
    let exit = resolve(manager, None).await.ok();
    Ok(json!({ "services": SERVICES, "exit": exit }))
}

/// Run one service's test through `node` (a group: its current node), or the
/// node the final rule currently leads to.
pub async fn test(manager: &CoreManager, id: &str, node: Option<&str>) -> Result<Outcome> {
    let service = SERVICES
        .iter()
        .find(|service| service.id == id)
        .with_context(|| format!("unknown service '{id}'"))?;
    let exit = resolve(manager, node).await?;
    let core = manager.test_exit(&exit.node).await?;
    let http = Http::new(core.proxy.clone())?;
    Ok(http.run(service).await)
}

/// A node to test through, and the groups that led to it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct TestExit {
    node: String,
    via: Vec<String>,
}

async fn resolve(manager: &CoreManager, name: Option<&str>) -> Result<TestExit> {
    ensure!(manager.status().phase == CorePhase::Running, "core is not running");
    let client = manager.client();
    let query = async {
        let target = match name {
            Some(name) => name.to_owned(),
            None => {
                let mode = client.get_base_config().await?.mode;
                final_target(&manager.runtime_config().await?, mode)
            }
        };
        anyhow::Ok((target, client.get_proxies().await?))
    };
    let (target, live) = tokio::time::timeout(Duration::from_secs(5), query)
        .await
        .map_err(|_| anyhow::anyhow!("core query timed out"))?
        .context("failed to read the core's proxies")?;
    ensure!(live.proxies.contains_key(&target), "proxy '{target}' not found");
    let (via, node) = crate::proxy_probe::follow_groups(&live, &target);
    Ok(TestExit { node, via })
}

/// Where traffic no rule singles out goes: the final `MATCH` rule's target in
/// rule mode.
fn final_target(runtime: &serde_yaml_ng::Mapping, mode: ClashMode) -> String {
    let rule = || {
        runtime
            .get("rules")?
            .as_sequence()?
            .iter()
            .filter_map(serde_yaml_ng::Value::as_str)
            .rev()
            .find_map(|rule| {
                let mut fields = rule.split(',').map(str::trim);
                (fields.next() == Some("MATCH")).then(|| fields.next())?
            })
            .map(str::to_owned)
    };
    match mode {
        ClashMode::Direct => "DIRECT".into(),
        ClashMode::Global => "GLOBAL".into(),
        ClashMode::Rule => rule().unwrap_or_else(|| "GLOBAL".into()),
    }
}

struct Http {
    /// Follows redirects.
    follow: Client,
}

struct Page {
    status: StatusCode,
    url: Url,
    body: String,
}

impl Page {
    fn served(&self) -> Result<()> {
        ensure!(
            self.status.is_success(),
            "unexpected response (HTTP {})",
            self.status.as_u16()
        );
        Ok(())
    }

    fn json(&self) -> Result<Value> {
        self.served()?;
        serde_json::from_str(&self.body)
            .with_context(|| format!("unexpected response (HTTP {}, not JSON)", self.status.as_u16()))
    }

    fn unexpected<T>(&self) -> Result<T> {
        bail!("unexpected response (HTTP {})", self.status.as_u16())
    }
}

impl Http {
    fn new(proxy: reqwest::Proxy) -> Result<Self> {
        let build = || {
            let mut headers = header::HeaderMap::new();
            headers.insert(
                header::ACCEPT_LANGUAGE,
                header::HeaderValue::from_static("en-US,en;q=0.9"),
            );
            Client::builder()
                .proxy(proxy.clone())
                .user_agent(USER_AGENT)
                .default_headers(headers)
                .redirect(redirect::Policy::limited(10))
                // Google resets some large HTTP/2 responses to this client.
                .http1_only()
                .connect_timeout(Duration::from_secs(8))
                .timeout(REQUEST_TIMEOUT)
                .build()
                .context("failed to build the test client")
        };
        Ok(Self { follow: build()? })
    }

    async fn run(&self, service: &Service) -> Outcome {
        let started = Instant::now();
        let result = tokio::time::timeout(CHECK_TIMEOUT, check(self, service.id))
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("test timed out")));
        let elapsed = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
        match result {
            Ok(finding) => Outcome {
                id: service.id,
                verdict: finding.verdict,
                region: finding.region,
                note: finding.note,
                ip: finding.ip,
                detail: finding.detail,
                error: None,
                elapsed,
            },
            Err(error) => Outcome {
                id: service.id,
                verdict: Verdict::Error,
                region: None,
                note: None,
                ip: None,
                detail: None,
                error: Some(format!("{error:#}")),
                elapsed,
            },
        }
    }

    fn get(&self, url: &str) -> RequestBuilder {
        self.follow.get(url)
    }

    async fn send(request: RequestBuilder) -> Result<Page> {
        let mut response = request.send().await.map_err(request_error)?;
        let status = response.status();
        let url = response.url().clone();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(request_error)? {
            body.extend_from_slice(&chunk);
            ensure!(body.len() <= BODY_LIMIT, "response body exceeds the test limit");
        }
        Ok(Page {
            status,
            url,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }

    async fn page(&self, url: &str) -> Result<Page> {
        Self::send(self.get(url)).await
    }

    /// A Cloudflare-fronted host's view of the client.
    async fn trace(&self, host: &str) -> Result<Trace> {
        let page = self.page(&format!("https://{host}/cdn-cgi/trace")).await?;
        page.served()?;
        let field = |key: &str| {
            page.body
                .lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .map(str::to_owned)
        };
        match (field("ip"), field("loc").and_then(|loc| country(&loc))) {
            (Some(ip), Some(loc)) if ip.parse::<std::net::IpAddr>().is_ok() => Ok(Trace {
                ip,
                loc,
                colo: field("colo"),
            }),
            _ => page.unexpected(),
        }
    }
}

struct Trace {
    ip: String,
    loc: String,
    colo: Option<String>,
}

/// The shortest useful reason for a failed request (reqwest nests the cause).
fn request_error(error: reqwest::Error) -> anyhow::Error {
    let kind = if error.is_timeout() {
        "request timed out"
    } else if error.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    let mut cause = std::error::Error::source(&error);
    let mut detail = None;
    while let Some(error) = cause {
        detail = Some(error.to_string());
        cause = error.source();
    }
    match detail {
        Some(detail) => anyhow::anyhow!("{kind}: {detail}"),
        None => anyhow::anyhow!("{kind}"),
    }
}

/// A two-letter region code, upper-cased; `None` for anything else.
fn country(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_alphabetic())).then(|| value.to_ascii_uppercase())
}

/// The text after `prefix` up to the next `"`.
fn quoted_after<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let start = text.find(prefix)? + prefix.len();
    let rest = &text[start..];
    Some(&rest[..rest.find('"')?])
}

fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Non-empty parts joined with `separator`.
fn join(parts: &[Option<String>], separator: &str) -> Option<String> {
    let parts: Vec<&str> = parts
        .iter()
        .flatten()
        .map(String::as_str)
        .filter(|part| !part.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(separator))
}

/// "City, Region · Network" without repeating a region named like its city.
fn place(city: Option<String>, region: Option<String>, network: Option<String>) -> Option<String> {
    let region = region.filter(|region| Some(region) != city.as_ref());
    join(&[join(&[city, region], ", "), network], " · ")
}

async fn check(http: &Http, id: &str) -> Result<Finding> {
    match id {
        "cloudflare" => cloudflare(http).await,
        "google" => google(http).await,
        "ipinfo" => ipinfo(http).await,
        "ip_api" => ip_api(http).await,
        "ip_sb" => ip_sb(http).await,
        "ipwhois" => ipwhois(http).await,
        "apple" => apple(http).await,
        "bing" => bing(http).await,
        "bilibili_hk_mo_tw" => bilibili(http, 18281381, 29892777, 183799).await,
        "bilibili_tw" => bilibili(http, 50762638, 100279344, 268176).await,
        "steam" => steam(http).await,
        _ => bail!("unknown service '{id}'"),
    }
}

async fn cloudflare(http: &Http) -> Result<Finding> {
    let trace = http.trace("www.cloudflare.com").await?;
    Ok(Finding {
        ip: Some(trace.ip),
        ..Finding::yes(Some(trace.loc)).detail(trace.colo.map(|colo| format!("colo {colo}")))
    })
}

/// Google's own placement of the address: the country its home page is served for.
async fn google(http: &Http) -> Result<Finding> {
    let page = Http::send(
        http.get("https://www.google.com/?hl=en")
            .header(header::COOKIE, GOOGLE_CONSENT),
    )
    .await?;
    google_finding(&page)
}

fn google_finding(page: &Page) -> Result<Finding> {
    page.served()?;
    // An arbitrary gl= link can name any country; only Google's placement field counts.
    let region = quoted_after(&page.body, "\"MgUcDb\":\"").and_then(country);
    match region {
        Some(region) => Ok(Finding::yes(Some(region))),
        None => page.unexpected(),
    }
}

async fn ipinfo(http: &Http) -> Result<Finding> {
    let data = http.page("https://ipinfo.io/json").await?.json()?;
    let region = string(&data, "country")
        .as_deref()
        .and_then(country)
        .context("no country in the answer")?;
    Ok(Finding {
        ip: Some(reported_ip(&data, "ip")?),
        ..Finding::yes(Some(region)).detail(place(
            string(&data, "city"),
            string(&data, "region"),
            string(&data, "org"),
        ))
    })
}

async fn ip_api(http: &Http) -> Result<Finding> {
    let data = http
        .page("http://ip-api.com/json/?fields=status,message,countryCode,regionName,city,isp,as,query,proxy,hosting")
        .await?
        .json()?;
    if data["status"] != "success" {
        bail!("{}", string(&data, "message").unwrap_or_else(|| "lookup failed".into()));
    }
    let region = string(&data, "countryCode")
        .as_deref()
        .and_then(country)
        .context("no country in the answer")?;
    let note = (data["hosting"] == true).then_some(Note::Hosting);
    Ok(Finding {
        ip: Some(reported_ip(&data, "query")?),
        note,
        ..Finding::yes(Some(region)).detail(place(
            string(&data, "city"),
            string(&data, "regionName"),
            string(&data, "isp"),
        ))
    })
}

async fn ip_sb(http: &Http) -> Result<Finding> {
    let data = http.page("https://api.ip.sb/geoip").await?.json()?;
    let region = string(&data, "country_code")
        .as_deref()
        .and_then(country)
        .context("no country in the answer")?;
    Ok(Finding {
        ip: Some(reported_ip(&data, "ip")?),
        ..Finding::yes(Some(region)).detail(place(
            string(&data, "city"),
            string(&data, "region"),
            string(&data, "isp"),
        ))
    })
}

async fn ipwhois(http: &Http) -> Result<Finding> {
    let data = http.page("https://ipwho.is/").await?.json()?;
    if data["success"] != true {
        bail!("{}", string(&data, "message").unwrap_or_else(|| "lookup failed".into()));
    }
    let region = string(&data, "country_code")
        .as_deref()
        .and_then(country)
        .context("no country in the answer")?;
    let network = string(&data["connection"], "isp").or_else(|| string(&data["connection"], "org"));
    Ok(Finding {
        ip: Some(reported_ip(&data, "ip")?),
        ..Finding::yes(Some(region)).detail(place(string(&data, "city"), string(&data, "region"), network))
    })
}

/// The country Apple's location service assigns (also used for App Store hints).
async fn apple(http: &Http) -> Result<Finding> {
    let page = http.page("https://gspe1-ssl.ls.apple.com/pep/gcc").await?;
    match country(&page.body) {
        Some(region) if page.status.is_success() => Ok(Finding::yes(Some(region))),
        _ => page.unexpected(),
    }
}

async fn bing(http: &Http) -> Result<Finding> {
    let page = http.page("https://www.bing.com/").await?;
    bing_finding(&page)
}

fn bing_finding(page: &Page) -> Result<Finding> {
    page.served()?;
    // Mainland China is served from its own host.
    if page.url.host_str() == Some("cn.bing.com") {
        return Ok(Finding::yes(Some("CN".into())));
    }
    match quoted_after(&page.body, "Region:\"") {
        // Bing's worldwide market: no country of its own.
        Some("WW") => Ok(Finding::yes(None).detail(Some("WW".into()))),
        Some(region) => match country(region) {
            Some(region) => Ok(Finding::yes(Some(region))),
            None => page.unexpected(),
        },
        None => page.unexpected(),
    }
}

/// Anonymous playback of one sample, including a small fetch from its CDN.
/// This does not establish access to the whole catalog or to paid content.
async fn bilibili(http: &Http, avid: u64, cid: u64, episode: u64) -> Result<Finding> {
    let data = http
        .page(&format!(
            "https://api.bilibili.com/pgc/player/web/playurl?avid={avid}&cid={cid}&qn=16&otype=json&ep_id={episode}&fnval=0&module=bangumi"
        ))
        .await?
        .json()?;
    let Some(url) = bilibili_sample(&data)? else {
        return Ok(Finding::no(None, Some(Note::RegionUnsupported)));
    };
    sample_media(http, &url).await?;
    Ok(Finding::yes(None).detail(Some(format!("ep{episode}"))))
}

async fn sample_media(http: &Http, url: &Url) -> Result<()> {
    let mut response = http
        .get(url.as_str())
        .header(header::REFERER, "https://www.bilibili.com/")
        .header(header::RANGE, "bytes=0-1023")
        .send()
        .await
        .map_err(request_error)?;
    ensure!(
        response.status().is_success(),
        "sample fetch failed (HTTP {})",
        response.status().as_u16()
    );
    let mut prefix = Vec::new();
    while prefix.len() < 32 {
        let Some(chunk) = response.chunk().await.map_err(request_error)? else {
            break;
        };
        prefix.extend_from_slice(&chunk[..chunk.len().min(32 - prefix.len())]);
    }
    ensure!(is_media(&prefix), "sample did not return recognizable media");
    Ok(())
}

fn bilibili_sample(data: &Value) -> Result<Option<Url>> {
    match data["code"].as_i64() {
        Some(0) => {
            // A success envelope without a stream, or an account/member gate,
            // proves nothing. Never promote it to "unlocked".
            let result = &data["result"];
            let preview = &result["is_preview"];
            ensure!(
                preview.is_null() || preview == false || preview == 0,
                "sample is a preview or its access format is unrecognized"
            );
            let stream = result["durl"]
                .as_array()
                .and_then(|streams| streams.first())
                .context("no anonymous sample stream")?;
            let url = string(stream, "url").context("no anonymous sample URL")?;
            let url = Url::parse(&url).context("invalid sample URL")?;
            ensure!(matches!(url.scheme(), "http" | "https"), "invalid sample URL scheme");
            Ok(Some(url))
        }
        Some(-10403) if string(data, "message").is_some_and(|message| message.contains("地区不可观看")) => {
            Ok(None)
        }
        _ => bail!(
            "unexpected response ({})",
            string(data, "message").unwrap_or_else(|| data["code"].to_string())
        ),
    }
}

fn is_media(prefix: &[u8]) -> bool {
    prefix.starts_with(b"FLV") || prefix.get(4..8) == Some(b"ftyp") || prefix.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
}

fn reported_ip(data: &Value, key: &str) -> Result<String> {
    let ip = string(data, key).context("no IP in the answer")?;
    ip.parse::<std::net::IpAddr>().context("invalid IP in the answer")?;
    Ok(ip)
}

/// The store's returned currency; it does not uniquely identify a country.
async fn steam(http: &Http) -> Result<Finding> {
    let page = http.page("https://store.steampowered.com/app/761830").await?;
    steam_finding(&page)
}

fn steam_finding(page: &Page) -> Result<Finding> {
    page.served()?;
    match quoted_after(&page.body, "\"priceCurrency\" content=\"") {
        Some(currency) if currency.len() == 3 && currency.bytes().all(|byte| byte.is_ascii_uppercase()) => {
            Ok(Finding::yes(None).detail(Some(currency.to_owned())))
        }
        _ => page.unexpected(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(status: StatusCode, url: &str, body: &str) -> Page {
        Page {
            status,
            url: Url::parse(url).unwrap(),
            body: body.into(),
        }
    }

    #[test]
    fn catalog_ids_are_unique() {
        let mut ids: Vec<_> = SERVICES.iter().map(|service| service.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SERVICES.len());
    }

    #[test]
    fn tests_default_to_the_final_rule_target() {
        let runtime: serde_yaml_ng::Mapping =
            serde_yaml_ng::from_str("rules: ['DOMAIN-SUFFIX,apple.com,Apple', 'GEOIP,CN,DIRECT', 'MATCH, Final ']")
                .unwrap();
        assert_eq!(final_target(&runtime, ClashMode::Rule), "Final");
        assert_eq!(final_target(&runtime, ClashMode::Global), "GLOBAL");
        assert_eq!(final_target(&runtime, ClashMode::Direct), "DIRECT");
        assert_eq!(final_target(&serde_yaml_ng::Mapping::new(), ClashMode::Rule), "GLOBAL");
    }

    #[test]
    fn helpers_read_markers_and_places() {
        assert_eq!(country(" de "), Some("DE".into()));
        assert_eq!(country("DEU"), None);
        assert_eq!(quoted_after(r#"x "MgUcDb":"JP","y""#, "\"MgUcDb\":\""), Some("JP"));
        assert_eq!(
            place(Some("Tokyo".into()), Some("Tokyo".into()), Some("AS1 Example".into())).as_deref(),
            Some("Tokyo · AS1 Example")
        );
        assert_eq!(place(None, None, None), None);
        let page = Page {
            status: StatusCode::OK,
            url: Url::parse("https://example.com/").unwrap(),
            body: String::new(),
        };
        assert!(page.json().is_err());
    }

    #[test]
    fn unknown_and_blocked_responses_never_prove_public_facts() {
        let body = r#"{"country":"JP","ip":"203.0.113.7"}"#;
        for status in [
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::BAD_GATEWAY,
        ] {
            assert!(page(status, "https://ipinfo.io/json", body).json().is_err());
            assert!(google_finding(&page(status, "https://www.google.com/", r#""MgUcDb":"JP""#)).is_err());
            assert!(bing_finding(&page(status, "https://cn.bing.com/", r#"Region:"CN""#)).is_err());
            assert!(
                steam_finding(&page(
                    status,
                    "https://store.steampowered.com/",
                    r#""priceCurrency" content="USD""#
                ))
                .is_err()
            );
        }
        for body in ["", "Just a moment...", r#"<a href="/?gl=JP">Japan</a>"#] {
            assert!(google_finding(&page(StatusCode::OK, "https://www.google.com/", body)).is_err());
        }
        assert_eq!(
            google_finding(&page(StatusCode::OK, "https://www.google.com/", r#""MgUcDb":"TW""#))
                .unwrap()
                .region
                .as_deref(),
            Some("TW")
        );
        assert!(reported_ip(&json!({"ip":"not an address"}), "ip").is_err());
        assert!(reported_ip(&json!({}), "ip").is_err());
        assert_eq!(reported_ip(&json!({"ip":"2001:db8::1"}), "ip").unwrap(), "2001:db8::1");
    }

    #[test]
    fn bilibili_needs_an_anonymous_stream_and_an_explicit_region_refusal() {
        let success = json!({"code":0,"result":{"durl":[{"url":"https://media.example/sample.mp4"}]}});
        assert!(bilibili_sample(&success).unwrap().is_some());
        assert!(
            bilibili_sample(&json!({"code":-10403,"message":"抱歉您所在地区不可观看！"}))
                .unwrap()
                .is_none()
        );
        for response in [
            json!({"code":0}),
            json!({"code":0,"result":{"durl":[]}}),
            json!({"code":0,"result":{"is_preview":true,"durl":[{"url":"https://media.example/preview"}]}}),
            json!({"code":0,"result":{"is_preview":1,"durl":[{"url":"https://media.example/preview"}]}}),
            json!({"code":0,"result":{"is_preview":"true","durl":[{"url":"https://media.example/preview"}]}}),
            json!({"code":0,"result":{"durl":[{"url":"file:///tmp/video"}]}}),
            json!({"code":-10403,"message":"请先登录"}),
            json!({"code":-10403,"message":"大会员专享"}),
            json!({"code":-412,"message":"请求被拦截"}),
        ] {
            assert!(bilibili_sample(&response).is_err(), "{response}");
        }
    }

    #[test]
    fn sample_requires_media_bytes_and_store_requires_a_currency() {
        for bytes in [
            &b"\0\0\0\x20ftypisom"[..],
            &b"FLV\x01\x05"[..],
            &[0x1a, 0x45, 0xdf, 0xa3][..],
        ] {
            assert!(is_media(bytes));
        }
        for body in ["", "<html>Login required</html>", "Just a moment...", r#"{"code":0}"#] {
            assert!(!is_media(body.as_bytes()));
            assert!(steam_finding(&page(StatusCode::OK, "https://store.steampowered.com/", body)).is_err());
        }
        let store = steam_finding(&page(
            StatusCode::OK,
            "https://store.steampowered.com/",
            r#""priceCurrency" content="USD""#,
        ))
        .unwrap();
        assert_eq!(store.detail.as_deref(), Some("USD"));
        assert_eq!(store.region, None);
    }

    #[tokio::test]
    async fn sample_fetch_rejects_cdn_blocks_and_successful_login_pages() {
        use axum::{Router, extract::Path, routing::get};
        async fn media(Path(kind): Path<String>, headers: header::HeaderMap) -> (StatusCode, &'static [u8]) {
            assert_eq!(headers.get(header::RANGE).unwrap(), "bytes=0-1023");
            assert_eq!(headers.get(header::REFERER).unwrap(), "https://www.bilibili.com/");
            assert!(!headers.contains_key(header::AUTHORIZATION));
            assert!(!headers.contains_key(header::COOKIE));
            match kind.as_str() {
                "media" => (StatusCode::PARTIAL_CONTENT, b"\0\0\0\x20ftypisom"),
                "blocked" => (StatusCode::FORBIDDEN, b"\0\0\0\x20ftypisom"),
                "login" => (StatusCode::OK, b"<html>Please log in</html>"),
                _ => (StatusCode::OK, b""),
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/{kind}", get(media)))
                .await
                .unwrap();
        });
        let http = Http {
            follow: Client::builder().no_proxy().build().unwrap(),
        };
        for kind in ["media", "blocked", "login", "empty"] {
            let url = Url::parse(&format!("http://{address}/{kind}")).unwrap();
            assert_eq!(sample_media(&http, &url).await.is_ok(), kind == "media", "{kind}");
        }
        server.abort();
    }

    #[tokio::test]
    async fn removed_checks_are_rejected() {
        let http = Http::new(reqwest::Proxy::all("http://127.0.0.1:1").unwrap()).unwrap();
        for id in [
            "chatgpt",
            "claude",
            "netflix",
            "disney_plus",
            "youtube_premium",
            "prime_video",
            "hbo_max",
            "dazn",
            "bbc_iplayer",
            "spotify",
            "dmm",
            "dmm_tv",
            "abema",
            "bahamut",
        ] {
            assert!(!SERVICES.iter().any(|service| service.id == id));
            assert!(
                check(&http, id)
                    .await
                    .unwrap_err()
                    .to_string()
                    .starts_with("unknown service")
            );
        }
    }

    /// Runs every check through `MIHOMO_TEST_UNLOCK_PROXY` (an HTTP proxy URL),
    /// and prints the outcomes. No sign-in credentials are used.
    #[tokio::test]
    #[ignore = "needs network access through MIHOMO_TEST_UNLOCK_PROXY"]
    async fn live_checks_through_a_proxy() {
        let proxy = std::env::var("MIHOMO_TEST_UNLOCK_PROXY").expect("MIHOMO_TEST_UNLOCK_PROXY");
        let only = std::env::var("MIHOMO_TEST_UNLOCK_ONLY").ok();
        let http = Http::new(reqwest::Proxy::all(proxy).unwrap()).unwrap();
        let services = SERVICES.iter().filter(|service| {
            only.as_deref()
                .is_none_or(|only| only.split(',').any(|id| id == service.id))
        });
        let outcomes = futures_util::future::join_all(services.map(|service| http.run(service))).await;
        for outcome in &outcomes {
            println!("{}", serde_json::to_string(outcome).unwrap());
        }
        assert!(outcomes.iter().any(|outcome| outcome.verdict != Verdict::Error));
    }
}
