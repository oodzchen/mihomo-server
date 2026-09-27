//! Immutable runtime revisions and an atomic commit/recovery record.
//! The service must hold its data-directory lock while using this store.

use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Mapping;

use crate::enhance::merge::use_merge;

pub const MAX_CONFIG_BYTES: usize = 8 * 1024 * 1024;
const CONTROLLER_FIELDS: [&str; 4] = [
    "external-controller",
    "external-controller-tls",
    "external-controller-unix",
    "external-controller-pipe",
];
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeState {
    pub schema_version: u32,
    pub current: Option<Revision>,
    pub pending: Option<Revision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_profile: Option<String>,
}

pub struct RuntimeStore {
    root: PathBuf,
    state: RuntimeState,
}

/// Preserve upstream merge semantics, then enforce the service controller boundary.
pub fn generate(base: Mapping, overlay: &Mapping) -> Result<Mapping> {
    let config = use_merge(overlay, base);
    for field in CONTROLLER_FIELDS {
        if let Some(value) = config.get(field) {
            ensure!(
                value.as_str() == Some(""),
                "{field} must be empty; the service owns its private controller"
            );
        }
    }
    Ok(config)
}

pub fn parse(yaml: &str) -> Result<Mapping> {
    ensure!(yaml.len() <= MAX_CONFIG_BYTES, "configuration exceeds 8 MiB");
    let config = serde_yaml_ng::from_str::<Mapping>(yaml).context("configuration must be a YAML mapping")?;
    generate(config, &Mapping::new())
}

/// Subscription controller addresses belong to the source application's runtime.
/// Discard them in the generated candidate, without rewriting the saved source.
/// Explicit runtime edits and enhancements still use the strict controller check.
pub fn normalize_profile(mut config: Mapping) -> Mapping {
    for field in CONTROLLER_FIELDS {
        config.remove(field);
    }
    config
}

pub fn parse_profile(yaml: &str) -> Result<Mapping> {
    ensure!(yaml.len() <= MAX_CONFIG_BYTES, "configuration exceeds 8 MiB");
    let config = serde_yaml_ng::from_str::<Mapping>(yaml).context("configuration must be a YAML mapping")?;
    generate(normalize_profile(config), &Mapping::new())
}

impl RuntimeStore {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join("config");
        fs::create_dir_all(root.join("revisions"))?;
        let manifest = root.join("state.yaml");
        let state = if manifest.try_exists()? {
            ensure!(manifest.metadata()?.len() <= 64 * 1024, "runtime manifest is too large");
            serde_yaml_ng::from_str::<RuntimeState>(&fs::read_to_string(&manifest)?)?
        } else {
            RuntimeState {
                schema_version: 1,
                current: None,
                pending: None,
                active_profile: None,
                pending_profile: None,
            }
        };
        ensure!(state.schema_version == 1, "unsupported runtime manifest version");
        let mut store = Self { root, state };
        if let Some(current) = &store.state.current {
            store.path(current)?;
        }
        // After a service restart, the new owned core starts the last committed revision.
        // Never promote a candidate merely because it was written before the crash.
        if store.state.pending.is_some() {
            store.abort()?;
        }
        Ok(store)
    }

    pub fn state(&self) -> RuntimeState {
        self.state.clone()
    }

    pub fn path(&self, revision: &Revision) -> Result<PathBuf> {
        let name = revision.file.as_str();
        ensure!(
            name.starts_with("rev-")
                && name.ends_with(".yaml")
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || b"rev-.yaml".contains(&byte)),
            "invalid runtime revision filename"
        );
        Ok(self.root.join("revisions").join(name))
    }

    pub fn current_path(&self) -> Result<Option<PathBuf>> {
        self.state
            .current
            .as_ref()
            .map(|revision| self.path(revision))
            .transpose()
    }

    pub fn read_current(&self) -> Result<Mapping> {
        let path = self.current_path()?.context("no committed configuration")?;
        ensure!(
            path.metadata()?.len() <= MAX_CONFIG_BYTES as u64,
            "configuration exceeds 8 MiB"
        );
        parse(&fs::read_to_string(path)?)
    }

    pub fn stage(&self, config: Mapping) -> Result<Revision> {
        let config = generate(config, &Mapping::new())?;
        let yaml = serde_yaml_ng::to_string(&config)?;
        ensure!(yaml.len() <= MAX_CONFIG_BYTES, "configuration exceeds 8 MiB");
        let revision = Revision {
            file: format!("rev-{}.yaml", unique_id()?),
        };
        write_new(&self.path(&revision)?, yaml.as_bytes())?;
        sync_directory(&self.root.join("revisions"))?;
        Ok(revision)
    }

    pub fn begin(&mut self, revision: Revision) -> Result<()> {
        self.begin_profile(revision, None)
    }

    pub fn begin_profile(&mut self, revision: Revision, profile: Option<String>) -> Result<()> {
        ensure!(
            self.state.pending.is_none(),
            "configuration application is already pending"
        );
        ensure!(self.path(&revision)?.is_file(), "candidate revision is missing");
        let mut state = self.state.clone();
        state.pending = Some(revision);
        state.pending_profile = profile;
        self.save(state)
    }

    pub fn commit(&mut self) -> Result<()> {
        let mut state = self.state.clone();
        state.current = Some(state.pending.take().context("no pending configuration")?);
        state.active_profile = state.pending_profile.take();
        self.save(state)
    }

    pub fn abort(&mut self) -> Result<()> {
        let mut state = self.state.clone();
        state.pending = None;
        state.pending_profile = None;
        self.save(state)
    }

    pub fn restore(&mut self, previous: RuntimeState) -> Result<()> {
        ensure!(previous.pending.is_none(), "cannot restore a pending state");
        self.save(previous)
    }

    fn save(&mut self, state: RuntimeState) -> Result<()> {
        let temporary = self.root.join(format!("state-{}.tmp", unique_id()?));
        write_new(&temporary, serde_yaml_ng::to_string(&state)?.as_bytes())?;
        if let Err(error) = fs::rename(&temporary, self.root.join("state.yaml")) {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        // Rename is the logical commit point, even if the subsequent directory sync fails.
        self.state = state;
        sync_directory(&self.root)
    }
}

pub(super) fn unique_id() -> Result<String> {
    Ok(format!(
        "{:x}-{:x}-{:x}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ))
}

pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
