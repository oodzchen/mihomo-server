//! Catalog-first deletion: a committed catalog can never reference a removed file.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeleteJournal {
    schema_version: u32,
    uid: String,
    file: Option<String>,
}

impl ProfileStore {
    pub fn delete_profile(&mut self, uid: &str, active: Option<&str>) -> Result<()> {
        self.begin_delete(uid, active)?;
        let result = self.publish_delete();
        let recovery = self.recover_delete();
        result.map_err(|error| error.context(format!("profile deletion recovery: {recovery:?}")))?;
        recovery.context("profile deleted from catalog; file cleanup is pending")
    }

    /// Staging is public for deterministic interruption tests; caller owns the data lock.
    pub fn begin_delete(&mut self, uid: &str, active: Option<&str>) -> Result<()> {
        ensure!(
            Some(uid) != active && self.profiles.current.as_deref() != Some(uid),
            "current profile cannot be deleted; select another profile first"
        );
        ensure!(!uid.is_empty() && uid.len() <= 256, "invalid profile UID");
        ensure!(
            !self.data_dir.join("profile-delete.yaml").try_exists()?
                && !self.data_dir.join("profile-refresh.yaml").try_exists()?
                && !self.data_dir.join("profile-merge.yaml").try_exists()?,
            "profile recovery is pending"
        );
        let item = self.get_item(uid)?;
        ensure!(
            matches!(item.itype.as_deref(), Some("local" | "remote")),
            "only local or remote profiles can be deleted"
        );
        if let Some(option) = &item.option {
            ensure!(
                [
                    &option.merge,
                    &option.script,
                    &option.rules,
                    &option.proxies,
                    &option.groups
                ]
                .iter()
                .all(|link| link.is_none()),
                "linked profile deletion is not supported yet"
            );
        }
        for other in self.profiles.items.iter().flatten() {
            if let Some(option) = &other.option {
                ensure!(
                    [
                        &option.merge,
                        &option.script,
                        &option.rules,
                        &option.proxies,
                        &option.groups
                    ]
                    .iter()
                    .all(|link| link.as_deref() != Some(uid)),
                    "profile is referenced by an enhancement link"
                );
            }
        }
        let journal = DeleteJournal {
            schema_version: 1,
            uid: uid.into(),
            file: item.file.as_ref().map(ToString::to_string),
        };
        if let Some(file) = &journal.file {
            self.deletable_file(file)?;
        }
        let path = self.data_dir.join("profile-delete.yaml");
        let temporary = self.data_dir.join(format!("profile-delete-{}.tmp", unique_id()?));
        let result = (|| {
            write_new(&temporary, serde_yaml_ng::to_string(&journal)?.as_bytes())?;
            fs::rename(&temporary, &path)?;
            sync_directory(&self.data_dir)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn publish_delete(&mut self) -> Result<()> {
        let journal = self.delete_journal()?.context("no pending profile deletion")?;
        ensure!(
            self.profiles.current.as_deref() != Some(journal.uid.as_str()),
            "current profile cannot be deleted"
        );
        let mut profiles = self.profiles.clone();
        let items = profiles.items.as_mut().context("profile catalog is empty")?;
        ensure!(
            items
                .iter()
                .any(|item| item.uid.as_deref() == Some(journal.uid.as_str())),
            "profile missing while deleting"
        );
        items.retain(|item| item.uid.as_deref() != Some(journal.uid.as_str()));
        self.save(profiles)
    }

    /// Present UID means no catalog commit: abort cleanup. Absent UID means finish cleanup.
    pub fn recover_delete(&mut self) -> Result<()> {
        let Some(journal) = self.delete_journal()? else {
            return Ok(());
        };
        if let Ok(item) = self.get_item(&journal.uid) {
            ensure!(
                item.file.as_deref() == journal.file.as_deref(),
                "deletion journal conflicts with catalog"
            );
        } else if let Some(file) = journal.file {
            // Copied catalogs can legitimately share files: retain content referenced elsewhere.
            if !self
                .profiles
                .items
                .iter()
                .flatten()
                .any(|item| item.file.as_deref() == Some(file.as_str()))
            {
                if self.deletable_file(&file)? {
                    fs::remove_file(self.data_dir.join("profiles").join(&file))?;
                }
                sync_directory(&self.data_dir.join("profiles"))?;
            }
        }
        fs::remove_file(self.data_dir.join("profile-delete.yaml"))?;
        sync_directory(&self.data_dir)
    }

    fn deletable_file(&self, file: &str) -> Result<bool> {
        validate_profile_file(file)?;
        match fs::symlink_metadata(self.data_dir.join("profiles").join(file)) {
            Ok(metadata) => {
                ensure!(metadata.is_file(), "profile deletion requires a regular file");
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn delete_journal(&self) -> Result<Option<DeleteJournal>> {
        let path = self.data_dir.join("profile-delete.yaml");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.is_file() && metadata.len() <= 64 * 1024,
            "unsafe profile deletion journal"
        );
        let journal: DeleteJournal = serde_yaml_ng::from_str(&fs::read_to_string(path)?)?;
        ensure!(
            journal.schema_version == 1 && !journal.uid.is_empty() && journal.uid.len() <= 256,
            "invalid profile deletion journal"
        );
        if let Some(file) = &journal.file {
            validate_profile_file(file)?;
        }
        Ok(Some(journal))
    }
}
