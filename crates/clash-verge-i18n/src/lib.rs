use parking_lot::RwLock;
use std::{borrow::Cow, collections::HashMap, sync::LazyLock};

pub const DEFAULT_LANGUAGE: &str = "zh";

pub static SUPPORTED_LOCALES: &[&str] = &[
    "ar", "de", "en", "es", "fa", "id", "jp", "ko", "ru", "tr", "tt", "zh", "zhtw",
];

static RAW_LOCALES: &[(&str, &str)] = &[
    ("ar", include_str!("../locales/ar.yml")),
    ("de", include_str!("../locales/de.yml")),
    ("en", include_str!("../locales/en.yml")),
    ("es", include_str!("../locales/es.yml")),
    ("fa", include_str!("../locales/fa.yml")),
    ("id", include_str!("../locales/id.yml")),
    ("jp", include_str!("../locales/jp.yml")),
    ("ko", include_str!("../locales/ko.yml")),
    ("ru", include_str!("../locales/ru.yml")),
    ("tr", include_str!("../locales/tr.yml")),
    ("tt", include_str!("../locales/tt.yml")),
    ("zh", include_str!("../locales/zh.yml")),
    ("zhtw", include_str!("../locales/zhtw.yml")),
];

static LOCALES_DATA: LazyLock<HashMap<&'static str, HashMap<String, String>>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for &(locale, content) in RAW_LOCALES {
        let mut locale_map = HashMap::new();
        let value = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(content)
            .unwrap_or_else(|error| panic!("invalid embedded {locale} locale: {error}"));
        flatten_value("", &value, &mut locale_map);
        map.insert(locale, locale_map);
    }
    map
});

fn flatten_value(prefix: &str, value: &serde_yaml_ng::Value, map: &mut HashMap<String, String>) {
    match value {
        serde_yaml_ng::Value::Mapping(m) => {
            for (k, v) in m {
                let key_str = match k {
                    serde_yaml_ng::Value::String(s) => s.clone(),
                    serde_yaml_ng::Value::Number(n) => n.to_string(),
                    serde_yaml_ng::Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                let new_prefix = if prefix.is_empty() {
                    key_str
                } else {
                    format!("{prefix}.{key_str}")
                };
                flatten_value(&new_prefix, v, map);
            }
        }
        serde_yaml_ng::Value::String(s) => {
            map.insert(prefix.to_string(), s.clone());
        }
        serde_yaml_ng::Value::Number(n) => {
            map.insert(prefix.to_string(), n.to_string());
        }
        serde_yaml_ng::Value::Bool(b) => {
            map.insert(prefix.to_string(), b.to_string());
        }
        _ => {}
    }
}

static ACTIVE_LOCALE: LazyLock<RwLock<Cow<'static, str>>> = LazyLock::new(|| RwLock::new(system_language()));

#[inline]
pub fn locale_alias(locale: &str) -> Option<&'static str> {
    if locale.eq_ignore_ascii_case("ja") || locale.eq_ignore_ascii_case("ja-jp") || locale.eq_ignore_ascii_case("jp") {
        Some("jp")
    } else if locale.eq_ignore_ascii_case("zh")
        || locale.eq_ignore_ascii_case("zh-cn")
        || locale.eq_ignore_ascii_case("zh-hans")
        || locale.eq_ignore_ascii_case("zh-sg")
        || locale.eq_ignore_ascii_case("zh-my")
        || locale.eq_ignore_ascii_case("zh-chs")
    {
        Some("zh")
    } else if locale.eq_ignore_ascii_case("zh-tw")
        || locale.eq_ignore_ascii_case("zh-hk")
        || locale.eq_ignore_ascii_case("zh-hant")
        || locale.eq_ignore_ascii_case("zh-mo")
        || locale.eq_ignore_ascii_case("zh-cht")
    {
        Some("zhtw")
    } else {
        None
    }
}

#[inline]
pub fn resolve_supported_language(language: &str) -> Option<Cow<'static, str>> {
    if language.is_empty() {
        return None;
    }
    let normalized = language.to_lowercase().replace('_', "-");
    let segments: Vec<&str> = normalized.split('-').collect();
    for i in (1..=segments.len()).rev() {
        let prefix = segments[..i].join("-");
        if let Some(alias) = locale_alias(&prefix)
            && let Some(&found) = SUPPORTED_LOCALES.iter().find(|&&l| l.eq_ignore_ascii_case(alias))
        {
            return Some(Cow::Borrowed(found));
        }
        if let Some(&found) = SUPPORTED_LOCALES.iter().find(|&&l| l.eq_ignore_ascii_case(&prefix)) {
            return Some(Cow::Borrowed(found));
        }
    }
    None
}

