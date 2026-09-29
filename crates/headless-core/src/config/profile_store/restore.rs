//! Durable catalog/settings restore intent. The runtime manifest is the commit marker.
//! Callers own the data lock and must validate/generate/probe the candidate before begin.
use super::super::{
    runtime::{Revision, RuntimeStore},
    settings::{MAX_SETTINGS_BYTES, ServiceSettings, SettingsStore},
};
use super::sha256_hex as hash;
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Read as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
};

const JOURNAL: &str = "backup-restore.yaml";
const MAX_JOURNAL_BYTES: usize = 2 * MAX_CONFIG_BYTES + 2 * 1024 * 1024;
const MAX_SOURCES: usize = 1020;
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const OTHER_JOURNALS: [&str; 5] = [
    "profile-import.yaml",
    "profile-refresh.yaml",
    "profile-merge.yaml",
    "profile-delete.yaml",
    "settings-transaction.yaml",
];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreFile {
    file: String,
    bytes: usize,
    sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    transaction: String,
    previous_revision: Option<Revision>,
    revision: Revision,
    runtime_bytes: usize,
    runtime_sha256: String,
    previous: IProfiles,
    candidate: IProfiles,
    previous_settings: ServiceSettings,
    candidate_settings: ServiceSettings,
    files: Vec<RestoreFile>,
}
/// Owned, bounded sources and allocated filenames. Never a reusable validation receipt.
pub struct RestorePlan {
    journal: Journal,
    sources: Vec<Vec<u8>>,
}
impl RestorePlan {
    pub fn active_profile(&self) -> Option<&str> {
        self.journal.candidate.current.as_deref()
    }
}
fn equal(a: &IProfiles, b: &IProfiles) -> Result<bool> {
    Ok(serde_json::to_value(a)? == serde_json::to_value(b)?)
}
fn directory(path: &Path) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    ensure!(
        m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o7022 == 0,
        "unsafe restore directory"
    );
    Ok(())
}
fn regular(file: &File, limit: usize) -> Result<fs::Metadata> {
    let m = file.metadata()?;
    ensure!(
        m.is_file()
            && m.nlink() == 1
            && m.uid() == unsafe { libc::geteuid() }
            && m.mode() & 0o7077 == 0
            && m.len() <= limit as u64,
        "unsafe or oversized restore file"
    );
    Ok(m)
}
fn read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let before = regular(&file, limit)?;
    let mut bytes = Vec::new();
    (&file).take(limit as u64 + 1).read_to_end(&mut bytes)?;
    let after = regular(&file, limit)?;
    let path_meta = fs::symlink_metadata(path)?;
    ensure!(
        bytes.len() == before.len() as usize
            && before.len() == after.len()
            && before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
            && before.dev() == path_meta.dev()
            && before.ino() == path_meta.ino(),
        "restore file changed during read"
    );
    Ok(bytes)
}
fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn catalog(c: &IProfiles, settings: &ServiceSettings) -> Result<()> {
    let mut ids = BTreeMap::new();
    ensure!(
        serde_yaml_ng::to_string(c)?.len() + 40 <= MAX_CONFIG_BYTES,
        "restore catalog exceeds 8 MiB"
    );
    for item in c.items.iter().flatten() {
        let uid = item.uid.as_deref().context("restore UID missing")?;
        ensure!(
            !uid.trim().is_empty()
                && uid.len() <= 256
                && !uid.chars().any(char::is_control)
                && ids.insert(uid, item).is_none(),
            "invalid restore UID"
        );
        validate_profile_file(item.file.as_deref().context("restore source missing")?)?;
        ensure!(
            matches!(
                item.itype.as_deref(),
                Some("local" | "remote" | "merge" | "script" | "rules" | "proxies" | "groups")
            ),
            "invalid restore type"
        );
        if item.itype.as_deref() == Some("remote") {
            subscription_url(item.url.as_deref().context("restore URL missing")?)?;
        }
    }
    for item in c.items.iter().flatten() {
        if let Some(option) = &item.option {
            ensure!(
                matches!(item.itype.as_deref(), Some("local" | "remote")),
                "restore auxiliary has options"
            );
            for (link, kind) in [
                (&option.merge, "merge"),
                (&option.script, "script"),
                (&option.rules, "rules"),
                (&option.proxies, "proxies"),
                (&option.groups, "groups"),
            ] {
                if let Some(uid) = link {
                    ensure!(
                        ids.get(uid.as_str())
                            .is_some_and(|item| item.itype.as_deref() == Some(kind)),
                        "invalid restore auxiliary link"
                    );
                }
            }
        }
    }
    if let Some(uid) = c.current.as_deref() {
        ensure!(
            ids.get(uid)
                .is_some_and(|item| matches!(item.itype.as_deref(), Some("local" | "remote"))),
            "invalid restore active profile"
        );
    }
    settings.validate()?;
    ensure!(
        serde_yaml_ng::to_string(settings)?.len() <= MAX_SETTINGS_BYTES,
        "restore settings exceed 64 KiB"
    );
    ensure!(
        settings.profile_dns.keys().all(|uid| ids
            .get(uid.as_str())
            .is_some_and(|item| matches!(item.itype.as_deref(), Some("local" | "remote")))),
        "restore DNS preference is not a base profile"
    );
    Ok(())
}
fn source(kind: &str, bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes)?;
    match kind {
        "local" | "remote" => {
            super::super::runtime::parse_profile(text)?;
        }
        "merge" => {
            serde_yaml_ng::from_str::<Mapping>(text)?;
        }
        "script" => ensure!(
            !text.trim().is_empty() && bytes.len() <= 1024 * 1024,
            "invalid restore script"
        ),
        "rules" => SequenceKind::Rules.validate_source(text)?,
        "proxies" => SequenceKind::Proxies.validate_source(text)?,
        "groups" => SequenceKind::Groups.validate_source(text)?,
        _ => bail!("invalid restore source type"),
    }
    Ok(())
}
impl ProfileStore {
    fn restore_admission(&self, settings: &SettingsStore) -> Result<()> {
        ensure!(
            self.data_dir == settings.data_dir(),
            "restore stores have different roots"
        );
        directory(&self.data_dir)?;
        directory(&self.data_dir.join("profiles"))?;
        for name in OTHER_JOURNALS {
            ensure!(
                !exists(&self.data_dir.join(name))?,
                "another configuration transaction is pending"
            );
        }
        Ok(())
    }
    /// Read a previously verified disposable catalog into an opaque bounded plan.
    /// This validates sources, not scripts/Mihomo or runtime/settings equivalence.
    pub fn prepare_restore(
        &self,
        candidate: &ProfileStore,
        settings: &SettingsStore,
        candidate_settings: ServiceSettings,
        runtime: &RuntimeStore,
        revision: Revision,
    ) -> Result<RestorePlan> {
        self.restore_admission(settings)?;
        ensure!(
            runtime.data_dir() == self.data_dir,
            "restore runtime has a different root"
        );
        ensure!(!exists(&self.data_dir.join(JOURNAL))?, "restore recovery is pending");
        let state = runtime.state();
        ensure!(
            state.pending.is_none() && state.active_profile.as_deref() == self.profiles.current.as_deref(),
            "runtime/catalog is not settled"
        );
        directory(&self.data_dir.join("config"))?;
        directory(&self.data_dir.join("config/revisions"))?;
        let runtime_bytes = read(&runtime.path(&revision)?, MAX_CONFIG_BYTES)?;
        super::super::runtime::parse(std::str::from_utf8(&runtime_bytes)?)?;
        directory(&candidate.data_dir)?;
        directory(&candidate.data_dir.join("profiles"))?;
        catalog(&self.profiles, &settings.snapshot())?;
        catalog(&candidate.profiles, &candidate_settings)?;
        let transaction = unique_id()?;
        let mut restored = candidate.snapshot();
        let mut allocated = BTreeMap::<String, (String, String)>::new();
        let mut files = Vec::new();
        let mut sources = Vec::new();
        let mut total = 0;
        for item in restored.items.iter_mut().flatten() {
            let original = item.file.as_deref().unwrap().to_owned();
            let kind = item.itype.as_deref().unwrap();
            let file = if let Some((file, saved_kind)) = allocated.get(&original) {
                ensure!(kind == saved_kind, "shared restore source has different types");
                file.clone()
            } else {
                ensure!(sources.len() < MAX_SOURCES, "too many restore sources");
                let bytes = read(&candidate.data_dir.join("profiles").join(&original), MAX_CONFIG_BYTES)?;
                source(kind, &bytes)?;
                total += bytes.len();
                ensure!(total <= MAX_SOURCE_BYTES, "restore sources exceed 64 MiB");
                let ext = if kind == "script" { "js" } else { "yaml" };
                let file = format!("restore-{transaction}-{:04x}.{ext}", files.len());
                files.push(RestoreFile {
                    file: file.clone(),
                    bytes: bytes.len(),
                    sha256: hash(&bytes),
                });
                sources.push(bytes);
                allocated.insert(original, (file.clone(), kind.to_owned()));
                file
            };
            item.file = Some(file.into());
        }
        let journal = Journal {
            schema_version: 1,
            transaction,
            previous_revision: state.current,
            revision,
            runtime_bytes: runtime_bytes.len(),
            runtime_sha256: hash(&runtime_bytes),
            previous: self.snapshot(),
            candidate: restored,
            previous_settings: settings.snapshot(),
            candidate_settings,
            files,
        };
        self.check_restore_journal(&journal, runtime)?;
        Ok(RestorePlan { journal, sources })
    }
    fn check_restore_journal(&self, j: &Journal, runtime: &RuntimeStore) -> Result<()> {
        ensure!(
            j.schema_version == 1
                && j.transaction.len() <= 128
                && j.transaction.split('-').count() == 3
                && j.transaction
                    .split('-')
                    .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())),
            "invalid restore transaction"
        );
        runtime.path(&j.revision)?;
        if let Some(previous) = &j.previous_revision {
            runtime.path(previous)?;
        }
        ensure!(
            j.previous_revision.as_ref() != Some(&j.revision),
            "restore revision is already committed"
        );
        ensure!(
            j.runtime_bytes <= MAX_CONFIG_BYTES
                && j.runtime_sha256.len() == 64
                && j.runtime_sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid restore runtime digest"
        );
        catalog(&j.previous, &j.previous_settings)?;
        catalog(&j.candidate, &j.candidate_settings)?;
        ensure!(
            j.files.len() <= MAX_SOURCES && j.files.iter().all(|f| f.bytes <= MAX_CONFIG_BYTES),
            "invalid restore file count/size"
        );
        ensure!(
            j.files.iter().map(|f| f.bytes as u64).sum::<u64>() <= MAX_SOURCE_BYTES as u64,
            "invalid restore file budget"
        );
        let mut names = HashSet::new();
        for (index, file) in j.files.iter().enumerate() {
            let prefix = format!("restore-{}-{index:04x}", j.transaction);
            ensure!(
                (file.file == format!("{prefix}.yaml") || file.file == format!("{prefix}.js"))
                    && file.bytes <= MAX_CONFIG_BYTES
                    && file.sha256.len() == 64
                    && file
                        .sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid restore allocation"
            );
            names.insert(file.file.as_str());
        }
        let candidate_files: HashSet<_> = j
            .candidate
            .items
            .iter()
            .flatten()
            .map(|i| i.file.as_deref().unwrap())
            .collect();
        ensure!(
            names == candidate_files
                && j.previous
                    .items
                    .iter()
                    .flatten()
                    .all(|i| !names.contains(i.file.as_deref().unwrap())),
            "invalid restore file coverage"
        );
        ensure!(
            serde_yaml_ng::to_string(j)?.len() <= MAX_JOURNAL_BYTES,
            "restore journal is too large"
        );
        Ok(())
    }
    fn restore_journal(&self, runtime: &RuntimeStore) -> Result<Option<Journal>> {
        let path = self.data_dir.join(JOURNAL);
        if !exists(&path)? {
            return Ok(None);
        }
        directory(&self.data_dir)?;
        let bytes = read(&path, MAX_JOURNAL_BYTES)?;
        let j: Journal = serde_yaml_ng::from_slice(&bytes)?;
        self.check_restore_journal(&j, runtime)?;
        Ok(Some(j))
    }
    fn restore_state(&self, j: &Journal, settings: &SettingsStore, runtime: &RuntimeStore) -> Result<bool> {
        self.restore_admission(settings)?;
        ensure!(
            runtime.data_dir() == self.data_dir,
            "restore runtime has a different root"
        );
        let path = self.data_dir.join("profiles.yaml");
        let disk_catalog: IProfiles = if exists(&path)? {
            serde_yaml_ng::from_slice(&read(&path, MAX_CONFIG_BYTES)?)?
        } else {
            IProfiles::default()
        };
        let disk_settings: ServiceSettings =
            serde_yaml_ng::from_slice(&read(&self.data_dir.join("settings.yaml"), MAX_SETTINGS_BYTES)?)?;
        ensure!(
            equal(&self.profiles, &disk_catalog)? && settings.snapshot() == disk_settings,
            "restore stores changed on disk"
        );
        ensure!(
            (equal(&self.profiles, &j.previous)? || equal(&self.profiles, &j.candidate)?)
                && (disk_settings == j.previous_settings || disk_settings == j.candidate_settings),
            "restore conflicts with catalog/settings"
        );
        let state = runtime.state();
        directory(&self.data_dir.join("config"))?;
        let state_path = self.data_dir.join("config/state.yaml");
        if exists(&state_path)? {
            let disk_state: super::super::runtime::RuntimeState =
                serde_yaml_ng::from_slice(&read(&state_path, 64 * 1024)?)?;
            ensure!(
                serde_json::to_value(&state)? == serde_json::to_value(disk_state)?,
                "restore runtime changed on disk"
            );
        } else {
            ensure!(
                state.current.is_none() && state.pending.is_none(),
                "restore runtime manifest missing"
            );
        }
        let committed = state.current.as_ref() == Some(&j.revision);
        ensure!(
            committed || state.current == j.previous_revision,
            "restore conflicts with runtime revision"
        );
        let expected = if committed { &j.candidate } else { &j.previous };
        ensure!(
            state.active_profile.as_deref() == expected.current.as_deref(),
            "restore conflicts with runtime profile"
        );
        ensure!(
            state.pending.as_ref().is_none_or(|r| r == &j.revision)
                && (state.pending.is_none() || state.pending_profile.as_deref() == j.candidate.current.as_deref()),
            "restore conflicts with pending runtime"
        );
        Ok(committed)
    }
    fn restore_files(&self, j: &Journal, complete: bool) -> Result<()> {
        if complete {
            directory(&self.data_dir.join("config/revisions"))?;
            let bytes = read(
                &self.data_dir.join("config/revisions").join(&j.revision.file),
                j.runtime_bytes,
            )?;
            ensure!(
                bytes.len() == j.runtime_bytes && hash(&bytes) == j.runtime_sha256,
                "restore runtime changed"
            );
        }
        for file in &j.files {
            let path = self.data_dir.join("profiles").join(&file.file);
            if exists(&path)? {
                let bytes = read(&path, file.bytes)?;
                ensure!(
                    bytes.len() == file.bytes && hash(&bytes) == file.sha256,
                    "restored source changed"
                );
                for item in j
                    .candidate
                    .items
                    .iter()
                    .flatten()
                    .filter(|i| i.file.as_deref() == Some(&file.file))
                {
                    source(item.itype.as_deref().unwrap(), &bytes)?;
                }
            } else {
                ensure!(!complete, "restored source missing");
            }
            let part = path.with_extension("part");
            if exists(&part)? {
                read(&part, file.bytes)?;
                ensure!(!complete, "restore staging is incomplete");
            }
        }
        Ok(())
    }
    /// Durable intent precedes new immutable files; existing files are never overwritten.
    /// RuntimeStore::begin_profile must first register this plan's validated revision.
    pub fn begin_restore(&mut self, plan: RestorePlan, settings: &SettingsStore, runtime: &RuntimeStore) -> Result<()> {
        self.check_restore_journal(&plan.journal, runtime)?;
        ensure!(!exists(&self.data_dir.join(JOURNAL))?, "restore recovery is pending");
        ensure!(
            !self.restore_state(&plan.journal, settings, runtime)?
                && equal(&self.profiles, &plan.journal.previous)?
                && settings.snapshot() == plan.journal.previous_settings,
            "restore plan is stale"
        );
        ensure!(
            runtime.state().pending.as_ref() == Some(&plan.journal.revision),
            "restore runtime is not pending"
        );
        for file in &plan.journal.files {
            let path = self.data_dir.join("profiles").join(&file.file);
            ensure!(
                !exists(&path)? && !exists(&path.with_extension("part"))?,
                "restore allocation exists"
            );
        }
        let tmp = self.data_dir.join(format!("backup-restore-{}.tmp", unique_id()?));
        let result = (|| {
            write_new(&tmp, serde_yaml_ng::to_string(&plan.journal)?.as_bytes())?;
            fs::rename(&tmp, self.data_dir.join(JOURNAL))?;
            sync_directory(&self.data_dir)?;
            for (file, bytes) in plan.journal.files.iter().zip(plan.sources) {
                let path = self.data_dir.join("profiles").join(&file.file);
                let part = path.with_extension("part");
                write_new(&part, &bytes)?;
                fs::rename(part, path)?;
                sync_directory(&self.data_dir.join("profiles"))?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(tmp);
        }
        result
    }
    /// Publish both pointers before RuntimeStore::commit, which is the only commit point.
    pub fn publish_restore(&mut self, settings: &mut SettingsStore, runtime: &RuntimeStore) -> Result<()> {
        let j = self.restore_journal(runtime)?.context("no pending backup restore")?;
        ensure!(
            !self.restore_state(&j, settings, runtime)? && runtime.state().pending.as_ref() == Some(&j.revision),
            "restore runtime is not pending"
        );
        self.restore_files(&j, true)?;
        if !equal(&self.profiles, &j.candidate)? {
            self.save(j.candidate)?;
        }
        if settings.snapshot() != j.candidate_settings {
            settings.replace(j.candidate_settings)?;
        }
        Ok(())
    }
    /// Idempotent recovery: current==candidate rolls forward; current==previous rolls back.
    /// A post-rename fsync failure is already committed and must never roll back.
    pub fn recover_restore(&mut self, settings: &mut SettingsStore, runtime: &RuntimeStore) -> Result<()> {
        let Some(j) = self.restore_journal(runtime)? else {
            return Ok(());
        };
        ensure!(
            runtime.state().pending.is_none(),
            "abort uncommitted runtime before restore recovery"
        );
        let committed = self.restore_state(&j, settings, runtime)?;
        self.restore_files(&j, committed)?;
        let (catalog, target_settings) = if committed {
            (&j.candidate, &j.candidate_settings)
        } else {
            (&j.previous, &j.previous_settings)
        };
        if !equal(&self.profiles, catalog)? {
            self.save(catalog.clone())?;
        }
        if settings.snapshot() != *target_settings {
            settings.replace(target_settings.clone())?;
        }
        if !committed {
            for file in &j.files {
                let path = self.data_dir.join("profiles").join(&file.file);
                for path in [&path, &path.with_extension("part")] {
                    if exists(path)? {
                        fs::remove_file(path)?;
                    }
                }
            }
            sync_directory(&self.data_dir.join("profiles"))?;
        }
        fs::remove_file(self.data_dir.join(JOURNAL))?;
        sync_directory(&self.data_dir)
    }
}
