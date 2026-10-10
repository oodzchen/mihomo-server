//! Region-restriction ("unlock") tests: where common overseas services and IP
//! location databases place the proxy, and whether they serve it.
//!
//! Every request goes through one node, in an isolated core that has no rules
//! and bypasses the running core's TUN and listeners (see `proxy_probe`), so the
//! results describe that node alone: neither the active rules, which may send a
//! service to another group or DIRECT, nor the host's own proxying change them.
//! Each check is one small, independent test; the page runs several at once.
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
/// Pages are read up to this size; the markers checked are in the first part.
const BODY_LIMIT: usize = 4 << 20;

/// Regions under broad sanctions or service bans: no tested service serves them.
const SANCTIONED: &[&str] = &["CN", "RU", "BY", "IR", "KP", "SY", "CU"];
/// OpenAI, Anthropic and Google's Gemini/AI Studio also exclude Hong Kong and Macau.
const AI_RESTRICTED: &[&str] = &["CN", "HK", "MO", "RU", "BY", "IR", "KP", "SY", "CU"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// IP location databases and what large platforms think the location is.
    Location,
    Streaming,
    Ai,
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
    service("netflix", "Netflix", Category::Streaming),
    service("disney_plus", "Disney+", Category::Streaming),
    service("youtube_premium", "YouTube Premium", Category::Streaming),
    service("prime_video", "Amazon Prime Video", Category::Streaming),
    service("hbo_max", "HBO Max", Category::Streaming),
    service("hulu", "Hulu", Category::Streaming),
    service("paramount_plus", "Paramount+", Category::Streaming),
    service("peacock", "Peacock", Category::Streaming),
    service("dazn", "DAZN", Category::Streaming),
    service("bbc_iplayer", "BBC iPlayer", Category::Streaming),
    service("spotify", "Spotify", Category::Streaming),
    service("tiktok", "TikTok", Category::Streaming),
    service("dmm", "DMM", Category::Streaming),
    service("dmm_tv", "DMM TV", Category::Streaming),
    service("abema", "AbemaTV", Category::Streaming),
    service("bahamut", "Bahamut Anime", Category::Streaming),
    service("bilibili_hk_mo_tw", "Bilibili HK/MO/TW", Category::Streaming),
    service("bilibili_tw", "Bilibili TW", Category::Streaming),
    service("chatgpt", "ChatGPT", Category::Ai),
    service("claude", "Claude", Category::Ai),
    service("gemini", "Gemini", Category::Ai),
    service("ai_studio", "Google AI Studio", Category::Ai),
    service("copilot", "Microsoft Copilot", Category::Ai),
    service("grok", "Grok", Category::Ai),
    service("perplexity", "Perplexity", Category::Ai),
    service("steam", "Steam", Category::Other),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Served (for location services: answered).
    Yes,
    /// Served with a limitation the note names.
    Partial,
    No,
    /// The test itself failed: network error, timeout or unrecognized answer.
    Error,
}

/// Why a service is limited or refused, for the page to explain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Note {
    /// Netflix: only Netflix's own productions.
    OriginalsOnly,
    /// ChatGPT: the website works, the mobile app endpoint refuses the address.
    AppBlocked,
    /// The region the service sees is outside the regions it serves.
    RegionUnsupported,
    /// Disney+: the region is known but not launched yet.
    ComingSoon,
    /// The service recognized a proxy, VPN or data-center address.
    ProxyDetected,
    /// An IP database classifies the address as hosting/data center.
    Hosting,
    /// The service's international edition, not the local one.
    Overseas,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
