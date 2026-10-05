//! The client's own saved settings. Only the interface language is kept: a
//! copy of the instance's shared preference, or a choice made on the status
//! page while no instance runs (it becomes the instance's once one does).
use crate::i18n::Language;
use anyhow::{Context as _, Result};
use serde_json::{Value, json};
use std::path::PathBuf;

/// `$XDG_CONFIG_HOME`, else `~/.config`.
pub fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

/// Where the settings live; `None` keeps them in memory only (tests).
pub struct Store(Option<PathBuf>);

impl Store {
    pub fn user() -> Self {
        Self(config_home().map(|config| config.join("mihomo-server-desktop").join("settings.json")))
    }

    #[cfg(test)]
    pub fn memory() -> Self {
        Self(None)
    }

    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self(Some(path))
    }

    /// The saved language; a missing or unreadable file is none.
    pub fn language(&self) -> Option<Language> {
        let text = std::fs::read_to_string(self.0.as_ref()?).ok()?;
        let settings: Value = serde_json::from_str(&text).ok()?;
        Language::from_code(settings.get("language")?.as_str()?)
    }

    /// Replace the file atomically (`None` saves no language).
    pub fn save_language(&self, language: Option<Language>) -> Result<()> {
        let Some(path) = &self.0 else { return Ok(()) };
        let directory = path.parent().context("settings directory")?;
        std::fs::create_dir_all(directory)?;
        let temporary = path.with_extension("json.new");
        let text = serde_json::to_string_pretty(&json!({"language": language.map(Language::code)}))?;
        std::fs::write(&temporary, text + "\n").with_context(|| format!("cannot write {}", temporary.display()))?;
        std::fs::rename(&temporary, path).with_context(|| format!("cannot replace {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_round_trip_and_bad_files_are_ignored() {
        let directory = std::env::temp_dir().join(format!("mihomo-desktop-settings-{}", std::process::id()));
        let path = directory.join("nested").join("settings.json");
        let store = Store::at(path.clone());
        assert_eq!(store.language(), None);
        store.save_language(Some(Language::Zhtw)).unwrap();
        assert_eq!(store.language(), Some(Language::Zhtw));
        store.save_language(None).unwrap();
        assert_eq!(store.language(), None);
        std::fs::write(&path, "{broken").unwrap();
        assert_eq!(store.language(), None);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
