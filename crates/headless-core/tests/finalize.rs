use headless_core::{config::runtime::parse, enhance::finalize::finalize};
use serde_yaml_ng::{Mapping, Value};

#[test]
fn lan_widens_only_enabled_loopback_bindings() {
    for address in [
        "localhost",
        " LOCALHOST ",
        "127.0.0.1",
        "127.0.0.2",
        "127.1",
        "127.65535",
        "127.1.2",
        "::1",
        "[::1]",
        "0:0:0:0:0:0:0:1",
    ] {
        let input = parse(&format!("allow-lan: true\nbind-address: '{address}'")).unwrap();
        assert_eq!(finalize(input)["bind-address"].as_str(), Some("*"), "{address}");
    }
    for address in [
        "192.168.1.2",
        "*",
        "::",
        "0.0.0.0",
        "127",
        "127.16777216",
        "127.256.1",
        "127.1.256.1",
        "[::2]",
        "localhost:7890",
        "",
    ] {
        let input = parse(&format!("allow-lan: true\nbind-address: '{address}'")).unwrap();
        assert_eq!(finalize(input)["bind-address"].as_str(), Some(address), "{address}");
    }
    for allow in ["false", "null", "'true'"] {
        let input = parse(&format!("allow-lan: {allow}\nbind-address: '127.0.0.1'")).unwrap();
        assert_eq!(finalize(input)["bind-address"].as_str(), Some("127.0.0.1"));
    }
    assert!(!finalize(parse("allow-lan: true").unwrap()).contains_key("bind-address"));
    assert_eq!(
        finalize(parse("allow-lan: true\nbind-address: 42").unwrap())["bind-address"],
        Value::from(42)
    );
}

#[test]
fn group_cleanup_preserves_builtins_nested_groups_and_order() {
    let input = parse(
        r#"
proxies:
  - {name: alive, type: ss}
  - string-node
proxy-providers: {known: {type: file}}
proxy-groups:
  - name: manual
    type: select
    proxies: [alive, missing, string-node, nested, known, DIRECT, REJECT, REJECT-DROP, PASS, PASS-RULE, DIRECT, 42]
    use: [missing-provider, 42]
  - name: nested
    type: select
    proxies: [manual, missing]
rules: ['MATCH,manual']
custom: kept
"#,
    )
    .unwrap();
    let result = finalize(input);
    assert_eq!(
        result["proxy-groups"][0]["proxies"],
        serde_yaml_ng::from_str::<Value>(
            "[alive, string-node, nested, known, DIRECT, REJECT, REJECT-DROP, PASS, PASS-RULE, DIRECT, 42]"
        )
        .unwrap()
    );
    assert_eq!(result["proxy-groups"][0]["use"], Value::Sequence(vec![]));
    assert_eq!(
        result["proxy-groups"][1]["proxies"],
        serde_yaml_ng::from_str::<Value>("[manual]").unwrap()
    );
    assert_eq!(result["custom"].as_str(), Some("kept"));
    assert_eq!(result["rules"][0].as_str(), Some("MATCH,manual"));
}

#[test]
fn provider_backed_groups_keep_dynamic_names_only_with_a_valid_use() {
    let result = finalize(
        parse(
            r#"
proxy-providers: {providerA: {type: file}}
proxy-groups:
  - {name: dynamic, type: select, use: [providerA, ghost, 0], proxies: [dynamic-node, DIRECT]}
  - {name: missing, type: select, use: [ghost], proxies: [dynamic-node, DIRECT]}
  - {name: no-use, type: select, proxies: [dynamic-node, DIRECT]}
"#,
        )
        .unwrap(),
    );
    assert_eq!(
        result["proxy-groups"][0]["use"],
        serde_yaml_ng::from_str::<Value>("[providerA]").unwrap()
    );
    assert_eq!(
        result["proxy-groups"][0]["proxies"],
        serde_yaml_ng::from_str::<Value>("[dynamic-node, DIRECT]").unwrap()
    );
    for i in [1, 2] {
        assert_eq!(
            result["proxy-groups"][i]["proxies"],
            serde_yaml_ng::from_str::<Value>("[DIRECT]").unwrap()
        );
    }
}

#[test]
fn malformed_nested_shapes_are_left_for_core_validation() {
    for yaml in [
        "proxy-groups: invalid",
        "proxy-groups: [invalid, {name: group, use: invalid, proxies: invalid}]",
        "proxies: invalid\nproxy-providers: invalid\nproxy-groups: [{name: group, proxies: [DIRECT, {unexpected: true}]}]",
        "proxy-groups: [{name: empty, proxies: [missing]}]\nrules: ['MATCH,empty']",
    ] {
        let input = parse(yaml).unwrap();
        let result = finalize(input.clone());
        if yaml.contains("name: empty") {
            assert_eq!(result["proxy-groups"][0]["proxies"], Value::Sequence(vec![]));
            assert_eq!(result["rules"], input["rules"]);
        } else {
            assert_eq!(result, input);
        }
    }
}

#[test]
fn final_order_matches_upstream_and_is_idempotent() {
    let input = parse("rules: []\ncustom: value\nproxy-groups: []\nallow-lan: true\nbind-address: localhost\nmode: direct\nproxies: []\nproxy-providers: {}\nrule-providers: {}\ndns: {enable: false}").unwrap();
    let result = finalize(input.clone());
    assert_eq!(result.len(), input.len());
    assert_eq!(
        result.keys().filter_map(Value::as_str).collect::<Vec<_>>(),
        [
            "mode",
            "allow-lan",
            "custom",
            "bind-address",
            "dns",
            "proxies",
            "proxy-providers",
            "proxy-groups",
            "rule-providers",
            "rules"
        ]
    );
    let again: Mapping = finalize(result.clone());
    assert_eq!(
        serde_yaml_ng::to_string(&result).unwrap(),
        serde_yaml_ng::to_string(&again).unwrap()
    );
}
