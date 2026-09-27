//! A profile file pointer and the runtime journal share a recoverable commit.
//! Old raw files remain immutable; garbage collection is a separate operation.
use super::*;
use crate::config::{PrfOption, runtime::Revision};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplacementKind {
    #[default]
    Refresh,
    RawEdit,
}

impl ReplacementKind {
    fn is_refresh(&self) -> bool {
        matches!(self, Self::Refresh)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RefreshJournal {
    schema_version: u32,
    #[serde(default, skip_serializing_if = "ReplacementKind::is_refresh")]
    kind: ReplacementKind,
    previous: PrfItem,
    candidate: PrfItem,
    /// Active refresh commits at this runtime revision; inactive at catalog rename.
    runtime_revision: Option<Revision>,
}

impl ProfileStore {
    /// Prepare private immutable content and recovery metadata before changing the catalog.
    pub fn begin_refresh(
        &mut self,
        uid: &str,
        remote: RemoteProfile,
        runtime_revision: Option<Revision>,
    ) -> Result<()> {
        let previous = self.get_item(uid)?.clone();
        ensure!(
            previous.itype.as_deref() == Some("remote"),
            "only remote profiles can be refreshed"
        );
        ensure!(
            subscription_url(previous.url.as_deref().context("remote URL missing")?)? == subscription_url(&remote.url)?,
            "remote profile changed during download"
        );
        let yaml = validate_yaml(&remote.yaml)?;
        let mut candidate = previous.clone();
        candidate.file = Some(format!("refresh-{}.yaml", unique_id()?).into());
        candidate.extra = remote.extra;
        candidate.home = remote.home.map(Into::into);
        candidate.option = PrfOption::merge(previous.option.as_ref(), Some(&remote.option));
        candidate.updated = Some(usize::try_from(
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        )?);
        self.begin_replacement(previous, candidate, yaml, runtime_revision, ReplacementKind::Refresh)
    }

    /// Compare the opaque file revision, then replace only raw content, preserving all metadata.
    pub fn begin_raw_edit(
        &mut self,
        uid: &str,
        revision: &str,
        yaml: &str,
        runtime_revision: Option<Revision>,
    ) -> Result<()> {
        let previous = self.get_item(uid)?.clone();
        Self::validate_base(&previous)?;
        ensure!(
            previous.file.as_deref() == Some(revision),
            "profile raw revision changed; reload before saving"
        );
        self.read_content(&previous)?;
        ensure!(yaml.len() <= MAX_CONFIG_BYTES, "profile exceeds 8 MiB");
        serde_yaml_ng::from_str::<Mapping>(yaml).context("profile must be a YAML mapping")?;
        let mut candidate = previous.clone();
        candidate.file = Some(format!("raw-{}.yaml", unique_id()?).into());
        self.begin_replacement(previous, candidate, yaml, runtime_revision, ReplacementKind::RawEdit)
    }

    fn begin_replacement(
        &mut self,
        previous: PrfItem,
        candidate: PrfItem,
        yaml: &str,
        runtime_revision: Option<Revision>,
        kind: ReplacementKind,
    ) -> Result<()> {
        let journal_path = self.data_dir.join("profile-refresh.yaml");
        ensure!(
            !journal_path.try_exists()?
                && !self.data_dir.join("profile-merge.yaml").try_exists()?
                && !self.data_dir.join("backup-restore.yaml").try_exists()?
                && !self.data_dir.join("profile-import.yaml").try_exists()?
                && !self.data_dir.join("profile-delete.yaml").try_exists()?,
            "profile refresh recovery is pending"
        );
        let journal = RefreshJournal {
            schema_version: 1,
            kind,
            previous,
            candidate,
            runtime_revision,
        };
        let bytes = serde_yaml_ng::to_string(&journal)?;
        ensure!(
            bytes.len() <= MAX_CONFIG_BYTES,
            "profile refresh metadata exceeds 8 MiB"
        );
        let content = self
            .data_dir
            .join("profiles")
            .join(journal.candidate.file.as_deref().unwrap());
        let temporary = self.data_dir.join(format!("profile-refresh-{}.tmp", unique_id()?));
        let result = (|| {
            write_new(&content, yaml.as_bytes())?;
            sync_directory(&self.data_dir.join("profiles"))?;
            write_new(&temporary, bytes.as_bytes())?;
            fs::rename(&temporary, &journal_path)?;
            sync_directory(&self.data_dir)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
            // Rename may already have published a recoverable journal.
            if !journal_path.try_exists()? {
                let _ = fs::remove_file(content);
            }
        }
        result
    }

    pub fn publish_refresh(&mut self) -> Result<()> {
        let journal = self.refresh_journal()?.context("no pending profile refresh")?;
        self.replace_refreshed_item(journal.candidate)
    }

    /// Call under the data lock before other mutations, at startup and after apply/rollback.
    /// A committed active refresh follows the runtime manifest, never a pending revision.
    pub fn recover_refresh(&mut self, committed: Option<&Revision>) -> Result<()> {
        let Some(journal) = self.refresh_journal()? else {
            return Ok(());
        };
        let uid = journal
            .previous
            .uid
            .as_deref()
            .context("refresh UID missing")?
            .to_string();
        let published = self.get_item(&uid)?.file == journal.candidate.file;
        let accepted = match &journal.runtime_revision {
            Some(expected) => committed == Some(expected),
            None => published,
        };
        let desired = if accepted { journal.candidate } else { journal.previous };
        if self.get_item(&uid)?.file != desired.file {
            self.replace_refreshed_item(desired)?;
        }
        // Do not overwrite subsequent node reconciliation when the file is already correct.
        fs::remove_file(self.data_dir.join("profile-refresh.yaml"))?;
        sync_directory(&self.data_dir)
    }

    fn replace_refreshed_item(&mut self, item: PrfItem) -> Result<()> {
        let mut profiles = self.profiles.clone();
        let existing = profiles
            .items
            .as_mut()
            .and_then(|items| items.iter_mut().find(|existing| existing.uid == item.uid))
            .context("refreshed profile missing")?;
        *existing = item;
        self.save(profiles)
    }

    fn refresh_journal(&self) -> Result<Option<RefreshJournal>> {
        let path = self.data_dir.join("profile-refresh.yaml");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_CONFIG_BYTES as u64,
            "unsafe profile refresh journal"
        );
        let journal: RefreshJournal = serde_yaml_ng::from_str(&fs::read_to_string(path)?)?;
        ensure!(
            journal.schema_version == 1,
            "unsupported profile refresh journal version"
        );
        ensure!(
            journal.previous.uid.is_some()
                && journal.previous.uid == journal.candidate.uid
                && (if journal.kind.is_refresh() {
                    journal.previous.itype.as_deref() == Some("remote")
                } else {
                    matches!(journal.previous.itype.as_deref(), Some("local" | "remote"))
                })
                && journal.candidate.itype == journal.previous.itype
                && journal.previous.url == journal.candidate.url
                && journal.previous.file != journal.candidate.file,
            "invalid profile refresh journal"
        );
        if !journal.kind.is_refresh() {
            let mut metadata = journal.candidate.clone();
            metadata.file = journal.previous.file.clone();
            ensure!(
                serde_json::to_value(metadata)? == serde_json::to_value(&journal.previous)?,
                "raw edit journal changes profile metadata"
            );
        }
        let current = self.get_item(journal.previous.uid.as_deref().unwrap())?;
        ensure!(
            current.file == journal.previous.file || current.file == journal.candidate.file,
            "refresh journal conflicts with catalog"
        );
        for item in [&journal.previous, &journal.candidate] {
            let file = item.file.as_deref().context("refresh file missing")?;
            validate_profile_file(file)?;
            let metadata = fs::symlink_metadata(self.data_dir.join("profiles").join(file))?;
            ensure!(
                metadata.is_file() && metadata.len() <= MAX_CONFIG_BYTES as u64,
                "unsafe profile refresh content"
            );
        }
        Ok(Some(journal))
    }
}
