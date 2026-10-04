//! Pure readings of management responses; no I/O.
use serde_json::{Map, Value};

const GROUP_TYPES: [&str; 5] = ["Selector", "URLTest", "Fallback", "LoadBalance", "Relay"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub kind: String,
    pub now: Option<String>,
    /// URLTest/Fallback groups pinned by a manual selection.
    pub fixed: Option<String>,
    pub all: Vec<String>,
}

fn group(name: &str, value: &Value) -> Option<Group> {
    let kind = value.get("type")?.as_str()?;
    GROUP_TYPES.contains(&kind).then(|| Group {
        name: name.to_owned(),
        kind: kind.to_owned(),
        now: text(value, "now"),
        fixed: text(value, "fixed"),
        all: value
            .get("all")
            .and_then(Value::as_array)
            .map(|all| all.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default(),
    })
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Groups in configuration order (Mihomo lists them in GLOBAL), GLOBAL last.
pub fn groups(proxies: &Map<String, Value>) -> Vec<Group> {
    let mut ordered: Vec<Group> = Vec::new();
    let global_order = proxies
        .get("GLOBAL")
        .and_then(|global| global.get("all"))
        .and_then(Value::as_array)
        .map(|all| all.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    for name in global_order {
        if let Some(found) = proxies.get(name).and_then(|value| group(name, value))
            && !ordered.iter().any(|known| known.name == name)
        {
            ordered.push(found);
        }
    }
    let mut rest: Vec<Group> = proxies
        .iter()
        .filter(|(name, _)| *name != "GLOBAL" && !ordered.iter().any(|known| &known.name == *name))
        .filter_map(|(name, value)| group(name, value))
        .collect();
    rest.sort_by(|a, b| a.name.cmp(&b.name));
    ordered.extend(rest);
    if let Some(global) = proxies.get("GLOBAL").and_then(|value| group("GLOBAL", value)) {
        ordered.push(global);
    }
    ordered
}

/// The group a bare `proxy select NODE` changes: GLOBAL in global mode,
/// otherwise the conventional main group, otherwise the first selector.
pub fn default_group<'a>(groups: &'a [Group], mode: &str) -> Option<&'a Group> {
    if mode.eq_ignore_ascii_case("global")
        && let Some(global) = groups.iter().find(|group| group.name == "GLOBAL")
    {
        return Some(global);
    }
    groups
        .iter()
        .find(|group| matches!(group.name.to_lowercase().as_str(), "proxies" | "proxy"))
        .or_else(|| {
            groups
                .iter()
                .find(|group| group.kind == "Selector" && group.name != "GLOBAL")
        })
        .or_else(|| groups.iter().find(|group| group.name != "GLOBAL"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub uid: String,
    pub name: String,
    pub kind: String,
    pub url: Option<String>,
    pub updated: Option<u64>,
    /// upload + download, total and expiry (seconds) reported by the provider.
    pub usage: Option<(u64, u64, u64)>,
    pub current: bool,
}

/// Local and remote subscriptions; enhancement items (merge/script) are omitted.
pub fn subscriptions(profiles: &Value) -> Vec<Subscription> {
    let current = profiles.get("current").and_then(Value::as_str);
    profiles
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let uid = text(item, "uid")?;
            let kind = text(item, "type")?;
            if kind != "local" && kind != "remote" {
                return None;
            }
            let usage = item.get("extra").and_then(|extra| {
                let field = |key| extra.get(key).and_then(Value::as_u64).unwrap_or(0);
                Some((field("upload") + field("download"), field("total"), field("expire")))
                    .filter(|(used, total, expire)| *used + *total + *expire > 0)
            });
            Some(Subscription {
                name: text(item, "name").unwrap_or_else(|| uid.clone()),
                current: current == Some(uid.as_str()),
                uid,
                kind,
                url: text(item, "url"),
                updated: item.get("updated").and_then(Value::as_u64),
                usage,
            })
        })
        .collect()
}

/// The proxy mode the core reports, else the configured one (`proxy_access`).
pub fn current_mode(access: &Value) -> String {
    access
        .pointer("/reported/mode")
        .or_else(|| access.pointer("/configured/mode"))
        .and_then(Value::as_str)
        .unwrap_or("rule")
        .to_lowercase()
}

/// TUN facts from `proxy_access` and `multi_user`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TunState {
    /// The saved setting.
    pub enabled: bool,
    /// Another account holds the host's one system-wide TUN.
    pub held_by: Option<String>,
    /// `Some(false)`: this account may not create a TUN interface.
    pub capable: Option<bool>,
    pub device: Option<String>,
    pub system: bool,
}

