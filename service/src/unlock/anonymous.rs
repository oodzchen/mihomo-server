//! Individually scoped checks of public fields, region gates and offers.
//! None use a signed-in user's cookies, account or subscription.
use super::*;

fn location(region: String, note: Note) -> Finding {
    Finding::of(Verdict::Yes, Some(region), Some(note))
}

fn region_gate(allowed: bool, region: Option<String>) -> Finding {
    Finding::of(
        if allowed { Verdict::Yes } else { Verdict::No },
        region,
        Some(if allowed {
            Note::RegionCheckOnly
        } else {
            Note::RegionUnsupported
        }),
    )
}

/// Netflix's nonmember page contains requestCountry, not playback rights.
pub(super) async fn netflix(http: &Http) -> Result<Finding> {
    netflix_finding(&http.page("https://www.netflix.com/title/81280792").await?)
}

fn netflix_finding(page: &Page) -> Result<Finding> {
    page.served()?;
    ensure!(
        page.url.host_str() == Some("www.netflix.com"),
        "unrecognized Netflix host"
    );
    // This flat JavaScript object contains non-JSON \\x escapes in countryName.
    // Read only its explicit id; arbitrary country fields and URL locales are
    // not evidence of request placement. Require a complete object.
    let object = page
        .body
        .split_once("\"requestCountry\":{")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(object, _)| object)
        .context("Netflix returned no requestCountry object")?;
    ensure!(!object.contains('{'), "Netflix returned a nested requestCountry object");
    let region = quoted_after(object, "\"id\":\"")
        .and_then(country)
        .context("Netflix returned no requestCountry.id")?;
    Ok(location(region, Note::LocationOnly))
}

pub(super) async fn prime_video(http: &Http) -> Result<Finding> {
    prime_video_finding(&http.page("https://www.primevideo.com/").await?)
}

fn prime_video_finding(page: &Page) -> Result<Finding> {
    page.served()?;
    ensure!(
        page.url.host_str() == Some("www.primevideo.com"),
        "unrecognized Prime Video host"
    );
    let region = quoted_after(&page.body, "\"currentTerritory\":\"").and_then(country);
    let restricted = page.body.split_once("\"isServiceRestricted\":").and_then(|(_, tail)| {
        serde_json::Deserializer::from_str(tail)
            .into_iter::<Value>()
            .next()
            .and_then(Result::ok)
            .and_then(|value| value.as_bool())
    });
    if page.url.path().contains("servicerestricted") || restricted == Some(true) {
        return Ok(region_gate(false, region));
    }
    // countryCode also appears in content ratings and must not be used here.
    Ok(location(
        region.context("Prime Video returned no currentTerritory")?,
        Note::LocationOnly,
    ))
}

/// A signup-market redirect is useful public information, not a signup test.
pub(super) async fn spotify(http: &Http) -> Result<Finding> {
    spotify_finding(&Http::send(http.manual.get("https://www.spotify.com/signup")).await?)
}

fn spotify_finding(page: &Page) -> Result<Finding> {
    ensure!(
        matches!(page.status.as_u16(), 301 | 302 | 303 | 307 | 308),
        "Spotify returned no market redirect (HTTP {})",
        page.status.as_u16()
    );
    let target = page
        .url
        .join(page.location.as_deref().context("Spotify returned no Location")?)?;
    ensure!(
        target.scheme() == "https" && target.host_str() == Some("www.spotify.com"),
        "unrecognized Spotify redirect host"
    );
    if target.path().split('/').any(|segment| segment == "why-not-available") {
        return Ok(region_gate(false, None));
    }
    let mut segments = target.path().trim_matches('/').split('/');
    let market = segments.next().context("no signup market")?;
    ensure!(
        segments.next() == Some("signup") && segments.next().is_none(),
        "unrecognized Spotify signup redirect"
    );
    let region = market
        .split('-')
        .next()
        .and_then(country)
        .context("no signup market country")?;
    Ok(location(region, Note::SignupMarket))
}

