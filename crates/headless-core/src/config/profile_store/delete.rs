//! Catalog-first deletion: a committed catalog can never reference a removed file.
use super::super::settings::SettingsStore;
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeletedAuxiliary {
    uid: String,
    file: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeleteJournal {
    schema_version: u32,
    uid: String,
    file: Option<String>,
    #[serde(default)]
    auxiliaries: Vec<DeletedAuxiliary>,
}

fn reserved(uid: &str) -> bool {
    matches!(uid, "Merge" | "Script" | "Rules" | "Proxies" | "Groups")
}

fn links(item: &PrfItem) -> Vec<(&str, &str)> {
    item.option.as_ref().map_or_else(Vec::new, |option| {
        [
            (&option.merge, "merge"),
            (&option.script, "script"),
            (&option.rules, "rules"),
            (&option.proxies, "proxies"),
            (&option.groups, "groups"),
        ]
        .into_iter()
        .filter_map(|(uid, kind)| uid.as_deref().map(|uid| (uid, kind)))
        .collect()
    })
}

impl ProfileStore {
    pub fn delete_profile(&mut self, uid: &str, active: Option<&str>) -> Result<()> {
        self.delete_profile_inner(uid, active, None)
    }

    /// The deletion journal remains until catalog-dependent settings and files are cleaned.
    pub fn delete_profile_with_settings(
        &mut self,
        uid: &str,
        active: Option<&str>,
        settings: &mut SettingsStore,
    ) -> Result<()> {
        self.delete_profile_inner(uid, active, Some(settings))
    }

    fn delete_profile_inner(
        &mut self,
        uid: &str,
        active: Option<&str>,
        settings: Option<&mut SettingsStore>,
    ) -> Result<()> {
        self.begin_delete(uid, active)?;
        let result = self.publish_delete();
        let recovery = self.recover_delete_inner(settings);
        result.map_err(|error| error.context(format!("profile deletion recovery: {recovery:?}")))?;
        recovery.context("profile deleted from catalog; settings/file cleanup is pending")
    }

    /// Staging is public for deterministic interruption tests; caller owns the data lock.
    pub fn begin_delete(&mut self, uid: &str, active: Option<&str>) -> Result<()> {
        ensure!(
            Some(uid) != active && self.profiles.current.as_deref() != Some(uid),
            "current profile cannot be deleted; select another profile first"
        );
        ensure!(!uid.is_empty() && uid.len() <= 256, "invalid profile UID");
        ensure!(
            !self.data_dir.join("backup-restore.yaml").try_exists()?
                && !self.data_dir.join("profile-import.yaml").try_exists()?
                && !self.data_dir.join("profile-delete.yaml").try_exists()?
                && !self.data_dir.join("profile-refresh.yaml").try_exists()?
                && !self.data_dir.join("profile-merge.yaml").try_exists()?,
            "profile recovery is pending"
        );
        let item = self.get_item(uid)?;
        ensure!(
            matches!(item.itype.as_deref(), Some("local" | "remote")),
            "only local or remote profiles can be deleted"
        );
        ensure!(!reserved(uid), "reserved profiles cannot be deleted");
        let journal = DeleteJournal {
            schema_version: 2,
            uid: uid.into(),
            file: item.file.as_ref().map(ToString::to_string),
            auxiliaries: self.delete_auxiliaries(uid)?,
        };
        for file in journal
            .file
            .iter()
            .chain(journal.auxiliaries.iter().filter_map(|item| item.file.as_ref()))
        {
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
        self.check_delete_catalog(&journal)?;
        items.retain(|item| {
            item.uid.as_deref() != Some(journal.uid.as_str())
                && !journal
                    .auxiliaries
                    .iter()
                    .any(|aux| item.uid.as_deref() == Some(aux.uid.as_str()))
        });
        self.save(profiles)
    }

    /// Present UID means no catalog commit: abort cleanup. Absent UID means finish cleanup.
    pub fn recover_delete(&mut self) -> Result<()> {
        self.recover_delete_inner(None)
    }

    pub fn recover_delete_with_settings(&mut self, settings: &mut SettingsStore) -> Result<()> {
        self.recover_delete_inner(Some(settings))
    }

    fn recover_delete_inner(&mut self, settings: Option<&mut SettingsStore>) -> Result<()> {
        let Some(journal) = self.delete_journal()? else {
            return Ok(());
        };
        if self.get_item(&journal.uid).is_ok() {
            self.check_delete_catalog(&journal)?;
        } else {
            ensure!(
                self.profiles.current.as_deref() != Some(journal.uid.as_str()),
                "deletion conflicts with current profile"
            );
            ensure!(
                journal.auxiliaries.iter().all(|aux| self.get_item(&aux.uid).is_err()),
                "partial cascade catalog commit"
            );
            ensure!(
                !self.profiles.items.iter().flatten().any(|item| links(item)
                    .iter()
                    .any(|(link, _)| *link == journal.uid || journal.auxiliaries.iter().any(|aux| aux.uid == *link))),
                "surviving enhancement link points to deleted profile"
            );
            if let Some(settings) = settings {
                settings.remove_profile_dns(&journal.uid)?;
            }
            // Retain any file referenced by a surviving row, including reserved defaults.
            for file in journal
                .file
                .iter()
                .chain(journal.auxiliaries.iter().filter_map(|item| item.file.as_ref()))
            {
                if !self
                    .profiles
                    .items
                    .iter()
                    .flatten()
                    .any(|item| item.file.as_deref() == Some(file.as_str()))
                    && self.deletable_file(file)?
                {
                    fs::remove_file(self.data_dir.join("profiles").join(file))?;
                }
            }
            sync_directory(&self.data_dir.join("profiles"))?;
        }
        fs::remove_file(self.data_dir.join("profile-delete.yaml"))?;
        sync_directory(&self.data_dir)
    }

    fn check_delete_catalog(&self, journal: &DeleteJournal) -> Result<()> {
        let item = self.get_item(&journal.uid)?;
        ensure!(
            matches!(item.itype.as_deref(), Some("local" | "remote"))
                && item.file.as_deref() == journal.file.as_deref(),
            "deletion journal conflicts with catalog"
        );
        if journal.schema_version == 2 {
            ensure!(
                self.delete_auxiliaries(&journal.uid)? == journal.auxiliaries,
                "deletion links changed since staging"
            );
        } else {
            ensure!(links(item).is_empty(), "legacy deletion has enhancement links");
        }
        Ok(())
    }

    fn delete_auxiliaries(&self, uid: &str) -> Result<Vec<DeletedAuxiliary>> {
        ensure!(
            !self
                .profiles
                .items
                .iter()
                .flatten()
                .any(|other| other.uid.as_deref() != Some(uid) && links(other).iter().any(|(link, _)| *link == uid)),
            "profile is referenced by an enhancement link"
        );
        let mut result: Vec<DeletedAuxiliary> = Vec::new();
        for (link, kind) in links(self.get_item(uid)?) {
            if reserved(link) {
                continue;
            }
            let Ok(aux) = self.get_item(link) else {
                continue;
            };
            ensure!(
                aux.itype.as_deref() == Some(kind) && aux.option.is_none(),
                "invalid linked auxiliary profile"
            );
            if self.profiles.current.as_deref() == Some(link)
                || self.profiles.items.iter().flatten().any(|other| {
                    other.uid.as_deref() != Some(uid) && links(other).iter().any(|(other_link, _)| *other_link == link)
                })
            {
                continue;
            }
            if !result.iter().any(|item| item.uid == link) {
                result.push(DeletedAuxiliary {
                    uid: link.into(),
                    file: aux.file.as_ref().map(ToString::to_string),
                });
            }
        }
        Ok(result)
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
            matches!(journal.schema_version, 1 | 2)
                && !journal.uid.is_empty()
                && journal.uid.len() <= 256
                && !reserved(&journal.uid),
            "invalid profile deletion journal"
        );
        if let Some(file) = &journal.file {
            validate_profile_file(file)?;
        }
        ensure!(
            journal.auxiliaries.len() <= 5 && (journal.schema_version == 2 || journal.auxiliaries.is_empty()),
            "invalid deletion auxiliary list"
        );
        let mut ids = HashSet::new();
        for aux in &journal.auxiliaries {
            ensure!(
                !aux.uid.is_empty()
                    && aux.uid.len() <= 256
                    && aux.uid != journal.uid
                    && !reserved(&aux.uid)
                    && ids.insert(&aux.uid),
                "invalid deleted auxiliary UID"
            );
            if let Some(file) = &aux.file {
                validate_profile_file(file)?;
            }
        }
        Ok(Some(journal))
    }
}
