//! Service-owned persistence using upstream profiles.yaml and PrfItem schemas.
//! The caller must hold the service data-directory lock for all reads/writes.

use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, bail, ensure};
use serde_yaml_ng::Mapping;

use super::{
    IProfiles, PrfItem, PrfSelected,
    remote::{RemoteProfile, subscription_url, validate_yaml},
    runtime::{MAX_CONFIG_BYTES, sync_directory, unique_id, write_new},
};

mod defaults;
mod delete;
mod edit;
mod merge;
mod refresh;
mod sequence;
pub use defaults::{DEFAULT_GLOBAL_MERGE, DEFAULT_GLOBAL_SCRIPT};
pub use merge::{
    EnhancementContent, EnhancementPlan, GenerationPlan, GlobalEnhancementKind, MergeContent, MergePlan, ScriptContent,
};
pub use sequence::SequenceKind;

pub use edit::{ProfilePatch, RemoteOptionsPatch};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RawContent {
    pub uid: String,
    pub revision: String,
    pub yaml: String,
}

pub struct ProfileStore {
    data_dir: PathBuf,
    profiles: IProfiles,
}

/// Retain upstream single-filename restrictions, including Windows separators.
pub fn validate_profile_file(file: &str) -> Result<()> {
    let mut components = Path::new(file).components();
    if file.is_empty()
        || file.contains('/')
        || file.contains('\\')
        || !matches!(
            (components.next(), components.next()),
            (Some(Component::Normal(_)), None)
        )
    {
        bail!("profile file must be a single filename");
    }
    Ok(())
}

impl ProfileStore {
    pub fn open(data_dir: &Path) -> Result<Self> {
        fs::create_dir_all(data_dir.join("profiles"))?;
        let path = data_dir.join("profiles.yaml");
        let profiles: IProfiles = if path.try_exists()? {
            ensure!(
                path.metadata()?.len() <= MAX_CONFIG_BYTES as u64,
                "profiles metadata exceeds 8 MiB"
            );
            serde_yaml_ng::from_str(&fs::read_to_string(path)?)?
        } else {
            IProfiles::default()
        };
        let mut ids = HashSet::new();
        for item in profiles.items.iter().flatten() {
            let uid = item.uid.as_deref().context("profile UID is missing")?;
            ensure!(!uid.is_empty() && ids.insert(uid), "profile UID is empty or duplicated");
            if let Some(file) = &item.file {
                validate_profile_file(file)?;
            }
        }
        Ok(Self {
            data_dir: data_dir.to_owned(),
            profiles,
        })
    }

    pub fn snapshot(&self) -> IProfiles {
        self.profiles.clone()
    }

    pub fn selections(&self, uid: &str) -> Result<Vec<PrfSelected>> {
        Ok(self.get_item(uid)?.selected.clone().unwrap_or_default())
    }

    pub fn save_selections(&mut self, uid: &str, selected: Vec<PrfSelected>) -> Result<()> {
        if self.selections(uid)? == selected {
            return Ok(());
        }
        let mut profiles = self.profiles.clone();
        let item = profiles
            .items
            .as_mut()
            .and_then(|items| items.iter_mut().find(|item| item.uid.as_deref() == Some(uid)))
            .context("profile is missing while saving node selections")?;
        item.selected = (!selected.is_empty()).then_some(selected);
        self.save(profiles)
    }

    /// Upstream record/update semantics, normalizing duplicate entries for this group.
    pub fn record_selection(&mut self, uid: &str, group: &str, node: &str) -> Result<()> {
        let mut selected = self.selections(uid)?;
        selected.retain(|entry| entry.name.as_deref() != Some(group));
        selected.push(PrfSelected {
            name: Some(group.into()),
            now: Some(node.into()),
        });
        self.save_selections(uid, selected)
    }

    pub fn forget_selection(&mut self, uid: &str, group: &str) -> Result<()> {
        let mut selected = self.selections(uid)?;
        selected.retain(|entry| entry.name.as_deref() != Some(group));
        self.save_selections(uid, selected)
    }

    /// Ported from IProfiles::get_item without the desktop singleton.
    pub fn get_item(&self, uid: impl AsRef<str>) -> Result<&PrfItem> {
        let uid = uid.as_ref();
        for item in self.profiles.items.iter().flatten() {
            if item.uid.as_deref() == Some(uid) {
                return Ok(item);
            }
        }
        bail!("failed to get the profile item \"uid:{uid}\"");
    }

    pub fn read_raw(&self, uid: &str) -> Result<RawContent> {
        let item = self.get_item(uid)?;
        Self::validate_base(item)?;
        Ok(RawContent {
            uid: uid.to_owned(),
            revision: item.file.as_deref().context("profile file missing")?.to_owned(),
            yaml: self.read_content(item)?,
        })
    }

    pub fn read_mapping(&self, uid: &str) -> Result<Mapping> {
        let item = self.get_item(uid)?;
        self.generate_mapping(uid, serde_yaml_ng::from_str(&self.read_content(item)?)?)
    }