/// Three anonymous device requests; the browser key is public, not user auth.
pub(super) async fn disney_plus(http: &Http) -> Result<Finding> {
    const KEY: &str = "ZGlzbmV5JmJyb3dzZXImMS4wLjA.Cu56AgSfBTDag5NiRA81oLHkDZfu5L3CKadnefEAY84";
    const API: &str = "https://disney.api.edge.bamgrid.com";
    let device = Http::send(
        http.follow
            .post(format!("{API}/devices"))
            .bearer_auth(KEY)
            .json(&json!({
                "deviceFamily":"browser", "applicationRuntime":"chrome", "deviceProfile":"windows", "attributes":{}
            })),
    )
    .await?;
    if disney_refused(&device) {
        return Ok(region_gate(false, None));
    }
    let assertion = string(&device.json()?, "assertion").context("no anonymous device assertion")?;
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
            .post(format!("{API}/token"))
            .bearer_auth(KEY)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form),
    )
    .await?;
    if disney_refused(&token) {
        return Ok(region_gate(false, None));
    }
    let refresh = string(&token.json()?, "refresh_token").context("no anonymous refresh token")?;
    let session = Http::send(http.follow.post(format!("{API}/graph/v1/device/graphql"))
        .header(header::AUTHORIZATION, KEY).json(&json!({
            "query":"mutation refreshToken($input: RefreshTokenInput!) { refreshToken(refreshToken: $input) { activeSession { sessionId } } }",
            "variables":{"input":{"refreshToken":refresh}}
        }))).await?;
    if disney_refused(&session) {
        return Ok(region_gate(false, None));
    }
    disney_finding(&session.json()?)
}

fn disney_refused(page: &Page) -> bool {
    // A generic 403 or failed device key is not a region restriction.
    serde_json::from_str::<Value>(&page.body).ok().is_some_and(|data| {
        data["errors"]
            .as_array()
            .is_some_and(|errors| errors.iter().any(|error| error["code"] == "forbidden-location"))
    })
}

fn disney_finding(data: &Value) -> Result<Finding> {
    ensure!(
        data["errors"].is_null() || data["errors"] == json!([]),
        "Disney+ returned session errors"
    );
    let session = &data["extensions"]["sdk"]["session"];
    let allowed = session["inSupportedLocation"]
        .as_bool()
        .context("no inSupportedLocation boolean")?;
    let region = string(&session["location"], "countryCode").as_deref().and_then(country);
    // No country exceptions or launch-list inference: use the service flag.
    Ok(region_gate(allowed, region))
}

pub(super) async fn dazn(http: &Http) -> Result<Finding> {
    let page = Http::send(
        http.follow
            .post("https://startup.core.indazn.com/misl/v5/Startup")
            .json(&json!({
                "LandingPageKey":"generic", "languages":"en-US,en", "Platform":"web",
                "PlatformAttributes":{}, "Manufacturer":"", "PromoCode":"", "Version":"2"
            })),
    )
    .await?;
    dazn_finding(&page.json()?)
}

fn dazn_finding(data: &Value) -> Result<Finding> {
    let region = &data["Region"];
    let allowed = region["isAllowed"]
        .as_bool()
        .context("DAZN returned no Region.isAllowed boolean")?;
    let country = string(region, "GeolocatedCountry").as_deref().and_then(country);
    Ok(region_gate(allowed, country))
}

/// HBO's anonymous session exposes currentLocationTerritory. Market links on
/// the homepage never establish support, and missing routing never means denial.
pub(super) async fn hbo_max(http: &Http) -> Result<Finding> {
    let headers = |request: RequestBuilder| {
        request
        .header("x-device-info", "beam/5.0.0 (desktop/desktop; Windows/10; afbb9b4d-5bd5-4a3e-a4ee-d5fc95e78c1c/da0cdd94-5a39-42ef-aa68-54cbc1b852c3)")
        .header("x-disco-client", "WEB:10:beam:5.2.1")
        .header("x-disco-params", "realm=bolt,bid=beam,features=ar")
    };
    let mut device = [0u8; 16];
    getrandom::fill(&mut device).context("generate an anonymous device id")?;
    let device: String = device.iter().map(|byte| format!("{byte:02x}")).collect();
    let token = Http::send(headers(http.get(&format!(
        "https://default.any-any.prd.api.hbomax.com/token?realm=bolt&deviceId={device}"
    ))))
    .await?;
    token.served()?;
    let cookies = token.cookie_header();
    ensure!(!cookies.is_empty(), "HBO Max returned no anonymous session cookies");
    let bootstrap = Http::send(
        headers(
            http.follow
                .post("https://default.any-any.prd.api.hbomax.com/session-context/headwaiter/v1/bootstrap"),
        )
        .header(header::COOKIE, &cookies),
    )
    .await?;
    let data = bootstrap.json()?;
    let routing = &data["routing"];
    let tenant = string(routing, "tenant").context("HBO Max returned no routing tenant")?;
    let market = string(routing, "homeMarket").context("HBO Max returned no routing market")?;
    ensure!(
        [&tenant, &market]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')),
        "unrecognized HBO Max routing"
    );
    let me = Http::send(
        headers(http.get(&format!(
            "https://default.{tenant}-{market}.prd.api.hbomax.com/users/me"
        )))
        .header(header::COOKIE, &cookies),
    )
    .await?;
    hbo_finding(&me.json()?)
}

