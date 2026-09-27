use anyhow::Result;
use headless_core::config::{
    PrfOption,
    remote::{from_response, subscription_url, validate_yaml},
};

#[test]
fn upstream_dirty_query_repair_and_http_url_restrictions() -> Result<()> {
    let url = subscription_url("https://example.test/sub&token=a%2Bb&key=value#fragment")?;
    assert_eq!(url.path(), "/sub");
    assert_eq!(
        url.query_pairs().collect::<Vec<_>>(),
        [("token".into(), "a+b".into()), ("key".into(), "value".into())]
    );
    assert!(url.fragment().is_none());
    assert_eq!(
        subscription_url("https://example.test/sub&literal?token=ok")?.path(),
        "/sub&literal"
    );
    for input in ["file:///tmp/sub", "ftp://example.test/sub", "not a url", "http://"] {
        let error = subscription_url(input).unwrap_err();
        assert!(!error.to_string().contains(input));
    }
    Ok(())
}
#[test]
fn upstream_headers_preserve_usage_filename_home_and_interval_precedence() -> Result<()> {
    let url = subscription_url("https://example.test/fallback.yaml")?;
    let headers = vec![
        (
            "x-amz-meta-subscription-userinfo".into(),
            "upload=1; download=2; total=3; expire=4".into(),
        ),
        (
            "Content-Disposition".into(),
            "attachment; filename=plain.yaml; filename*=UTF-8''%E8%BF%9C%E7%A8%8B.yaml".into(),
        ),
        ("profile-update-interval".into(), "6".into()),
        ("profile-web-page-url".into(), " https://example.test/account ".into()),
    ];
    let profile = from_response(
        &url,
        None,
        &headers,
        "\u{feff}# raw\nproxies: []\n",
        PrfOption::default(),
    )?;
    assert_eq!(profile.name, "远程.yaml");
    assert_eq!(profile.yaml, "# raw\nproxies: []\n");
    let extra = profile.extra.unwrap();
    assert_eq!((extra.upload, extra.download, extra.total, extra.expire), (1, 2, 3, 4));
    assert_eq!(profile.home.as_deref(), Some("https://example.test/account"));
    assert_eq!(profile.option.update_interval, Some(360));
    assert_eq!(profile.option.allow_auto_update, Some(true));
    assert!(profile.option.merge.is_none());
    let override_profile = from_response(
        &url,
        Some("explicit"),
        &headers,
        "proxy-providers: {}",
        PrfOption {
            update_interval: Some(15),
            allow_auto_update: Some(false),
            ..Default::default()
        },
    )?;
    assert_eq!(override_profile.name, "explicit");
    assert_eq!(override_profile.option.update_interval, Some(15));
    assert_eq!(override_profile.option.allow_auto_update, Some(false));
    let headers = vec![
        (
            "Content-Disposition".into(),
            "attachment; filename=\"plain.yaml\"".into(),
        ),
        ("profile-web-page-url".into(), "javascript:alert(1)".into()),
        ("profile-update-interval".into(), u64::MAX.to_string()),
        ("notsubscription-userinfo".into(), "total=100".into()),
    ];
    let profile = from_response(&url, None, &headers, "proxies: []", PrfOption::default())?;
    assert_eq!(profile.name, "plain.yaml");
    assert!(profile.home.is_none() && profile.option.update_interval.is_none() && profile.extra.is_none());
    assert_eq!(
        from_response(&url, None, &[], "proxies: []", PrfOption::default())?.name,
        "fallback.yaml"
    );
    Ok(())
}
#[test]
fn remote_yaml_requires_a_mapping_with_upstream_subscription_keys() -> Result<()> {
    for yaml in ["mode: rule", "- sequence", "proxies: [", "<html>error</html>"] {
        assert!(validate_yaml(yaml).is_err());
    }
    assert!(validate_yaml("proxies: null").is_ok());
    assert!(validate_yaml("proxy-providers: {}").is_ok());
    let url = subscription_url("https://example.test/")?;
    assert_eq!(
        from_response(&url, None, &[], "proxies: []", PrfOption::default())?.name,
        "Remote File"
    );
    assert!(from_response(&url, Some(&"x".repeat(257)), &[], "proxies: []", PrfOption::default()).is_err());
    Ok(())
}