/// Parse an HTTP `Accept-Language` header (e.g. "zh-CN,zh;q=0.9,en;q=0.8")
/// and resolve the highest-preference supported language.
pub fn resolve_accept_language(header_value: &str) -> Option<Cow<'static, str>> {
    if header_value.is_empty() {
        return None;
    }
    let mut candidates: Vec<(&str, f32)> = Vec::new();
    for part in header_value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut subparts = part.split(';');
        let lang = subparts.next().unwrap_or("").trim();
        if lang.is_empty() || lang == "*" {
            continue;
        }
        let mut quality = 1.0f32;
        for param in subparts {
            let param = param.trim();
            if let Some(rest) = param.strip_prefix("q=")
                && let Ok(q) = rest.trim().parse::<f32>()
            {
                quality = q;
            }
        }
        if quality > 0.0 {
            candidates.push((lang, quality));
        }
    }
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (lang, _) in candidates {
        if let Some(resolved) = resolve_supported_language(lang) {
            return Some(resolved);
        }
    }
    None
}

#[inline]
pub fn current_language(language: Option<&str>) -> Cow<'static, str> {
    language
        .filter(|lang| !lang.is_empty())
        .and_then(resolve_supported_language)
        .unwrap_or_else(|| ACTIVE_LOCALE.read().clone())
}

#[inline]
pub fn system_language() -> Cow<'static, str> {
    let env_lang = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .ok();

    if let Some(val) = env_lang {
        let clean = val.split('.').next().unwrap_or(&val).split('@').next().unwrap_or(&val);
        if let Some(resolved) = resolve_supported_language(clean) {
            return resolved;
        }
    }
    Cow::Borrowed(DEFAULT_LANGUAGE)
}

#[inline]
pub fn sync_locale(language: Option<&str>) {
    let lang = current_language(language);
    *ACTIVE_LOCALE.write() = lang;
}

#[inline]
pub fn set_locale(language: &str) {
    let lang = resolve_supported_language(language).unwrap_or(Cow::Borrowed(DEFAULT_LANGUAGE));
    *ACTIVE_LOCALE.write() = lang;
}

#[inline]
pub fn translate(key: &str) -> Cow<'_, str> {
    translate_for(key, None)
}

/// Resolve an explicit caller language without changing the service-global
/// locale. Browser sessions can use this independently of backend messages.
#[inline]
pub fn translate_for<'a>(key: &'a str, language: Option<&str>) -> Cow<'a, str> {
    let normalized = key.replace(['/', ':'], ".");
    let current = match language {
        Some(language) => resolve_supported_language(language).unwrap_or(Cow::Borrowed(DEFAULT_LANGUAGE)),
        None => ACTIVE_LOCALE.read().clone(),
    };
    let data = &*LOCALES_DATA;

    if let Some(locale_map) = data.get(current.as_ref())
        && let Some(val) = locale_map.get(&normalized)
    {
        return Cow::Owned(val.clone());
    }

    if current != DEFAULT_LANGUAGE
        && let Some(zh_map) = data.get(DEFAULT_LANGUAGE)
        && let Some(val) = zh_map.get(&normalized)
    {
        return Cow::Owned(val.clone());
    }

    Cow::Borrowed(key)
}

fn snake_to_camel(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize = false;
    for c in s.chars() {
        if c == '_' || c == '-' {
            capitalize = true;
        } else if capitalize {
            result.extend(c.to_uppercase());
            capitalize = false;
        } else {
            result.push(c);
        }
    }
    result
}

/// Translate a service API error code into a localized message.
///
/// Looks up `service.errors.<code_snake_case>` and `service.errors.<code_camel_case>`.
/// If neither key exists or language is None, returns `Cow::Borrowed(default_message)`.
pub fn translate_service_error<'a>(
    code: &str,
    default_message: &'a str,
    language: Option<&str>,
) -> Cow<'a, str> {
    let Some(lang) = language else {
        return Cow::Borrowed(default_message);
    };
    if lang.is_empty() {
        return Cow::Borrowed(default_message);
    }

    let snake_key = format!("service.errors.{code}");
    let translated = translate_for(&snake_key, Some(lang));
    if translated != snake_key {
        return Cow::Owned(translated.into_owned());
    }

    let camel = snake_to_camel(code);
    let camel_key = format!("service.errors.{camel}");
    let translated = translate_for(&camel_key, Some(lang));
    if translated != camel_key {
        return Cow::Owned(translated.into_owned());
    }

    Cow::Borrowed(default_message)
}