fn hbo_finding(data: &Value) -> Result<Finding> {
    let region = string(&data["data"]["attributes"], "currentLocationTerritory")
        .as_deref()
        .and_then(country)
        .context("HBO Max returned no currentLocationTerritory")?;
    Ok(location(region, Note::LocationOnly))
}

pub(super) async fn abema(http: &Http) -> Result<Finding> {
    const URL: &str = "https://api.abema.io/v1/ip/check?device=android";
    let tcp = async { abema_finding(&http.page(URL).await?) };
    let quic = async {
        let relay = http.relay.as_ref()?;
        let attempts = (0..2).map(|_| {
            Box::pin(async {
                let request = http::Request::get(URL)
                    .header(header::USER_AGENT, USER_AGENT)
                    .body(Bytes::new())?;
                tokio::time::timeout(Duration::from_secs(4), http3::send(relay, request))
                    .await
                    .context("HTTP/3 timed out")?
            })
        });
        let (response, _) = futures_util::future::select_ok(attempts).await.ok()?;
        let body = match response.body {
            Ok(body) => body,
            Err(error) => return Some(Err(error)),
        };
        Some(abema_finding(&Page {
            status: response.status,
            url: Url::parse(URL).expect("constant URL"),
            body: String::from_utf8_lossy(&body).into_owned(),
            location: None,
            cookies: Vec::new(),
        }))
    };
    let (tcp, quic) = tokio::join!(tcp, quic);
    abema_protocols(tcp, quic)
}

fn abema_finding(page: &Page) -> Result<Finding> {
    let data: Value = serde_json::from_str(&page.body).context("Abema returned no IP-check JSON")?;
    let code = data["error"]
        .as_str()
        .or_else(|| data["error"]["code"].as_str())
        .or_else(|| data["code"].as_str());
    match code {
        Some("anonymous_ip") => return Ok(Finding::no(None, Some(Note::ProxyDetected))),
        Some("blocked_location") => return Ok(region_gate(false, None)),
        Some(_) => bail!("Abema returned an unrecognized IP-check error"),
        None => {}
    }
    page.served()?;
    ensure!(
        data["error"].is_null() && data["errors"].is_null(),
        "Abema returned IP-check errors"
    );
    let region = string(&data, "isoCountryCode")
        .as_deref()
        .and_then(country)
        .context("Abema returned no isoCountryCode")?;
    // A non-JP country is still a passed IP gate, not proof of any lineup.
    Ok(region_gate(true, Some(region)))
}

fn abema_protocols(tcp: Result<Finding>, quic: Option<Result<Finding>>) -> Result<Finding> {
    let (finding, protocol) = match quic {
        Some(quic) => {
            let quic = quic.context("Abema HTTP/3 response was inconclusive")?;
            match tcp {
                Ok(tcp) => {
                    ensure!(
                        tcp == quic,
                        "Abema IP checks disagree: TCP {:?}/{:?}; HTTP/3 {:?}/{:?}",
                        tcp.verdict,
                        tcp.region,
                        quic.verdict,
                        quic.region
                    );
                    (quic, "TCP + HTTP/3")
                }
                Err(_) => (quic, "HTTP/3"),
            }
        }
        None => (tcp?, "TCP"),
    };
    Ok(finding.detail(Some(protocol.into())))
}

