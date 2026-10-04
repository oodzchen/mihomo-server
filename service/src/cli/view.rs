//! Pure selection and formatting rules for the command line; no I/O.
use anyhow::{Result, bail};
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

/// Last measured delay in milliseconds; `Some(0)` means the test failed.
pub fn last_delay(proxies: &Map<String, Value>, name: &str) -> Option<u64> {
    proxies
        .get(name)?
        .get("history")?
        .as_array()?
        .last()?
        .get("delay")?
        .as_u64()
}

pub fn delay_label(delay: Option<u64>) -> String {
    match delay {
        None => "-".into(),
        Some(0) => "timeout".into(),
        Some(delay) if delay >= 10_000 => "timeout".into(),
        Some(delay) => format!("{delay} ms"),
    }
}

/// Resolve an operand against labelled items: exact label, 1-based index,
/// case-insensitive label, then a unique case-insensitive substring.
pub fn pick<'a, T>(items: &'a [T], query: &str, what: &str, labels: impl Fn(&T) -> Vec<&str>) -> Result<&'a T> {
    if let Some(item) = items.iter().find(|item| labels(item).contains(&query)) {
        return Ok(item);
    }
    if let Ok(index) = query.parse::<usize>()
        && (1..=items.len()).contains(&index)
    {
        return Ok(&items[index - 1]);
    }
    let lowered = query.to_lowercase();
    if let Some(item) = items
        .iter()
        .find(|item| labels(item).iter().any(|label| label.to_lowercase() == lowered))
    {
        return Ok(item);
    }
    let matches: Vec<&T> = items
        .iter()
        .filter(|item| labels(item).iter().any(|label| label.to_lowercase().contains(&lowered)))
        .collect();
    match matches.as_slice() {
        [item] => Ok(item),
        [] => bail!("no {what} matches '{query}'"),
        several => {
            let names: Vec<&str> = several.iter().take(8).map(|item| labels(item)[0]).collect();
            let more = if several.len() > names.len() { ", ..." } else { "" };
            bail!(
                "'{query}' matches {} {what}s: {}{more}; be more specific",
                several.len(),
                names.join(", ")
            )
        }
    }
}

pub fn pick_name<'a>(names: &'a [String], query: &str, what: &str) -> Result<&'a str> {
    pick(names, query, what, |name| vec![name.as_str()]).map(String::as_str)
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

pub fn age(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

/// UTC calendar date of a Unix timestamp (Howard Hinnant's civil_from_days).
pub fn date(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit + 1 < UNITS.len() {
        amount /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{amount:.1} {}", UNITS[unit])
    }
}

pub fn parse_mode(value: &str) -> Result<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "rule" => Ok("rule"),
        "global" => Ok("global"),
        "direct" => Ok("direct"),
        _ => bail!("unknown mode '{value}'; use rule, global or direct"),
    }
}

pub fn parse_switch(value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "enable" | "true" | "1" => Ok(true),
        "off" | "disable" | "false" | "0" => Ok(false),
        _ => bail!("expected on or off, not '{value}'"),
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
    fn delays_come_from_the_latest_history_entry() {
        let proxies = proxies();
        assert_eq!(delay_label(last_delay(&proxies, "日本 01")), "35 ms");
        assert_eq!(delay_label(last_delay(&proxies, "日本 02")), "timeout");
        assert_eq!(delay_label(last_delay(&proxies, "美国 07")), "-");
        assert_eq!(delay_label(Some(12_000)), "timeout");
    }

    #[test]
    fn names_resolve_exactly_by_index_case_and_unique_substring() {
        let names: Vec<String> = ["日本 01", "日本 02", "美国 07", "HK-1"].map(String::from).into();
        assert_eq!(pick_name(&names, "日本 02", "node").unwrap(), "日本 02");
        assert_eq!(pick_name(&names, "3", "node").unwrap(), "美国 07");
        assert_eq!(pick_name(&names, "hk-1", "node").unwrap(), "HK-1");
        assert_eq!(pick_name(&names, "美国", "node").unwrap(), "美国 07");
        let ambiguous = pick_name(&names, "日本", "node").unwrap_err().to_string();
        assert!(ambiguous.contains("matches 2 nodes: 日本 01, 日本 02"), "{ambiguous}");
        assert!(
            pick_name(&names, "9", "node")
                .unwrap_err()
                .to_string()
                .contains("no node matches")
        );
        // An exact numeric name wins over an index.
        let numeric: Vec<String> = ["2", "x"].map(String::from).into();
        assert_eq!(pick_name(&numeric, "2", "node").unwrap(), "2");
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
    fn formatting_helpers() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_795_305_600), "2026-11-22");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(age(100, 100), "just now");
        assert_eq!(age(7300, 100), "2 h ago");
        assert_eq!(age(100, 200), "just now");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(27_166_183_833), "25.3 GiB");
        assert_eq!(parse_mode("GLOBAL").unwrap(), "global");
        assert!(parse_mode("tun").is_err());
        assert!(parse_switch("on").unwrap());
        assert!(!parse_switch("OFF").unwrap());
        assert!(parse_switch("maybe").is_err());
    }
}