struct Finding {
    verdict: Option<Verdict>,
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
            verdict: Some(verdict),
            region,
            note,
            ..Self::default()
        }
    }

    /// Served unless the region is one the service excludes.
    fn unless_in(region: String, excluded: &[&str]) -> Self {
        if excluded.contains(&region.as_str()) {
            Self::no(Some(region), Some(Note::RegionUnsupported))
        } else {
            Self::yes(Some(region))
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
    /// Returns redirects, for checks that read where a service sends visitors.
    manual: Client,
}

struct Page {
    status: StatusCode,
    url: Url,
    location: Option<String>,
    cookies: Vec<String>,
    body: String,
}

impl Page {
    fn json(&self) -> Result<Value> {
        serde_json::from_str(&self.body)
            .with_context(|| format!("unexpected response (HTTP {}, not JSON)", self.status.as_u16()))
    }

    /// The response's cookies as a request `Cookie` header; a cookie set
    /// twice keeps its last value, as in a browser.
    fn cookie_header(&self) -> String {
        let mut cookies: Vec<(&str, &str)> = Vec::new();
        for pair in self.cookies.iter().filter_map(|cookie| cookie.split(';').next()) {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            cookies.retain(|(existing, _)| *existing != name);
            cookies.push((name, value));
        }
        cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn unexpected<T>(&self) -> Result<T> {
        bail!("unexpected response (HTTP {})", self.status.as_u16())
    }
}

impl Http {
    fn new(proxy: reqwest::Proxy) -> Result<Self> {
        let build = |policy| {
            let mut headers = header::HeaderMap::new();
            headers.insert(
                header::ACCEPT_LANGUAGE,
                header::HeaderValue::from_static("en-US,en;q=0.9"),
            );
            Client::builder()
                .proxy(proxy.clone())
                .user_agent(USER_AGENT)
                .default_headers(headers)
                .redirect(policy)
                // Google resets some large HTTP/2 responses to this client.
                .http1_only()
                .connect_timeout(Duration::from_secs(8))
                .timeout(REQUEST_TIMEOUT)
                .build()
                .context("failed to build the test client")
        };
        Ok(Self {
            follow: build(redirect::Policy::limited(10))?,
            manual: build(redirect::Policy::none())?,
        })
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
                verdict: finding.verdict.unwrap_or(Verdict::Yes),
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
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok().map(str::to_owned))
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(request_error)? {
            body.extend_from_slice(&chunk);
            if body.len() >= BODY_LIMIT {
                break;
            }
        }
        Ok(Page {
            status,
            url,
            location,
            cookies,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }

    async fn page(&self, url: &str) -> Result<Page> {
        Self::send(self.get(url)).await
    }

    /// A Cloudflare-fronted host's view of the client.
    async fn trace(&self, host: &str) -> Result<Trace> {
        let page = self.page(&format!("https://{host}/cdn-cgi/trace")).await?;
        let field = |key: &str| {
            page.body
                .lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .map(str::to_owned)
        };
        match (field("ip"), field("loc").and_then(|loc| country(&loc))) {
            (Some(ip), Some(loc)) => Ok(Trace {
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

const ISO3: &str = concat!(
    "ABWAWAFGAFAGOAOAIAAIALAAXALBALANDADAREAEARGARARMAMASMASATAAQATFTFATGAGAUSAUAUTATAZEAZBDIBIBELBEBENBJ",
    "BESBQBFABFBGDBDBGRBGBHRBHBHSBSBIHBABLMBLBLRBYBLZBZBMUBMBOLBOBRABRBRBBBBRNBNBTNBTBVTBVBWABWCAFCFCANCA",
    "CCKCCCHECHCHLCLCHNCNCIVCICMRCMCODCDCOGCGCOKCKCOLCOCOMKMCPVCVCRICRCUBCUCUWCWCXRCXCYMKYCYPCYCZECZDEUDE",
    "DJIDJDMADMDNKDKDOMDODZADZECUECEGYEGERIERESHEHESPESESTEEETHETFINFIFJIFJFLKFKFRAFRFROFOFSMFMGABGAGBRGB",
    "GEOGEGGYGGGHAGHGIBGIGINGNGLPGPGMBGMGNBGWGNQGQGRCGRGRDGDGRLGLGTMGTGUFGFGUMGUGUYGYHKGHKHMDHMHNDHNHRVHR",
    "HTIHTHUNHUIDNIDIMNIMINDINIOTIOIRLIEIRNIRIRQIQISLISISRILITAITJAMJMJEYJEJORJOJPNJPKAZKZKENKEKGZKGKHMKH",
    "KIRKIKNAKNKORKRKWTKWLAOLALBNLBLBRLRLBYLYLCALCLIELILKALKLSOLSLTULTLUXLULVALVMACMOMAFMFMARMAMCOMCMDAMD",
    "MDGMGMDVMVMEXMXMHLMHMKDMKMLIMLMLTMTMMRMMMNEMEMNGMNMNPMPMOZMZMRTMRMSRMSMTQMQMUSMUMWIMWMYSMYMYTYTNAMNA",
    "NCLNCNERNENFKNFNGANGNICNINIUNUNLDNLNORNONPLNPNRUNRNZLNZOMNOMPAKPKPANPAPCNPNPERPEPHLPHPLWPWPNGPGPOLPL",
    "PRIPRPRKKPPRTPTPRYPYPSEPSPYFPFQATQAREUREROURORUSRURWARWSAUSASDNSDSENSNSGPSGSGSGSSHNSHSJMSJSLBSBSLESL",
    "SLVSVSMRSMSOMSOSPMPMSRBRSSSDSSSTPSTSURSRSVKSKSVNSISWESESWZSZSXMSXSYCSCSYRSYTCATCTCDTDTGOTGTHATHTJKTJ",
    "TKLTKTKMTMTLSTLTONTOTTOTTTUNTNTURTRTUVTVTWNTWTZATZUGAUGUKRUAUMIUMURYUYUSAUSUZBUZVATVAVCTVCVENVEVGBVG",
    "VIRVIVNMVNVUTVUWLFWFWSMWSYEMYEZAFZAZMBZMZWEZW",
);

/// ISO 3166-1 alpha-3 to alpha-2.
fn alpha2(alpha3: &str) -> Option<String> {
    let (entries, _) = ISO3.as_bytes().as_chunks::<5>();
    entries
        .iter()
        .find(|entry| entry[..3] == *alpha3.as_bytes())
        .map(|entry| String::from_utf8_lossy(&entry[3..]).into_owned())
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
        "netflix" => netflix(http).await,
        "disney_plus" => disney_plus(http).await,
        "youtube_premium" => youtube_premium(http).await,
        "prime_video" => prime_video(http).await,
        "hbo_max" => hbo_max(http).await,
        "hulu" => hulu(http).await,
        "paramount_plus" => paramount_plus(http).await,
        "peacock" => peacock(http).await,
        "dazn" => dazn(http).await,
        "bbc_iplayer" => bbc_iplayer(http).await,
        "spotify" => spotify(http).await,
        "tiktok" => tiktok(http).await,
        "dmm" => dmm(http).await,
        "dmm_tv" => dmm_tv(http).await,
        "abema" => abema(http).await,
        "bahamut" => bahamut(http).await,
        "bilibili_hk_mo_tw" => bilibili(http, 18281381, 29892777, 183799).await,
        "bilibili_tw" => bilibili(http, 50762638, 100279344, 268176).await,
        "chatgpt" => chatgpt(http).await,
        "claude" => claude(http).await,
        "gemini" => gemini(http).await,
        "ai_studio" => ai_studio(http).await,
        "copilot" => traced(http, "copilot.microsoft.com", SANCTIONED).await,
        "grok" => traced(http, "grok.com", SANCTIONED).await,
        "perplexity" => traced(http, "www.perplexity.ai", SANCTIONED).await,
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
    let region = quoted_after(&page.body, "\"MgUcDb\":\"").and_then(country).or_else(|| {
        let start = page.body.find("gl=")? + 3;
        page.body.get(start..start + 2).and_then(country)
    });
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
        ip: string(&data, "ip"),
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
    let note = (data["hosting"] == true || data["proxy"] == true).then_some(Note::Hosting);
    Ok(Finding {
        ip: string(&data, "query"),
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
        ip: string(&data, "ip"),
        ..Finding::yes(Some(region)).detail(place(
            string(&data, "city"),
            string(&data, "region"),
            string(&data, "isp"),
        ))
    })
}

async fn ipwhois(http: &Http) -> Result<Finding> {
    let data = http.page("https://ipwho.is/").await?.json()?;
    if data["success"] == false {
        bail!("{}", string(&data, "message").unwrap_or_else(|| "lookup failed".into()));
    }
    let region = string(&data, "country_code")
        .as_deref()
        .and_then(country)
        .context("no country in the answer")?;
    let network = string(&data["connection"], "isp").or_else(|| string(&data["connection"], "org"));
    Ok(Finding {
        ip: string(&data, "ip"),
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

/// A licensed title tells full access from Netflix-originals-only.
async fn netflix(http: &Http) -> Result<Finding> {
    for title in ["81280792", "70143836"] {
        let page = http.page(&format!("https://www.netflix.com/title/{title}")).await?;
        match page.status {
            StatusCode::OK => {
                // Localized URLs name the country: /jp-en/title/…, /tw/title/….
                let from_path = page
                    .url
                    .path_segments()
                    .and_then(|mut segments| segments.next())
                    .and_then(|segment| segment.split('-').next())
                    .and_then(country);
                let region = from_path.or_else(|| {
                    quoted_after(&page.body, "\"requestCountry\":{\"id\":\"")
                        .or_else(|| quoted_after(&page.body, "\"country\":\""))
                        .and_then(country)
                });
                return Ok(Finding::yes(region));
            }
            StatusCode::NOT_FOUND => {}
            StatusCode::FORBIDDEN => return Ok(Finding::no(None, None)),
            _ => return page.unexpected(),
        }
    }
    Ok(Finding::of(Verdict::Partial, None, Some(Note::OriginalsOnly)))
}

/// Disney's device-token exchange, then the session's location.
async fn disney_plus(http: &Http) -> Result<Finding> {
    const KEY: &str = "ZGlzbmV5JmJyb3dzZXImMS4wLjA.Cu56AgSfBTDag5NiRA81oLHkDZfu5L3CKadnefEAY84";
    let api = "https://disney.api.edge.bamgrid.com";
    let device = Http::send(
        http.follow
            .post(format!("{api}/devices"))
            .bearer_auth(KEY)
            .json(&json!({"deviceFamily": "browser", "applicationRuntime": "chrome", "deviceProfile": "windows", "attributes": {}})),
    )
    .await?;
    if device.status == StatusCode::FORBIDDEN {
        return Ok(Finding::no(None, None));
    }
    let assertion = string(&device.json()?, "assertion").context("no device assertion")?;
    let form = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange"),
            ("latitude", "0"),
            ("longitude", "0"),
            ("platform", "browser"),
            ("subject_token", assertion.as_str()),
            ("subject_token_type", "urn:bamtech:params:oauth:token-type:device"),
        ])
        .finish();
    let token = Http::send(
        http.follow
            .post(format!("{api}/token"))
            .bearer_auth(KEY)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form),
    )
    .await?;
    if token.body.contains("forbidden-location") {
        return Ok(Finding::no(None, Some(Note::ProxyDetected)));
    }
    if token.status == StatusCode::FORBIDDEN {
        return Ok(Finding::no(None, None));
    }
    let refresh = string(&token.json()?, "refresh_token").context("no refresh token")?;
    let query = "mutation refreshToken($input: RefreshTokenInput!) { refreshToken(refreshToken: $input) { activeSession { sessionId } } }";
    let session = Http::send(
        http.follow
            .post(format!("{api}/graph/v1/device/graphql"))
            .header(header::AUTHORIZATION, KEY)
            .json(&json!({"query": query, "variables": {"input": {"refreshToken": refresh}}})),
    )
    .await?
    .json()?;
    let session = &session["extensions"]["sdk"]["session"];
    let region = string(&session["location"], "countryCode").as_deref().and_then(country);
    let supported = session["inSupportedLocation"].as_bool();
    let preview = http.page("https://www.disneyplus.com/").await?;
    if preview.url.path().contains("unavailable") {
        return Ok(Finding::no(region, Some(Note::RegionUnsupported)));
    }
    match supported {
        Some(true) => Ok(Finding::yes(region)),
        // Japan's Disney+ runs on a partner platform and reports false.
        Some(false) if region.as_deref() == Some("JP") => Ok(Finding::yes(region)),
        Some(false) => Ok(Finding::of(Verdict::Partial, region, Some(Note::ComingSoon))),
        None => bail!("unexpected response (no session location)"),
    }
}

async fn youtube_premium(http: &Http) -> Result<Finding> {
    let page = Http::send(
        http.get("https://www.youtube.com/premium")
            .header(header::COOKIE, GOOGLE_CONSENT),
    )
    .await?;
    let region = quoted_after(&page.body, "\"INNERTUBE_CONTEXT_GL\":\"")
        .or_else(|| quoted_after(&page.body, "\"countryCode\":\""))
        .and_then(country);
    if page.body.contains("Premium is not available in your country") || region.as_deref() == Some("CN") {
        return Ok(Finding::no(region, Some(Note::RegionUnsupported)));
    }
    match region {
        Some(region) if page.status.is_success() => Ok(Finding::yes(Some(region))),
        _ => page.unexpected(),
    }
}

async fn prime_video(http: &Http) -> Result<Finding> {
    let page = http.page("https://www.primevideo.com/").await?;
    if page.body.contains("isServiceRestricted\":true") {
        return Ok(Finding::no(None, Some(Note::RegionUnsupported)));
    }
    match quoted_after(&page.body, "\"currentTerritory\":\"").and_then(country) {
        Some(region) => Ok(Finding::yes(Some(region))),
        None if page.status.is_success() => Ok(Finding::no(None, None)),
        None => page.unexpected(),
    }
}

/// An anonymous session's home market, then the territory it was placed in.
async fn hbo_max(http: &Http) -> Result<Finding> {
    let headers = |request: RequestBuilder| {
        request
            .header(
                "x-device-info",
                "beam/5.0.0 (desktop/desktop; Windows/10; afbb9b4d-5bd5-4a3e-a4ee-d5fc95e78c1c/da0cdd94-5a39-42ef-aa68-54cbc1b852c3)",
            )
            .header("x-disco-client", "WEB:10:beam:5.2.1")
            .header("x-disco-params", "realm=bolt,bid=beam,features=ar")
    };
    let mut device = [0u8; 16];
    getrandom::fill(&mut device).context("generate a device id")?;
    let device: String = device.iter().map(|byte| format!("{byte:02x}")).collect();
    let token = Http::send(headers(http.get(&format!(
        "https://default.any-any.prd.api.hbomax.com/token?realm=bolt&deviceId={device}"
    ))))
    .await?;
    if !token.status.is_success() {
        return token.unexpected();
    }
    let cookie = token.cookie_header();
    let bootstrap = Http::send(
        headers(
            http.follow
                .post("https://default.any-any.prd.api.hbomax.com/session-context/headwaiter/v1/bootstrap"),
        )
        .header(header::COOKIE, &cookie),
    )
    .await?
    .json()?;
    let routing = &bootstrap["routing"];
    let (Some(tenant), Some(market)) = (string(routing, "tenant"), string(routing, "homeMarket")) else {
        return Ok(Finding::no(None, Some(Note::RegionUnsupported)));
    };
    let me = Http::send(
        headers(http.get(&format!(
            "https://default.{tenant}-{market}.prd.api.hbomax.com/users/me"
        )))
        .header(header::COOKIE, &cookie),
    )
    .await?;
    let region = quoted_after(&me.body.replace(' ', ""), "\"currentLocationTerritory\":\"").and_then(country);
    let Some(region) = region else {
        return me.unexpected();
    };
    // The site links every market it serves: /us/en, /tw/zh, ….
    let site = http.page("https://www.hbomax.com/").await?;
    let served = site
        .body
        .contains(&format!("\"url\":\"/{}/", region.to_ascii_lowercase()));
    Ok(if served || !site.body.contains("\"url\":\"/us/") {
        Finding::yes(Some(region))
    } else {
        Finding::no(Some(region), Some(Note::RegionUnsupported))
    })
}

/// Hulu (US) sends visitors elsewhere to Disney+.
async fn hulu(http: &Http) -> Result<Finding> {
    let page = Http::send(http.manual.get("https://www.hulu.com/welcome")).await?;
    match page.status {
        StatusCode::OK => Ok(Finding::yes(Some("US".into()))),
        status if status.is_redirection() => Ok(Finding::no(None, Some(Note::RegionUnsupported))),
        StatusCode::FORBIDDEN => Ok(Finding::no(None, Some(Note::ProxyDetected))),
        _ => page.unexpected(),
    }
}

/// Paramount+ serves its US site at the root and redirects other markets.
async fn paramount_plus(http: &Http) -> Result<Finding> {
    let page = Http::send(http.manual.get("https://www.paramountplus.com/")).await?;
    if page.status == StatusCode::OK {
        return Ok(Finding::yes(Some("US".into())));
    }
    let Some(location) = page.location.as_deref().filter(|_| page.status.is_redirection()) else {
        return page.unexpected();
    };
    let path = Url::parse(location)
        .or_else(|_| page.url.join(location))
        .map(|url| url.path().to_owned())
        .unwrap_or_default();
    let segment = path.trim_matches('/').split('/').next().unwrap_or_default();
    match country(segment) {
        Some(region) => Ok(Finding::yes(Some(region))),
        None => Ok(Finding::no(None, Some(Note::RegionUnsupported))),
    }
}

async fn peacock(http: &Http) -> Result<Finding> {
    let page = http.page("https://www.peacocktv.com/").await?;
    if page.url.path().contains("unavailable") {
        Ok(Finding::no(None, Some(Note::RegionUnsupported)))
    } else if page.status.is_success() {
        Ok(Finding::yes(Some("US".into())))
    } else {
        page.unexpected()
    }
}

async fn dazn(http: &Http) -> Result<Finding> {
    let page = Http::send(
        http.follow
            .post("https://startup.core.indazn.com/misl/v5/Startup")
            .json(&json!({
                "LandingPageKey": "generic", "languages": "en-US,en", "Platform": "web",
                "PlatformAttributes": {}, "Manufacturer": "", "PromoCode": "", "Version": "2"
            })),
    )
    .await?;
    if page.status == StatusCode::FORBIDDEN {
        return Ok(Finding::no(None, Some(Note::ProxyDetected)));
    }
    let data = page.json()?;
    let region = &data["Region"];
    let country = string(region, "GeolocatedCountry").as_deref().and_then(country);
    match region["isAllowed"].as_bool() {
        Some(true) => Ok(Finding::yes(country)),
        Some(false) => Ok(Finding::no(country, Some(Note::RegionUnsupported))),
        None => page.unexpected(),
    }
}

async fn bbc_iplayer(http: &Http) -> Result<Finding> {
    let page = http
        .page("https://open.live.bbc.co.uk/mediaselector/6/select/version/2.0/mediaset/pc/vpid/bbc_one_london/format/json/jsfunc/JS_callbacks0")
        .await?;
    if page.body.contains("\"result\":\"geolocation\"") {
        Ok(Finding::no(None, Some(Note::RegionUnsupported)))
    } else if page.body.contains("\"connection\"") {
        Ok(Finding::yes(Some("GB".into())))
    } else {
        page.unexpected()
    }
}

/// Spotify sends visitors to their market's pages: /jp/signup/, /hk-zh/signup/.
async fn spotify(http: &Http) -> Result<Finding> {
    let page = Http::send(http.manual.get("https://www.spotify.com/signup")).await?;
    let location = page.location.as_deref().unwrap_or_default();
    let path = page
        .url
        .join(location)
        .map(|url| url.path().to_owned())
        .unwrap_or_default();
    let segment = path.trim_matches('/').split('/').next().unwrap_or_default();
    match segment.split('-').next().and_then(country) {
        Some(region) => Ok(Finding::unless_in(region, SANCTIONED)),
        None if page.status.is_redirection() || page.status.is_success() => Ok(Finding::no(None, None)),
        None => page.unexpected(),
    }
}

async fn tiktok(http: &Http) -> Result<Finding> {
    let page = http.page("https://www.tiktok.com/explore").await?;
    let mut region = None;
    let mut rest = page.body.as_str();
    while let Some(start) = rest.find("\"region\":\"") {
        rest = &rest[start + 10..];
        if let Some(code) = rest
            .get(..3)
            .filter(|code| code.ends_with('"'))
            .and_then(|code| country(&code[..2]))
        {
            region = Some(code);
            break;
        }
    }
    match region {
        // Withdrawn from Hong Kong, banned in India and the mainland runs Douyin.
        Some(region) => Ok(Finding::unless_in(region, &["CN", "HK", "IN"])),
        None if page.status.is_success() => Ok(Finding::no(None, None)),
        None => page.unexpected(),
    }
}

/// Visitors from abroad get DMM's international (English) edition.
async fn dmm(http: &Http) -> Result<Finding> {
    let page = http.page("https://www.dmm.co.jp/top/").await?;
    if !page.status.is_success() {
        return page.unexpected();
    }
    if page.url.path().starts_with("/en/") || page.body.contains("not-available-in-your-region") {
        Ok(Finding::no(None, Some(Note::Overseas)))
    } else {
        Ok(Finding::yes(Some("JP".into())))
    }
}

async fn dmm_tv(http: &Http) -> Result<Finding> {
    let data = Http::send(
        http.follow
            .post("https://api.beacon.dmm.com/v1/streaming/start")
            .json(&json!({
                "player_name": "dmmtv_browser", "player_version": "0.0.0", "content_type_detail": "VOD_SVOD",
                "content_id": "11uvjcm4fw2wdu7drtd1epnvz", "purchase_product_id": null
            })),
    )
    .await?
    .json()?;
    match data["block_status"].as_str() {
        // Not logged in, but past the region check.
        Some("UNAUTHORIZED") => Ok(Finding::yes(Some("JP".into()))),
        Some("FOREIGN") => Ok(Finding::no(None, Some(Note::RegionUnsupported))),
        Some(_) => Ok(Finding::no(None, Some(Note::ProxyDetected))),
        None => bail!("unexpected response"),
    }
}

async fn abema(http: &Http) -> Result<Finding> {
    let page = http.page("https://api.abema.io/v1/ip/check?device=android").await?;
    if page.body.contains("anonymous_ip") {
        return Ok(Finding::no(None, Some(Note::ProxyDetected)));
    }
    match string(&page.json()?, "isoCountryCode").as_deref().and_then(country) {
        Some(region) if region == "JP" => Ok(Finding::yes(Some(region))),
        // Abroad only the free international lineup is offered.
        Some(region) => Ok(Finding::of(Verdict::Partial, Some(region), Some(Note::Overseas))),
        None => page.unexpected(),
    }
}

/// Bahamut's player token for a device registered from this address.
async fn bahamut(http: &Http) -> Result<Finding> {
    let device = http.page("https://ani.gamer.com.tw/ajax/getdeviceid.php").await?;
    if device.status == StatusCode::FORBIDDEN {
        return Ok(Finding::no(None, Some(Note::ProxyDetected)));
    }
    let cookie = device.cookie_header();
    let id = string(&device.json()?, "deviceid").context("no device id")?;
    let token = Http::send(
        http.get(&format!(
            "https://ani.gamer.com.tw/ajax/token.php?adID=89422&sn=37783&device={id}"
        ))
        .header(header::COOKIE, &cookie),
    )
    .await?;
    if !token.body.contains("animeSn") {
        return Ok(Finding::no(None, Some(Note::RegionUnsupported)));
    }
    let home = Http::send(http.get("https://ani.gamer.com.tw/").header(header::COOKIE, &cookie)).await?;
    Ok(Finding::yes(quoted_after(&home.body, "data-geo=\"").and_then(country)))
}

/// A title limited to the regions the endpoint stands for.
async fn bilibili(http: &Http, avid: u64, cid: u64, episode: u64) -> Result<Finding> {
    let data = http
        .page(&format!(
            "https://api.bilibili.com/pgc/player/web/playurl?avid={avid}&cid={cid}&qn=0&type=&otype=json&ep_id={episode}&fourk=1&fnver=0&fnval=16&module=bangumi"
        ))
        .await?
        .json()?;
    match data["code"].as_i64() {
        Some(0) => Ok(Finding::yes(None)),
        Some(-10403) => Ok(Finding::no(None, Some(Note::RegionUnsupported))),
        _ => bail!(
            "unexpected response ({})",
            string(&data, "message").unwrap_or_else(|| data["code"].to_string())
        ),
    }
}

/// Where OpenAI places the address, and whether its app endpoint refuses it.
async fn chatgpt(http: &Http) -> Result<Finding> {
    let trace = http.trace("chatgpt.com").await?;
    let compliance = Http::send(
        http.get("https://api.openai.com/compliance/cookie_requirements")
            .header(header::AUTHORIZATION, "Bearer null"),
    )
    .await?;
    let mut finding = if compliance.body.contains("unsupported_country") {
        Finding::no(Some(trace.loc), Some(Note::RegionUnsupported))
    } else {
        Finding::unless_in(trace.loc, AI_RESTRICTED)
    };
    if finding.verdict == Some(Verdict::Yes) {
        let app = http.page("https://ios.chat.openai.com/").await?;
        if app.status == StatusCode::FORBIDDEN && (app.body.contains("cf_details") || app.body.contains("VPN")) {
            finding = Finding::of(Verdict::Partial, finding.region, Some(Note::AppBlocked));
        }
    }
    Ok(Finding {
        ip: Some(trace.ip),
        ..finding
    })
}

async fn claude(http: &Http) -> Result<Finding> {
    let trace = http.trace("claude.ai").await?;
    let page = Http::send(http.manual.get("https://claude.ai/")).await?;
    let finding = if page
        .location
        .as_deref()
        .is_some_and(|location| location.contains("unavailable-in-region"))
    {
        Finding::no(Some(trace.loc), Some(Note::RegionUnsupported))
    } else {
        Finding::unless_in(trace.loc, AI_RESTRICTED)
    };
    Ok(Finding {
        ip: Some(trace.ip),
        ..finding
    })
}

/// The region Gemini's page is served for (ISO alpha-3 in its bootstrap data).
async fn gemini_region(http: &Http) -> Result<String> {
    let page = Http::send(
        http.get("https://gemini.google.com/")
            .header(header::COOKIE, GOOGLE_CONSENT),
    )
    .await?;
    let start = page.body.find(",2,1,200,\"").map(|start| start + 10);
    match start.and_then(|start| page.body.get(start..start + 3)).and_then(alpha2) {
        Some(region) => Ok(region),
        None => page.unexpected(),
    }
}

async fn gemini(http: &Http) -> Result<Finding> {
    Ok(Finding::unless_in(gemini_region(http).await?, AI_RESTRICTED))
}

async fn ai_studio(http: &Http) -> Result<Finding> {
    let page = Http::send(http.manual.get("https://aistudio.google.com/")).await?;
    let location = page.location.as_deref().unwrap_or_default();
    if location.contains("available-regions") || location.contains("unsupported") {
        return Ok(Finding::no(None, Some(Note::RegionUnsupported)));
    }
    if !(page.status.is_success() || page.status.is_redirection()) {
        return page.unexpected();
    }
    Ok(Finding::unless_in(gemini_region(http).await?, AI_RESTRICTED))
}

/// A Cloudflare-fronted service judged by the region it sees.
async fn traced(http: &Http, host: &str, excluded: &[&str]) -> Result<Finding> {
    let trace = http.trace(host).await?;
    Ok(Finding {
        ip: Some(trace.ip),
        ..Finding::unless_in(trace.loc, excluded)
    })
}

/// The store currency shows the region Steam prices for.
async fn steam(http: &Http) -> Result<Finding> {
    let page = http.page("https://store.steampowered.com/app/761830").await?;
    match quoted_after(&page.body, "\"priceCurrency\" content=\"") {
        Some(currency) if !currency.is_empty() => Ok(Finding::yes(None).detail(Some(currency.to_owned()))),
        _ => page.unexpected(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique_and_dispatched() {
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
    fn alpha3_codes_map_to_alpha2() {
        assert_eq!(alpha2("USA").as_deref(), Some("US"));
        assert_eq!(alpha2("HKG").as_deref(), Some("HK"));
        assert_eq!(alpha2("DEU").as_deref(), Some("DE"));
        assert_eq!(alpha2("ZWE").as_deref(), Some("ZW"));
        assert_eq!(alpha2("XXX"), None);
        assert_eq!(ISO3.len() % 5, 0);
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
            location: None,
            cookies: vec!["a=1; path=/".into(), "b=2".into(), "a=3; Max-Age=60".into()],
            body: String::new(),
        };
        assert_eq!(page.cookie_header(), "b=2; a=3");
        let finding = Finding::unless_in("HK".into(), AI_RESTRICTED);
        assert_eq!(finding.verdict, Some(Verdict::No));
        assert_eq!(finding.note, Some(Note::RegionUnsupported));
    }

    /// Runs every check through `MIHOMO_TEST_UNLOCK_PROXY` (an HTTP proxy URL)
    /// and prints the outcomes.
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