pub(super) async fn youtube_premium(http: &Http) -> Result<Finding> {
    let page = Http::send(
        http.get("https://www.youtube.com/premium?hl=en")
            .header(header::COOKIE, GOOGLE_CONSENT),
    )
    .await?;
    youtube_finding(&page)
}

fn youtube_finding(page: &Page) -> Result<Finding> {
    page.served()?;
    ensure!(
        page.url.host_str() == Some("www.youtube.com") && page.url.path() == "/premium",
        "unrecognized YouTube Premium page"
    );
    let start = page
        .body
        .split_once("var ytInitialData = ")
        .map(|(_, rest)| rest)
        .context("YouTube returned no initial page data")?;
    let data = serde_json::Deserializer::from_str(start)
        .into_iter::<Value>()
        .next()
        .context("YouTube returned no initial page data")??;
    let contents = &data["contents"];
    let region = quoted_after(&page.body, "\"INNERTUBE_CONTEXT_GL\":\"").and_then(country);
    if contains_denial(contents) {
        return Ok(region_gate(false, region));
    }
    ensure!(contains_offer(contents), "YouTube returned no recognized Premium offer");
    Ok(Finding::of(Verdict::Yes, region, Some(Note::OfferOnly)))
}

fn contains_offer(data: &Value) -> bool {
    if let Some(offer) = data.get("lpOfferCardOptionViewModel") {
        return string(&offer["title"], "content").is_some()
            && offer["purchaseButtonOverride"]["buttonViewModel"]["onTap"].is_object();
    }
    match data {
        Value::Object(map) => map.values().any(contains_offer),
        Value::Array(list) => list.iter().any(contains_offer),
        _ => false,
    }
}

