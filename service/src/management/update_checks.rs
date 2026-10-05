//! The latest results of explicit update checks, kept across restarts so
//! clients can offer a found update without checking again. Each record
//! names the version installed when it was taken: once that changes, the
//! record no longer says anything about an available update.
use super::preferences::{read_json, write_json};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoreChannel {
    Stable,
    Alpha,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreCheck {
    pub channel: CoreChannel,
    pub installed: String,
    pub latest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceCheck {
    /// `None` when this program does not run from a versioned release.
    pub installed: Option<String>,
    pub latest: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateChecks {
    #[serde(default)]
    pub core: Option<CoreCheck>,
    #[serde(default)]
    pub service: Option<ServiceCheck>,
}

pub struct UpdateCheckStore {
    /// `None` keeps records in memory only (tests, embedded routers).
    path: Option<PathBuf>,
    current: Mutex<UpdateChecks>,
}

impl UpdateCheckStore {
    pub fn in_memory() -> Self {
        Self {
            path: None,
            current: Mutex::new(UpdateChecks::default()),
        }
    }

    /// An unreadable record is reported and forgotten: it only saves a check.
    pub fn load(path: PathBuf) -> Self {
        let checks = read_json(&path, "update checks").unwrap_or_else(|error| {
            eprintln!("ignoring update checks {}: {error:#}", path.display());
            UpdateChecks::default()
        });
        Self {
            path: Some(path),
            current: Mutex::new(checks),
        }
    }

    pub async fn get(&self) -> UpdateChecks {
        self.current.lock().await.clone()
    }

    pub async fn record_core(&self, check: CoreCheck) -> Result<()> {
        self.update(|checks| checks.core = Some(check)).await
    }

    pub async fn record_service(&self, check: ServiceCheck) -> Result<()> {
        self.update(|checks| checks.service = Some(check)).await
    }

    async fn update(&self, change: impl FnOnce(&mut UpdateChecks)) -> Result<()> {
        let mut current = self.current.lock().await;
        let mut next = current.clone();
        change(&mut next);
        if next == *current {
            return Ok(());
        }
        if let Some(path) = &self.path {
            let path = path.clone();
            let saved = next.clone();
            tokio::task::spawn_blocking(move || write_json(&path, &saved, "update checks")).await??;
        }
        *current = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn records_persist_across_loads() {
        let directory = std::env::temp_dir().join(format!("mihomo-update-checks-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("update-checks.json");

        let store = UpdateCheckStore::load(path.clone());
        assert_eq!(store.get().await, UpdateChecks::default());
        let core = CoreCheck {
            channel: CoreChannel::Alpha,
            installed: "v1.0.0".into(),
            latest: "alpha-abc".into(),
        };
        let service = ServiceCheck {
            installed: None,
            latest: "v0.3.0".into(),
        };
        store.record_core(core.clone()).await.unwrap();
        store.record_service(service.clone()).await.unwrap();

        let reloaded = UpdateCheckStore::load(path.clone()).get().await;
        assert_eq!(reloaded.core, Some(core));
        assert_eq!(reloaded.service, Some(service));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        }

        fs::write(&path, "not json").unwrap();
        assert_eq!(UpdateCheckStore::load(path).get().await, UpdateChecks::default());
        fs::remove_dir_all(&directory).unwrap();
    }
}