pub fn tun_state(access: &Value, user: &Value) -> TunState {
    let held_by = access
        .get("tun_holder")
        .filter(|holder| !holder.is_null() && holder.get("self").and_then(Value::as_bool) != Some(true))
        .map(|holder| {
            holder
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("another user")
                .to_owned()
        });
    TunState {
        enabled: access.get("tun_enabled").and_then(Value::as_bool).unwrap_or(false),
        held_by,
        capable: user.get("tun_capable").and_then(Value::as_bool),
        device: text(user, "tun_device"),
        system: user.get("tun_system").and_then(Value::as_bool) == Some(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn proxies() -> Map<String, Value> {
        json!({
            "GLOBAL": {"type": "Selector", "now": "DIRECT", "all": ["DIRECT", "Proxies", "AI", "日本 01", "Auto"]},
            "AI": {"type": "Selector", "now": "日本 01", "all": ["Proxies", "日本 01", "日本 02"]},
            "Proxies": {"type": "Selector", "now": "日本 02", "all": ["日本 01", "日本 02", "美国 07"]},
            "Auto": {"type": "URLTest", "now": "日本 01", "fixed": "日本 02", "all": ["日本 01", "日本 02"]},
            "Hidden": {"type": "Fallback", "all": []},
            "日本 01": {"type": "AnyTLS", "history": [{"delay": 80}, {"delay": 35}]},
            "日本 02": {"type": "AnyTLS", "history": [{"delay": 0}]},
            "美国 07": {"type": "AnyTLS", "history": []},
            "DIRECT": {"type": "Direct"},
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn groups_follow_configuration_order_with_global_last() {
        let names: Vec<_> = groups(&proxies()).into_iter().map(|group| group.name).collect();
        assert_eq!(names, ["Proxies", "AI", "Auto", "Hidden", "GLOBAL"]);
        let auto = groups(&proxies())
            .into_iter()
            .find(|group| group.name == "Auto")
            .unwrap();
        assert_eq!(auto.fixed.as_deref(), Some("日本 02"));
    }

    #[test]
    fn default_group_depends_on_mode() {
        let groups = groups(&proxies());
        assert_eq!(default_group(&groups, "rule").unwrap().name, "Proxies");
        assert_eq!(default_group(&groups, "Global").unwrap().name, "GLOBAL");
        let without_main: Vec<_> = groups.into_iter().filter(|group| group.name != "Proxies").collect();
        assert_eq!(default_group(&without_main, "rule").unwrap().name, "AI");
    }

    #[test]
    fn subscriptions_hide_enhancements_and_mark_the_current_one() {
        let profiles = json!({
            "current": "R1",
            "items": [
                {"uid": "Merge", "type": "merge"},
                {"uid": "L1", "type": "local", "name": "amy", "updated": 10},
                {"uid": "R1", "type": "remote", "name": null, "url": "https://example/sub",
                 "extra": {"upload": 1, "download": 2, "total": 10, "expire": 0}},
            ]
        });
        let items = subscriptions(&profiles);
        assert_eq!(items.len(), 2);
        assert_eq!(
            (items[0].name.as_str(), items[0].current, items[0].usage),
            ("amy", false, None)
        );
        assert_eq!((items[1].name.as_str(), items[1].current), ("R1", true));
        assert_eq!(items[1].usage, Some((3, 10, 0)));
        assert_eq!(items[1].url.as_deref(), Some("https://example/sub"));
    }

    #[test]
    fn mode_and_tun_state_come_from_access_and_user() {
        assert_eq!(
            current_mode(&json!({"reported": {"mode": "Global"}, "configured": {"mode": "rule"}})),
            "global"
        );
        assert_eq!(
            current_mode(&json!({"reported": null, "configured": {"mode": "direct"}})),
            "direct"
        );
        assert_eq!(current_mode(&json!({})), "rule");
        let user = json!({"tun_device": "ms1000", "tun_capable": true, "tun_system": true});
        let mine = tun_state(
            &json!({"tun_enabled": true, "tun_holder": {"name": "me", "self": true}}),
            &user,
        );
        assert_eq!(
            mine,
            TunState {
                enabled: true,
                held_by: None,
                capable: Some(true),
                device: Some("ms1000".into()),
                system: true
            }
        );
        let held = tun_state(&json!({"tun_holder": {"name": "alice", "self": false}}), &Value::Null);
        assert_eq!(
            (held.enabled, held.held_by.as_deref(), held.capable),
            (false, Some("alice"), None)
        );
        assert_eq!(tun_state(&json!({"tun_holder": null}), &Value::Null).held_by, None);
    }
}