fn contains_denial(data: &Value) -> bool {
    match data {
        Value::String(text) => text.contains("Premium is not available in your country"),
        Value::Object(map) => map.values().any(contains_denial),
        Value::Array(list) => list.iter().any(contains_denial),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unlock::tests::page;

    #[test]
    fn netflix_and_prime_report_only_their_explicit_location_fields() {
        let netflix = |body: &str| page(StatusCode::OK, "https://www.netflix.com/jp-en/title/81280792", body);
        let finding = netflix_finding(&netflix(r#""requestCountry":{"supportedLocales":["en"],"id":"US","countryName":"United\x20States","status":"ALLOW"}"#)).unwrap();
        assert_eq!(finding.region.as_deref(), Some("US"));
        assert_eq!(finding.note, Some(Note::LocationOnly));
        for body in [
            r#"{"country":"JP"}"#,
            r#""requestCountry":{"id":"XXY"}"#,
            r#""requestCountry":{"nested":{"id":"JP"}}"#,
            r#""requestCountry":{"id":"JP""#,
            "<html>Sign in</html>",
        ] {
            assert!(netflix_finding(&netflix(body)).is_err());
        }
        let prime = |body: &str| page(StatusCode::OK, "https://www.primevideo.com/", body);
        let finding = prime_video_finding(&prime(r#""currentTerritory":"HK""#)).unwrap();
        assert_eq!(finding.region.as_deref(), Some("HK"));
        assert_eq!(finding.note, Some(Note::LocationOnly));
        assert!(prime_video_finding(&prime(r#"{"countryCode":"JP","description":"G (General Audience)"}"#)).is_err());
        assert!(prime_video_finding(&prime("<html>Welcome</html>")).is_err());
        assert_eq!(
            prime_video_finding(&prime(r#"{"isServiceRestricted":true}"#))
                .unwrap()
                .verdict,
            Verdict::No
        );
        for status in [StatusCode::FORBIDDEN, StatusCode::TOO_MANY_REQUESTS] {
            assert!(
                netflix_finding(&page(
                    status,
                    "https://www.netflix.com/",
                    r#""requestCountry":{"id":"JP"}"#
                ))
                .is_err()
            );
            assert!(
                prime_video_finding(&page(
                    status,
                    "https://www.primevideo.com/",
                    r#""currentTerritory":"JP""#
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn spotify_validates_the_signup_redirect_and_never_claims_registration() {
        let redirect = |status, target: &str| {
            let mut result = page(status, "https://www.spotify.com/signup", "");
            result.location = Some(target.into());
            result
        };
        let finding = spotify_finding(&redirect(StatusCode::FOUND, "https://www.spotify.com/jp/signup/")).unwrap();
        assert_eq!(finding.region.as_deref(), Some("JP"));
        assert_eq!(finding.note, Some(Note::SignupMarket));
        assert_eq!(
            spotify_finding(&redirect(StatusCode::FOUND, "/us-en/signup/"))
                .unwrap()
                .region
                .as_deref(),
            Some("US")
        );
        assert_eq!(
            spotify_finding(&redirect(StatusCode::FOUND, "/why-not-available/"))
                .unwrap()
                .verdict,
            Verdict::No
        );
        for target in [
            "https://example.org/us/signup/",
            "/us/login/",
            "/us/",
            "/us/signup/anything",
            "http://www.spotify.com/us/signup/",
        ] {
            assert!(spotify_finding(&redirect(StatusCode::FOUND, target)).is_err());
        }
        assert!(spotify_finding(&redirect(StatusCode::FORBIDDEN, "/us/signup/")).is_err());
        assert!(spotify_finding(&redirect(StatusCode::OK, "/us/signup/")).is_err());
        assert!(spotify_finding(&redirect(StatusCode::NOT_MODIFIED, "/us/signup/")).is_err());
    }

    #[test]
    fn disney_uses_the_anonymous_service_flag_without_country_exceptions() {
        for allowed in [true, false] {
            let data = json!({"extensions":{"sdk":{"session":{
                "inSupportedLocation":allowed,"isSubscriber":false,"account":null,"location":{"countryCode":"JP"}
            }}}});
            let finding = disney_finding(&data).unwrap();
            assert_eq!(finding.verdict, if allowed { Verdict::Yes } else { Verdict::No });
            assert_eq!(finding.region.as_deref(), Some("JP"));
            assert_eq!(
                finding.note,
                Some(if allowed {
                    Note::RegionCheckOnly
                } else {
                    Note::RegionUnsupported
                })
            );
        }
        for data in [
            json!({}),
            json!({"extensions":{"sdk":{"session":{"inSupportedLocation":"true"}}}}),
            json!({"errors":[{"code":"unauthorized"}],"extensions":{"sdk":{"session":{"inSupportedLocation":true}}}}),
        ] {
            assert!(disney_finding(&data).is_err());
        }
        let explicit = page(
            StatusCode::FORBIDDEN,
            "https://disney.api.edge.bamgrid.com/token",
            r#"{"errors":[{"code":"forbidden-location"}]}"#,
        );
        assert!(disney_refused(&explicit));
        for body in [
            "Access denied",
            r#"{"errors":[{"code":"invalid-token"}]}"#,
            r#"{"message":"forbidden-location"}"#,
        ] {
            assert!(!disney_refused(&page(
                StatusCode::FORBIDDEN,
                explicit.url.as_str(),
                body
            )));
        }
    }

    #[test]
    fn dazn_requires_a_boolean_and_hbo_only_reports_session_placement() {
        for allowed in [true, false] {
            let finding = dazn_finding(&json!({"Region":{"isAllowed":allowed,"GeolocatedCountry":"jp"}})).unwrap();
            assert_eq!(finding.verdict, if allowed { Verdict::Yes } else { Verdict::No });
            assert_eq!(finding.region.as_deref(), Some("JP"));
        }
        for data in [
            json!({}),
            json!({"Region":{"isAllowed":"true"}}),
            json!({"Region":{"GeolocatedCountry":"JP"}}),
        ] {
            assert!(dazn_finding(&data).is_err());
        }
        let hbo = hbo_finding(
            &json!({"data":{"attributes":{"anonymous":true,"currentLocationTerritory":"US","products":[]}}}),
        )
        .unwrap();
        assert_eq!(hbo.region.as_deref(), Some("US"));
        assert_eq!(hbo.note, Some(Note::LocationOnly));
        for data in [
            json!({}),
            json!({"routing":{"homeMarket":"amer"}}),
            json!({"errors":[{"code":"unauthorized"}]}),
        ] {
            assert!(hbo_finding(&data).is_err());
        }
    }

    #[test]
    fn abema_does_not_infer_catalog_access_from_country_or_arbitrary_text() {
        let response = |status, body: &str| page(status, "https://api.abema.io/v1/ip/check?device=android", body);
        for region in ["JP", "US"] {
            let finding = abema_finding(&response(
                StatusCode::OK,
                &json!({"isoCountryCode":region,"timeZone":"Asia/Tokyo"}).to_string(),
            ))
            .unwrap();
            assert_eq!(finding.verdict, Verdict::Yes);
            assert_eq!(finding.note, Some(Note::RegionCheckOnly));
            assert_eq!(finding.region.as_deref(), Some(region));
        }
        for (code, note) in [
            ("anonymous_ip", Note::ProxyDetected),
            ("blocked_location", Note::RegionUnsupported),
        ] {
            let finding = abema_finding(&response(StatusCode::FORBIDDEN, &json!({"error":code}).to_string())).unwrap();
            assert_eq!(finding.verdict, Verdict::No);
            assert_eq!(finding.note, Some(note));
        }
        for (status, body) in [
            (StatusCode::OK, r#"{"message":"anonymous_ip"}"#),
            (StatusCode::FORBIDDEN, r#"{"isoCountryCode":"JP"}"#),
            (StatusCode::OK, r#"{"error":"unknown","isoCountryCode":"JP"}"#),
            (StatusCode::OK, "<html>blocked_location</html>"),
        ] {
            assert!(abema_finding(&response(status, body)).is_err());
        }
        let yes = region_gate(true, Some("JP".into()));
        assert!(abema_protocols(Ok(yes.clone()), Some(Ok(region_gate(false, None)))).is_err());
        assert!(abema_protocols(Ok(yes.clone()), Some(Ok(region_gate(true, Some("US".into()))))).is_err());
        assert!(abema_protocols(Ok(yes.clone()), Some(Err(anyhow::anyhow!("HTTP 403")))).is_err());
        assert_eq!(
            abema_protocols(Ok(yes.clone()), None).unwrap().detail.as_deref(),
            Some("TCP")
        );
        assert_eq!(
            abema_protocols(Ok(yes.clone()), Some(Ok(yes)))
                .unwrap()
                .detail
                .as_deref(),
            Some("TCP + HTTP/3")
        );
    }

    #[test]
    fn youtube_requires_a_rendered_offer_and_never_uses_a_country_allowlist() {
        let response = |contents: Value| {
            page(
                StatusCode::OK,
                "https://www.youtube.com/premium?hl=en",
                &format!(
                    "\"INNERTUBE_CONTEXT_GL\":\"CN\"; var ytInitialData = {};",
                    json!({"contents":contents})
                ),
            )
        };
        let offer = json!({"lpOfferCardOptionViewModel":{"title":{"content":"$15.99/month"},"purchaseButtonOverride":{"buttonViewModel":{"onTap":{"innertubeCommand":{}}}}}});
        let finding = youtube_finding(&response(json!({"items":[offer]}))).unwrap();
        assert_eq!(finding.verdict, Verdict::Yes);
        assert_eq!(finding.note, Some(Note::OfferOnly));
        assert_eq!(finding.region.as_deref(), Some("CN"));
        assert_eq!(
            youtube_finding(&response(
                json!({"message":{"simpleText":"Premium is not available in your country"}})
            ))
            .unwrap()
            .verdict,
            Verdict::No
        );
        for contents in [
            json!({}),
            json!({"premiumPurchaseButtonRenderer":{"flowStateEntityKey":"example"}}),
            json!({"lpOfferCardOptionViewModel":{"title":{"content":"$15.99/month"}}}),
        ] {
            assert!(youtube_finding(&response(contents)).is_err());
        }
        assert!(
            youtube_finding(&page(
                StatusCode::OK,
                "https://www.youtube.com/premium",
                "<html>Premium ad-free</html>"
            ))
            .is_err()
        );
        let mut denied = response(json!({"message":"Premium is not available in your country"}));
        denied.status = StatusCode::FORBIDDEN;
        assert!(youtube_finding(&denied).is_err());
    }

    #[test]
    fn anonymous_session_cookies_keep_last_value_without_attributes() {
        let mut response = page(StatusCode::OK, "https://example.com/", "");
        response.cookies = vec![
            "session=first; Secure; HttpOnly".into(),
            "other=value; Path=/".into(),
            "session=last; Path=/".into(),
        ];
        assert_eq!(response.cookie_header(), "other=value; session=last");
    }
}