    /// Pure generation is available only when both script stages are identity templates.
    /// Services must execute `generation` stages in their bounded script worker.
    pub fn generate_mapping(&self, uid: &str, base: Mapping) -> Result<Mapping> {
        self.generation(uid, base)?.static_mapping()
    }

    pub fn generation(&self, uid: &str, base: Mapping) -> Result<GenerationPlan> {
        self.generate_with(uid, base, None, None)
    }

    pub fn read_generation(&self, uid: &str) -> Result<GenerationPlan> {
        self.generation(uid, serde_yaml_ng::from_str(&self.read_content(self.get_item(uid)?)?)?)
    }

    pub fn dns_source(&self, uid: &str) -> Result<Option<String>> {
        let item = self.get_item(uid)?;
        Self::validate_base(item)?;
        let config: Mapping = serde_yaml_ng::from_str(&self.read_content(item)?)?;
        super::dns::dns_override_source(uid, &config)
    }

    fn validate_base(item: &PrfItem) -> Result<()> {
        ensure!(
            matches!(item.itype.as_deref(), Some("local" | "remote")),
            "only local or remote profiles can be selected"
        );
        Ok(())
    }

    fn read_content(&self, item: &PrfItem) -> Result<String> {
        let file = item.file.as_deref().context("profile file is missing")?;
        validate_profile_file(file)?;
        let path = self.data_dir.join("profiles").join(file);
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(metadata.is_file(), "profile file must be regular, not a symlink");
        ensure!(metadata.len() <= MAX_CONFIG_BYTES as u64, "profile exceeds 8 MiB");
        fs::read_to_string(&path).context("cannot read profile content")
    }

    /// Import raw local YAML. Activation is a separate validated service operation.
    pub fn import_local(&mut self, name: &str, yaml: &str) -> Result<PrfItem> {
        ensure!(
            !name.trim().is_empty() && name.len() <= 256,
            "profile name must be 1..256 bytes"
        );
        ensure!(yaml.len() <= MAX_CONFIG_BYTES, "profile exceeds 8 MiB");
        serde_yaml_ng::from_str::<Mapping>(yaml).context("profile must be a YAML mapping")?;
        let uid = format!("L{}", unique_id()?);
        let file = format!("{uid}.yaml");
        let item = PrfItem {
            uid: Some(uid.into()),
            itype: Some("local".into()),
            name: Some(name.into()),
            file: Some(file.into()),
            desc: Some("".into()),
            updated: Some(usize::try_from(
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            )?),
            ..PrfItem::default()
        };
        self.persist_import(item, yaml)
    }

    /// Remote content retains upstream metadata; import does not activate it.
    pub fn import_remote(&mut self, profile: RemoteProfile) -> Result<PrfItem> {
        ensure!(
            !profile.name.trim().is_empty() && profile.name.len() <= 256,
            "profile name must be 1..256 bytes"
        );
        let url = subscription_url(&profile.url)?;
        let yaml = validate_yaml(&profile.yaml)?;
        let uid = format!("R{}", unique_id()?);
        let item = PrfItem {
            file: Some(format!("{uid}.yaml").into()),
            uid: Some(uid.into()),
            itype: Some("remote".into()),
            name: Some(profile.name.into()),
            desc: None,
            url: Some(url.to_string().into()),
            extra: profile.extra,
            home: profile.home.map(Into::into),
            option: Some(profile.option),
            updated: Some(usize::try_from(
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            )?),
            ..PrfItem::default()
        };
        self.persist_import(item, yaml)
    }

    fn persist_import(&mut self, item: PrfItem, yaml: &str) -> Result<PrfItem> {
        let path = self
            .data_dir
            .join("profiles")
            .join(item.file.as_deref().context("new file missing")?);
        write_new(&path, yaml.as_bytes())?;
        if let Err(error) = sync_directory(&self.data_dir.join("profiles")) {
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        let mut profiles = self.profiles.clone();
        profiles.items.get_or_insert_default().push(item.clone());
        if let Err(error) = self.save(profiles) {
            // A failed directory sync after rename can already have committed the item.
            if self.get_item(item.uid.as_deref().context("new UID missing")?).is_err() {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
        Ok(item)
    }

    /// `current` is a compatibility mirror; the runtime journal owns activation.
    pub fn set_current(&mut self, uid: Option<&str>) -> Result<()> {
        if self.profiles.current.as_deref() == uid {
            return Ok(());
        }
        let mut profiles = self.profiles.clone();
        profiles.current = uid.map(Into::into);
        self.save(profiles)
    }

    fn save(&mut self, profiles: IProfiles) -> Result<()> {
        let yaml = format!(
            "# Profiles Config for Clash Verge\n{}",
            serde_yaml_ng::to_string(&profiles)?
        );
        ensure!(yaml.len() <= MAX_CONFIG_BYTES, "profiles metadata exceeds 8 MiB");
        let temporary = self.data_dir.join(format!("profiles-{}.tmp", unique_id()?));
        write_new(&temporary, yaml.as_bytes())?;
        if let Err(error) = fs::rename(&temporary, self.data_dir.join("profiles.yaml")) {
            let _ = fs::remove_file(temporary);
            return Err(error.into());
        }
        self.profiles = profiles;
        sync_directory(&self.data_dir)
    }
}
