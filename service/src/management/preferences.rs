//! Per-instance presentation preferences shared by every client of this
//! instance (Web UI, desktop client), with change notification. They never
//! affect the core or its configuration.
use crate::secure_fs::{create_private, sync_directory};
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};
use tokio::sync::{Mutex, watch};

const MAX_BYTES: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Zh,
    Zhtw,
    En,
}

/// Unset fields mean "no preference": each client uses its own default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    #[serde(default)]
    pub language: Option<Language>,
}

pub struct PreferenceStore {
    /// `None` keeps preferences in memory only (tests, embedded routers).
    path: Option<PathBuf>,
    current: watch::Sender<Preferences>,
    write: Mutex<()>,
}

impl PreferenceStore {
    pub fn in_memory() -> Self {
        Self {
            path: None,
            current: watch::channel(Preferences::default()).0,
            write: Mutex::new(()),
        }
    }

    /// An unreadable or invalid file is reported and replaced on the next
    /// change: a presentation preference must never keep the service down.
    pub fn load(path: PathBuf) -> Self {
        let preferences = match read(&path) {
            Ok(preferences) => preferences,
            Err(error) => {
                eprintln!("ignoring preferences {}: {error:#}", path.display());
                Preferences::default()
            }
        };
        Self {
            path: Some(path),
            current: watch::channel(preferences).0,
            write: Mutex::new(()),
        }
    }

    pub fn get(&self) -> Preferences {
        self.current.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Preferences> {
        self.current.subscribe()
    }

    /// Persist first, then notify subscribers, so a published value survives
    /// a restart. Setting the current value is a no-op without an event;
    /// `None` clears the preference.
    pub async fn set_language(&self, language: Option<Language>) -> Result<Preferences> {
        let _write = self.write.lock().await;
        let mut next = self.get();
        if next.language == language {
            return Ok(next);
        }
        next.language = language;
        if let Some(path) = &self.path {
            let path = path.clone();
            let saved = next.clone();
            tokio::task::spawn_blocking(move || write(&path, &saved)).await??;
        }
        self.current.send_replace(next.clone());
        Ok(next)
    }
}

fn read(path: &Path) -> Result<Preferences> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Preferences::default()),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "preferences must not be a symlink"
    );
    let mut text = String::new();
    file.take(MAX_BYTES + 1).read_to_string(&mut text)?;
    ensure!(text.len() as u64 <= MAX_BYTES, "preferences exceed {MAX_BYTES} bytes");
    serde_json::from_str(&text).context("invalid preferences")
}

/// Atomic replacement: a private temporary file renamed over the old one.
fn write(path: &Path, preferences: &Preferences) -> Result<()> {
    let directory = path.parent().context("preferences directory")?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_extension(format!("json.{nanos}.tmp"));
    let result = (|| -> Result<()> {
        let mut file = create_private(&temp).context("create preferences file")?;
        file.write_all(serde_json::to_string_pretty(preferences)?.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        sync_directory(directory)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.with_context(|| format!("cannot save {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn language_persists_and_notifies_only_on_change() {
        let directory = std::env::temp_dir().join(format!("mihomo-preferences-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("preferences.json");

        let store = PreferenceStore::load(path.clone());
        assert_eq!(store.get(), Preferences::default());
        let mut changes = store.subscribe();
        changes.borrow_and_update();
        store.set_language(Some(Language::En)).await.unwrap();
        assert!(changes.has_changed().unwrap());
        assert_eq!(changes.borrow_and_update().language, Some(Language::En));
        store.set_language(Some(Language::En)).await.unwrap();
        assert!(!changes.has_changed().unwrap(), "an unchanged value sends no event");

        let reloaded = PreferenceStore::load(path.clone());
        assert_eq!(reloaded.get().language, Some(Language::En));
        reloaded.set_language(None).await.unwrap();
        assert_eq!(PreferenceStore::load(path.clone()).get(), Preferences::default());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        }

        fs::write(&path, "{\"language\":\"klingon\"}").unwrap();
        assert_eq!(PreferenceStore::load(path).get(), Preferences::default());
        fs::remove_dir_all(&directory).unwrap();
    }
}
