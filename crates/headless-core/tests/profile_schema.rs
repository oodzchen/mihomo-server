use headless_core::config::IProfiles;
use serde_yaml_ng::{Mapping, Value};

#[test]
fn upstream_profile_yaml_preserves_subscription_and_selection_fields() -> anyhow::Result<()> {
    let yaml = r"
current: remote-one
items:
  - uid: remote-one
    type: remote
    name: Example
    file: remote-one.yaml
    desc: subscription
    url: https://example.test/subscription
    selected:
      - name: Main
        now: node-one
    extra:
      upload: 1
      download: 2
      total: 3
      expire: 4
    updated: 1234
    option:
      user_agent: mihomo-server
      with_proxy: true
      self_proxy: false
      update_interval: 60
      timeout_seconds: 30
      danger_accept_invalid_certs: false
      allow_auto_update: true
      merge: merge-one
      script: script-one
      rules: rules-one
      proxies: proxies-one
      groups: groups-one
    home: https://example.test/
";
    let original: Mapping = serde_yaml_ng::from_str(yaml)?;
    let mut profiles: IProfiles = serde_yaml_ng::from_str(yaml)?;
    let items = profiles
        .items
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("missing items"))?;
    let item = items.first_mut().ok_or_else(|| anyhow::anyhow!("missing profile"))?;
    assert_eq!(item.itype.as_deref(), Some("remote"));
    item.file_data = Some("transient uploaded content".into());
    let saved = serde_yaml_ng::to_value(&profiles)?;
    assert_eq!(saved, Value::Mapping(original));

    let empty: IProfiles = serde_yaml_ng::from_str("items: [{type: local}]")?;
    let saved = serde_yaml_ng::to_value(&empty)?;
    let item = &saved["items"][0];
    let item = item
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("profile is not a mapping"))?;
    for field in ["desc", "url", "selected", "extra", "option", "home", "file_data"] {
        assert!(!item.contains_key(field), "unexpected serialized field: {field}");
    }
    assert!(item.contains_key("file"));
    assert!(item["file"].is_null());
    assert_eq!(item["type"], Value::from("local"));
    Ok(())
}