#[macro_export]
macro_rules! t {
    ($key:expr) => {
        $crate::translate(&$key)
    };
    ($key:expr, $($arg_name:ident = $arg_value:expr),*) => {
        {
            let mut _text = $crate::translate(&$key).into_owned();
            $(
                _text = _text.replace(&format!("{{{}}}", stringify!($arg_name)), &$arg_value);
            )*
            ::std::borrow::Cow::<'static, str>::Owned(_text)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_alias_resolves_common_variants() {
        assert_eq!(locale_alias("zh-cn"), Some("zh"));
        assert_eq!(locale_alias("zh-CN"), Some("zh"));
        assert_eq!(locale_alias("zh-tw"), Some("zhtw"));
        assert_eq!(locale_alias("zh-HK"), Some("zhtw"));
        assert_eq!(locale_alias("ja-jp"), Some("jp"));
        assert_eq!(locale_alias("ja"), Some("jp"));
    }

    #[test]
    fn resolve_supported_language_normalizes_and_matches() {
        assert_eq!(resolve_supported_language("zh_CN").as_deref(), Some("zh"));
        assert_eq!(resolve_supported_language("zh-Hans").as_deref(), Some("zh"));
        assert_eq!(resolve_supported_language("en-US").as_deref(), Some("en"));
        assert_eq!(resolve_supported_language("en_GB").as_deref(), Some("en"));
        assert_eq!(resolve_supported_language("ja_JP").as_deref(), Some("jp"));
        assert_eq!(resolve_supported_language("unknown_LANG"), None);
    }

    #[test]
    fn translation_looks_up_keys_and_supports_formatting() {
        assert_eq!(
            translate_for("notifications.dashboardToggled.title", Some("zh")),
            "仪表板"
        );
        assert_eq!(
            translate_for("notifications.clashModeChanged.body", Some("zh")),
            "已切换至 {mode}。"
        );
        assert_eq!(
            translate_for("notifications.dashboardToggled.title", Some("en")),
            "Dashboard"
        );
        assert_eq!(
            translate_for("notifications.clashModeChanged.body", Some("en")),
            "Switched to {mode}."
        );
    }

    #[test]
    fn fallback_to_default_language_when_key_missing_in_current_locale() {
        let untranslated = translate_for("notifications.nonExistentKey", Some("en"));
        assert_eq!(untranslated, "notifications.nonExistentKey");
    }

    #[test]
    fn all_embedded_locales_parse_and_explicit_languages_do_not_change_global_locale() {
        let before = current_language(None);
        for locale in SUPPORTED_LOCALES {
            assert!(
                LOCALES_DATA.get(locale).is_some_and(|entries| !entries.is_empty()),
                "{locale}"
            );
        }
        assert_eq!(
            translate_for("notifications.dashboardToggled.title", Some("en-US")),
            "Dashboard"
        );
        assert_eq!(
            translate_for("notifications.dashboardToggled.title", Some("zh-CN")),
            "仪表板"
        );
        assert_eq!(
            translate_for("notifications.dashboardToggled.title", Some("unsupported")),
            "仪表板"
        );
        assert_eq!(current_language(None), before);
    }

    #[test]
    fn resolve_accept_language_parses_quality_and_variants() {
        assert_eq!(resolve_accept_language("zh-CN,zh;q=0.9,en;q=0.8").as_deref(), Some("zh"));
        assert_eq!(resolve_accept_language("en-US,en;q=0.9,zh;q=0.8").as_deref(), Some("en"));
        assert_eq!(resolve_accept_language("fr-FR,fr;q=0.9,en;q=0.8").as_deref(), Some("en"));
        assert_eq!(resolve_accept_language("zh-TW,zh;q=0.8").as_deref(), Some("zhtw"));
        assert_eq!(resolve_accept_language("ja, en;q=0.5").as_deref(), Some("jp"));
        assert_eq!(resolve_accept_language("unknown-lang;q=1.0"), None);
        assert_eq!(resolve_accept_language(""), None);
    }

    #[test]
    fn translate_service_error_localizes_known_codes_and_preserves_fallback() {
        assert_eq!(
            translate_service_error("unauthorized", "authentication required", Some("zh")),
            "需要身份认证"
        );
        assert_eq!(
            translate_service_error("unauthorized", "authentication required", Some("en")),
            "Authentication required"
        );
        assert_eq!(
            translate_service_error("unauthorized", "authentication required", Some("zhtw")),
            "需要身分認證"
        );
        assert_eq!(
            translate_service_error("shutting_down", "service is shutting down", Some("zh")),
            "服务正在关闭"
        );
        assert_eq!(
            translate_service_error("shutting_down", "service is shutting down", Some("en")),
            "Service is shutting down"
        );
        assert_eq!(
            translate_service_error("shutting_down", "service is shutting down", None),
            "service is shutting down"
        );
        assert_eq!(
            translate_service_error("custom_unknown", "custom error detail", Some("zh")),
            "custom error detail"
        );
    }
}
