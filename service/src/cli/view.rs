//! Pure selection and formatting rules for the command line; no I/O.
use anyhow::{Result, bail};
pub use management_client::view::{Group, Subscription, default_group, groups, subscriptions};
use serde_json::{Map, Value};

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

pub fn age(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

/// How long until THEN, the counterpart of [`age`].
pub fn remaining(now: u64, then: u64) -> String {
    let seconds = then.saturating_sub(now);
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("in {} min", seconds / 60),
        3600..86_400 => format!("in {} h", seconds / 3600),
        _ => format!("in {} d", seconds / 86_400),
    }
}

/// `never`/`off`/`0` (no automatic rotation) or 1 to 3650 days.
pub fn parse_rotation_period(value: &str) -> Result<Option<u32>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "never" | "off" | "0" => Ok(None),
        days => match days.trim_end_matches('d').parse::<u32>() {
            Ok(days @ 1..=3650) => Ok(Some(days)),
            _ => bail!("rotation period must be 1 to 3650 days, or never"),
        },
    }
}

/// Label/value lines describing a `token_rotation` answer.
pub fn token_rotation(rotation: &Value, now: u64) -> Vec<(&'static str, String)> {
    let number = |name| rotation.get(name).and_then(Value::as_u64);
    let mut lines = Vec::new();
    if let Some(issued) = number("issued_at") {
        lines.push(("issued", format!("{} UTC ({})", date(issued), age(now, issued))));
    }
    match (number("period_days"), number("expires_at")) {
        (Some(days), Some(expires)) => {
            lines.push(("rotation", format!("every {days} d")));
            let soon = number("remind_at").is_some_and(|remind| now >= remind);
            lines.push((
                "rotates",
                format!(
                    "{} UTC ({}){}",
                    date(expires),
                    remaining(now, expires),
                    if soon { "; soon" } else { "" }
                ),
            ));
        }
        _ => lines.push(("rotation", "never (valid until rotated by hand)".into())),
    }
    lines
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

    #[test]
    fn token_rotation_periods_and_schedule_read_naturally() {
        for (input, expected) in [
            ("30", Some(30)),
            ("7d", Some(7)),
            ("3650", Some(3650)),
            ("never", None),
            ("OFF", None),
            ("0", None),
        ] {
            assert_eq!(parse_rotation_period(input).unwrap(), expected, "{input}");
        }
        for invalid in ["3651", "-1", "0d", "monthly", ""] {
            assert!(parse_rotation_period(invalid).is_err(), "{invalid}");
        }
        let day = 86_400;
        let rotation =
            serde_json::json!({"period_days": 30, "issued_at": 0, "expires_at": 30 * day, "remind_at": 23 * day});
        assert_eq!(
            token_rotation(&rotation, day),
            vec![
                ("issued", "1970-01-01 UTC (1 d ago)".to_owned()),
                ("rotation", "every 30 d".to_owned()),
                ("rotates", "1970-01-31 UTC (in 29 d)".to_owned()),
            ]
        );
        assert!(token_rotation(&rotation, 25 * day)[2].1.ends_with("; soon"));
        let never = serde_json::json!({"period_days": null, "issued_at": 0, "expires_at": null, "remind_at": null});
        assert_eq!(token_rotation(&never, 0)[1].1, "never (valid until rotated by hand)");
    }
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
