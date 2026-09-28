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
pub const DEFAULT_RETAINED_REVISIONS: usize = 10;
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
    #[cfg(unix)]
    pub(crate) fn data_dir(&self) -> &Path {
        self.root.parent().expect("runtime root has a parent")
    }

    pub fn open(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join("config");
        fs::create_dir_all(root.join("revisions"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = fs::set_permissions(&root, fs::Permissions::from_mode(0o700));
            let _ = fs::set_permissions(root.join("revisions"), fs::Permissions::from_mode(0o700));
        }
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
        let _ = store.clean_state_temporaries();
        if let Some(current) = &store.state.current {
            store.path(current)?;
        }
        // After a service restart, the new owned core starts the last committed revision.
        // Never promote a candidate merely because it was written before the crash.
        if store.state.pending.is_some() {
            store.abort()?;
        }
        let _ = store.gc_revisions(DEFAULT_RETAINED_REVISIONS);
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
        self.stage_yaml(&yaml)
    }

    /// Preserve an explicitly selected archived snapshot's exact bytes after validation.
    pub fn stage_yaml(&self, yaml: &str) -> Result<Revision> {
        parse(yaml)?;
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
        self.save(state)?;
        let _ = self.gc_revisions(DEFAULT_RETAINED_REVISIONS);
        Ok(())
    }

    pub fn abort(&mut self) -> Result<()> {
        let mut state = self.state.clone();
        state.pending = None;
        state.pending_profile = None;
        self.save(state)
    }

    pub fn restore(&mut self, previous: RuntimeState) -> Result<()> {
        ensure!(previous.pending.is_none(), "cannot restore a pending state");
        self.save(previous)?;
        let _ = self.gc_revisions(DEFAULT_RETAINED_REVISIONS);
        Ok(())
    }

    /// Garbage-collect unreferenced runtime revisions, retaining at least `keep_count`
    /// latest revisions in addition to active `current` and `pending` revisions.
    /// Also cleans up abandoned `state-*.tmp` temporary files.
    pub fn gc_revisions(&mut self, keep_count: usize) -> Result<usize> {
        let keep_count = keep_count.max(1);
        let mut pruned = self.clean_state_temporaries()?;

        let revisions_dir = self.root.join("revisions");
        if !revisions_dir.is_dir() {
            return Ok(pruned);
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(&revisions_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("rev-") && name_str.ends_with(".yaml") {
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(UNIX_EPOCH);
                entries.push((name_str.into_owned(), path, mtime));
            }
        }

        // Sort descending by mtime (newest first), tie-break by name
        entries.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.0.cmp(&a.0)));

        let current_file = self.state.current.as_ref().map(|r| r.file.as_str());
        let pending_file = self.state.pending.as_ref().map(|r| r.file.as_str());

        let mut kept = 0;
        for (name, path, _mtime) in entries {
            let is_active = Some(name.as_str()) == current_file || Some(name.as_str()) == pending_file;
            if is_active {
                continue;
            }
            if kept < keep_count {
                kept += 1;
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                pruned += 1;
            }
        }

        sync_directory(&revisions_dir)?;
        Ok(pruned)
    }

    /// Clean up orphan temporary state files (state-*.tmp) left over from interrupted saves.
    pub fn clean_state_temporaries(&self) -> Result<usize> {
        let mut cleaned = 0;
        if !self.root.is_dir() {
            return Ok(cleaned);
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("state-") && name_str.ends_with(".tmp") {
                let path = entry.path();
                if path.is_file() && fs::remove_file(path).is_ok() {
                    cleaned += 1;
                }
            }
        }
        sync_directory(&self.root)?;
        Ok(cleaned)
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
