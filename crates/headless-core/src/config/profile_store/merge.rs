//! Linked merge/sequence items sharing catalog/runtime coordinated recovery.
use super::*;
use crate::config::{PrfOption, runtime::Revision};
use crate::enhance::seq::use_seq;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EnhancementKind {
    #[default]
    Merge,
    Rules,
    Proxies,
    Groups,
    Script,
}

impl From<SequenceKind> for EnhancementKind {
    fn from(kind: SequenceKind) -> Self {
        match kind {
            SequenceKind::Rules => Self::Rules,
            SequenceKind::Proxies => Self::Proxies,
            SequenceKind::Groups => Self::Groups,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalEnhancementKind {
    Merge,
    Script,
}

impl GlobalEnhancementKind {
    pub fn uid(self) -> &'static str {
        EnhancementKind::from(self).reserved()
    }
    pub fn default_source(self) -> &'static str {
        match self {
            Self::Merge => DEFAULT_GLOBAL_MERGE,
            Self::Script => DEFAULT_GLOBAL_SCRIPT,
        }
    }
}

impl From<GlobalEnhancementKind> for EnhancementKind {
    fn from(kind: GlobalEnhancementKind) -> Self {
        match kind {
            GlobalEnhancementKind::Merge => Self::Merge,
            GlobalEnhancementKind::Script => Self::Script,
        }
    }
}

impl EnhancementKind {
    fn reserved(self) -> &'static str {
        match self {
            Self::Merge => "Merge",
            Self::Script => "Script",
            Self::Rules => "Rules",
            Self::Proxies => "Proxies",
            Self::Groups => "Groups",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rules => "rules",
            Self::Proxies => "proxies",
            Self::Groups => "groups",
            Self::Script => "script",
        }
    }
    fn sequence(self) -> Option<SequenceKind> {
        match self {
            Self::Merge | Self::Script => None,
            Self::Rules => Some(SequenceKind::Rules),
            Self::Proxies => Some(SequenceKind::Proxies),
            Self::Groups => Some(SequenceKind::Groups),
        }
    }
    fn validate(self, yaml: &str) -> Result<()> {
        ensure!(yaml.len() <= MAX_CONFIG_BYTES, "enhancement exceeds 8 MiB");
        if self == Self::Script {
            crate::enhance::script::validate_source(yaml)?;
        } else if let Some(kind) = self.sequence() {
            sequence::parse_sequence(yaml, kind)?;
        } else {
            serde_yaml_ng::from_str::<Mapping>(yaml).context("merge must be a YAML mapping")?;
        }
        Ok(())
    }
    fn link(self, item: &PrfItem) -> Option<&str> {
        item.option.as_ref().and_then(|option| match self {
            Self::Merge => option.merge.as_deref(),
            Self::Rules => option.rules.as_deref(),
            Self::Proxies => option.proxies.as_deref(),
            Self::Groups => option.groups.as_deref(),
            Self::Script => option.script.as_deref(),
        })
    }
    fn set_link(self, option: &mut PrfOption, uid: Option<smartstring::alias::String>) {
        match self {
            Self::Merge => option.merge = uid,
            Self::Rules => option.rules = uid,
            Self::Proxies => option.proxies = uid,
            Self::Groups => option.groups = uid,
            Self::Script => option.script = uid,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct EnhancementContent {
    pub uid: Option<String>,
    pub yaml: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ScriptContent {
    pub uid: Option<String>,
    pub source: Option<String>,
}

pub struct EnhancementPlan {
    journal: EnhancementJournal,
    yaml: Option<String>,
    pub generation: Option<GenerationPlan>,
}

/// Sequence output followed by settings, global merge/script, profile merge and
/// profile script. Script execution belongs to the service's bounded worker.
#[derive(Clone)]
pub struct GenerationPlan {
    pub profile_uid: String,
    pub dns_source: Option<String>,
    pub config: Mapping,
    pub global_merge: Mapping,
    pub global_script: Option<String>,
    pub profile_merge: Mapping,
    pub script: Option<String>,
    pub name: String,
}

impl GenerationPlan {
    pub fn static_mapping(self) -> Result<Mapping> {
        for source in [&self.global_script, &self.script].into_iter().flatten() {
            ensure!(
                source == DEFAULT_GLOBAL_SCRIPT,
                "scripted generation requires a bounded worker"
            );
        }
        let config = super::super::runtime::generate(self.config, &self.global_merge)?;
        super::super::runtime::generate(config, &self.profile_merge)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnhancementJournal {
    schema_version: u32,
    // Existing enhancement journals omitted this field. Preserve their recovery format.
    #[serde(default)]
    kind: EnhancementKind,
    #[serde(default)]
    global: bool,
    uid: String,
    previous: IProfiles,
    candidate: IProfiles,
    new_file: Option<String>,
    runtime_revision: Option<Revision>,
}

fn item<'a>(profiles: &'a IProfiles, uid: &str) -> Result<&'a PrfItem> {
    profiles
        .items
        .iter()
        .flatten()
        .find(|item| item.uid.as_deref() == Some(uid))
        .context("enhancement transaction profile is missing")
}

impl EnhancementJournal {
    fn marker<'a>(&self, profiles: &'a IProfiles) -> Result<Option<&'a str>> {
        if self.global {
            Ok(profiles
                .items
                .iter()
                .flatten()
                .find(|row| row.uid.as_deref() == Some(&self.uid))
                .and_then(|row| row.file.as_deref()))
        } else {
            Ok(self.kind.link(item(profiles, &self.uid)?))
        }
    }
}

pub type MergeContent = EnhancementContent;
pub type MergePlan = EnhancementPlan;

impl ProfileStore {
    pub fn read_global(&self, kind: GlobalEnhancementKind) -> Result<EnhancementContent> {
        Ok(EnhancementContent {
            uid: Some(kind.uid().into()),
            yaml: Some(self.reserved_enhancement(kind.into(), None)?),
        })
    }

    /// Replace only a reserved row's file pointer, preserving all base links.
    /// No active profile means no runtime generation/application is requested.
    pub fn prepare_global(
        &self,
        kind: GlobalEnhancementKind,
        source: String,
        active: Option<&str>,
    ) -> Result<EnhancementPlan> {
        let enhancement = EnhancementKind::from(kind);
        enhancement.validate(&source)?;
        if kind == GlobalEnhancementKind::Merge {
            super::super::runtime::generate(Mapping::new(), &serde_yaml_ng::from_str(&source)?)?;
        }
        let old = self
            .profiles
            .items
            .iter()
            .flatten()
            .find(|row| row.uid.as_deref() == Some(kind.uid()));
        if let Some(old) = old {
            ensure!(
                old.itype.as_deref() == Some(enhancement.name()) && old.option.is_none(),
                "invalid reserved enhancement row"
            );
            validate_profile_file(old.file.as_deref().context("global enhancement file missing")?)?;
        }
        let generation = active
            .map(|uid| {
                let base = self.get_item(uid)?;
                self.generate_with(
                    uid,
                    serde_yaml_ng::from_str(&self.read_content(base)?)?,
                    None,
                    Some((enhancement, &source)),
                )
            })
            .transpose()?;
        let file = format!(
            "{}-{}.{}",
            kind.uid(),
            unique_id()?,
            if kind == GlobalEnhancementKind::Script {
                "js"
            } else {
                "yaml"
            }
        );
        let mut row = old.cloned().unwrap_or_else(|| PrfItem {
            uid: Some(kind.uid().into()),
            itype: Some(enhancement.name().into()),
            ..Default::default()
        });
        row.file = Some(file.clone().into());
        row.updated = Some(usize::try_from(
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        )?);
        let mut candidate = self.profiles.clone();
        let items = candidate.items.get_or_insert_default();
        if let Some(existing) = items.iter_mut().find(|row| row.uid.as_deref() == Some(kind.uid())) {
            *existing = row;
        } else {
            items.push(row);
        }
        ensure!(
            serde_yaml_ng::to_string(&candidate)?.len() <= MAX_CONFIG_BYTES,
            "profiles metadata exceeds 8 MiB"
        );
        Ok(EnhancementPlan {
            generation,
            yaml: Some(source),
            journal: EnhancementJournal {
                schema_version: 1,
                kind: enhancement,
                global: true,
                uid: kind.uid().into(),
                previous: self.profiles.clone(),
                candidate,
                new_file: Some(file),
                runtime_revision: None,
            },
        })
    }
    // Compatibility entry points for the original merge-only store API.
    pub fn begin_merge(&mut self, plan: MergePlan, revision: Option<Revision>) -> Result<()> {
        self.begin_enhancement(plan, revision)
    }
    pub fn publish_merge(&mut self) -> Result<()> {
        self.publish_enhancement()
    }
    pub fn recover_merge(&mut self, committed: Option<&Revision>) -> Result<()> {
        self.recover_enhancement(committed)
    }
    pub fn read_merge(&self, uid: &str) -> Result<EnhancementContent> {
        self.read_enhancement(uid, EnhancementKind::Merge)
    }

    pub fn read_sequence(&self, uid: &str, kind: SequenceKind) -> Result<EnhancementContent> {
        self.read_enhancement(uid, kind.into())
    }

    pub fn read_script(&self, uid: &str) -> Result<ScriptContent> {
        let content = self.read_enhancement(uid, EnhancementKind::Script)?;
        Ok(ScriptContent {
            uid: content.uid,
            source: content.yaml,
        })
    }

    pub fn prepare_script(&self, uid: &str, source: Option<String>) -> Result<EnhancementPlan> {
        self.prepare_enhancement(uid, EnhancementKind::Script, source)
    }

    fn read_enhancement(&self, uid: &str, kind: EnhancementKind) -> Result<EnhancementContent> {
        let base = self.get_item(uid)?;
        Self::validate_base(base)?;
        let Some(uid) = kind.link(base) else {
            return Ok(EnhancementContent { uid: None, yaml: None });
        };
        let merge = self.get_item(uid)?;
        ensure!(
            merge.itype.as_deref() == Some(kind.name()),
            "linked item must have type {}",
            kind.name()
        );
        ensure!(
            merge.option.is_none(),
            "nested enhancement options are not supported yet"
        );
        let yaml = self.read_content(merge)?;
        kind.validate(&yaml)?;
        Ok(EnhancementContent {
            uid: Some(uid.into()),
            yaml: Some(yaml),
        })
    }

    /// Build a complete candidate without writing files or changing the active profile.
    pub fn prepare_merge(&self, uid: &str, yaml: Option<String>) -> Result<EnhancementPlan> {
        self.prepare_enhancement(uid, EnhancementKind::Merge, yaml)
    }

    pub fn prepare_sequence(&self, uid: &str, kind: SequenceKind, yaml: Option<String>) -> Result<EnhancementPlan> {
        self.prepare_enhancement(uid, kind.into(), yaml)
    }

    pub(super) fn generate_with(
        &self,
        uid: &str,
        mut base: Mapping,
        replacement: Option<(EnhancementKind, Option<&str>)>,
        global_replacement: Option<(EnhancementKind, &str)>,
    ) -> Result<GenerationPlan> {
        Self::validate_base(self.get_item(uid)?)?;
        base = super::super::runtime::normalize_profile(base);
        let dns_source = super::super::dns::dns_override_source(uid, &base)?;
        // Preserve upstream process_seq_items order, then global merge/script,
        // then profile merge/script. Missing profile links use reserved defaults.
        for kind in [
            EnhancementKind::Rules,
            EnhancementKind::Proxies,
            EnhancementKind::Groups,
        ] {
            let saved;
            let yaml = if let Some((_, yaml)) = replacement.filter(|(target, _)| *target == kind) {
                yaml
            } else {
                saved = self.effective_enhancement(uid, kind, global_replacement)?;
                Some(saved.as_str())
            };
            let sequence = kind.sequence().context("sequence kind missing")?;
            let fallback;
            let yaml = match yaml {
                Some(yaml) => yaml,
                None => {
                    fallback = self.reserved_enhancement(kind, global_replacement)?;
                    &fallback
                }
            };
            base = use_seq(sequence::parse_sequence(yaml, sequence)?, base, sequence.field());
        }
        let global_merge =
            serde_yaml_ng::from_str(&self.reserved_enhancement(EnhancementKind::Merge, global_replacement)?)?;
        let resolve = |kind| -> Result<String> {
            match replacement.filter(|(target, _)| *target == kind) {
                Some((_, Some(source))) => Ok(source.to_owned()),
                Some((_, None)) => self.reserved_enhancement(kind, global_replacement),
                None => self.effective_enhancement(uid, kind, global_replacement),
            }
        };
        let profile_merge = serde_yaml_ng::from_str(&resolve(EnhancementKind::Merge)?)?;
        // Validate both merge stages before any worker or persistence is started.
        let checked = super::super::runtime::generate(base.clone(), &global_merge)?;
        super::super::runtime::generate(checked, &profile_merge)?;
        Ok(GenerationPlan {
            profile_uid: uid.into(),
            dns_source,
            config: base,
            global_merge,
            global_script: Some(self.reserved_enhancement(EnhancementKind::Script, global_replacement)?),
            profile_merge,
            script: Some(resolve(EnhancementKind::Script).context("invalid script enhancement links")?),
            name: self.get_item(uid)?.name.as_deref().unwrap_or_default().to_owned(),
        })
    }

    fn reserved_enhancement(
        &self,
        kind: EnhancementKind,
        replacement: Option<(EnhancementKind, &str)>,
    ) -> Result<String> {
        if let Some((_, source)) = replacement.filter(|(target, _)| *target == kind) {
            return Ok(source.into());
        }
        if let Ok(item) = self.get_item(kind.reserved()) {
            ensure!(
                item.itype.as_deref() == Some(kind.name()),
                "reserved enhancement has the wrong type"
            );
            ensure!(
                item.option.is_none(),
                "nested enhancement options are not supported yet"
            );
            let source = self.read_content(item)?;
            kind.validate(&source)?;
            return Ok(source);
        }
        Ok(match kind {
            EnhancementKind::Merge => DEFAULT_GLOBAL_MERGE,
            EnhancementKind::Script => DEFAULT_GLOBAL_SCRIPT,
            _ => "prepend: []\nappend: []\ndelete: []\n",
        }
        .into())
    }

    fn effective_enhancement(
        &self,
        uid: &str,
        kind: EnhancementKind,
        replacement: Option<(EnhancementKind, &str)>,
    ) -> Result<String> {
        if kind.link(self.get_item(uid)?) == Some(kind.reserved()) {
            return self.reserved_enhancement(kind, replacement);
        }
        match self.read_enhancement(uid, kind)?.yaml {
            Some(source) => Ok(source),
            None => self.reserved_enhancement(kind, replacement),
        }
    }

    fn prepare_enhancement(&self, uid: &str, kind: EnhancementKind, yaml: Option<String>) -> Result<EnhancementPlan> {
        let base = self.get_item(uid)?;
        Self::validate_base(base)?;
        // Check existing link types/content before replacing or detaching them.
        self.read_enhancement(uid, kind)?;
        let raw: Mapping = serde_yaml_ng::from_str(&self.read_content(base)?)?;
        if let Some(yaml) = &yaml {
            kind.validate(yaml)?;
        }
        let generation = self.generate_with(uid, raw, Some((kind, yaml.as_deref())), None)?;
        let old_link = kind.link(base).map(ToOwned::to_owned);
        let mut candidate = self.profiles.clone();
        let new_item = if yaml.is_some() {
            let uid = format!("{}{}", &kind.name()[..1], unique_id()?);
            Some(PrfItem {
                file: Some(format!("{uid}.{}", if kind == EnhancementKind::Script { "js" } else { "yaml" }).into()),
                uid: Some(uid.into()),
                itype: Some(kind.name().into()),
                updated: Some(usize::try_from(
                    SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                )?),
                ..Default::default()
            })
        } else {
            None
        };
        let new_file = new_item
            .as_ref()
            .and_then(|item| item.file.as_ref())
            .map(ToString::to_string);
        let items = candidate.items.as_mut().context("profile catalog is empty")?;
        let base = items.iter_mut().find(|item| item.uid.as_deref() == Some(uid)).unwrap();
        if yaml.is_some() || base.option.is_some() {
            kind.set_link(
                base.option.get_or_insert_with(PrfOption::default),
                new_item.as_ref().and_then(|item| item.uid.clone()),
            );
        }
        if let Some(merge) = new_item {
            items.push(merge);
        }
        if let Some(old) = &old_link {
            let referenced = items.iter().any(|item| {
                item.option.as_ref().is_some_and(|option| {
                    [
                        &option.merge,
                        &option.script,
                        &option.rules,
                        &option.proxies,
                        &option.groups,
                    ]
                    .iter()
                    .any(|uid| uid.as_deref() == Some(old.as_str()))
                })
            });
            // Preserve reserved upstream defaults for the later global pipeline.
            if !referenced && !["Merge", "Rules", "Proxies", "Groups", "Script"].contains(&old.as_str()) {
                items.retain(|item| item.uid.as_deref() != Some(old.as_str()));
            }
        }
        ensure!(
            serde_yaml_ng::to_string(&candidate)?.len() <= MAX_CONFIG_BYTES,
            "profiles metadata exceeds 8 MiB"
        );
        Ok(EnhancementPlan {
            generation: Some(generation),
            yaml,
            journal: EnhancementJournal {
                schema_version: 1,
                kind,
                global: false,
                uid: uid.into(),
                previous: self.profiles.clone(),
                candidate,
                new_file,
                runtime_revision: None,
            },
        })
    }

    pub fn begin_enhancement(&mut self, mut plan: EnhancementPlan, revision: Option<Revision>) -> Result<()> {
        ensure!(
            !self.data_dir.join("profile-merge.yaml").try_exists()?
                && !self.data_dir.join("profile-refresh.yaml").try_exists()?
                && !self.data_dir.join("backup-restore.yaml").try_exists()?
                && !self.data_dir.join("profile-import.yaml").try_exists()?
                && !self.data_dir.join("profile-delete.yaml").try_exists()?,
            "profile recovery is pending"
        );
        ensure!(
            serde_yaml_ng::to_value(&self.profiles)? == serde_yaml_ng::to_value(&plan.journal.previous)?,
            "profile catalog changed before enhancement preparation"
        );
        ensure!(
            plan.journal.marker(&plan.journal.previous)? != plan.journal.marker(&plan.journal.candidate)?,
            "enhancement link is already absent"
        );
        plan.journal.runtime_revision = revision;
        let path = self.data_dir.join("profile-merge.yaml");
        let temporary = self.data_dir.join(format!("profile-merge-{}.tmp", unique_id()?));
        let content = plan
            .journal
            .new_file
            .as_ref()
            .map(|file| self.data_dir.join("profiles").join(file));
        let result = (|| {
            if let (Some(path), Some(yaml)) = (&content, &plan.yaml) {
                write_new(path, yaml.as_bytes())?;
                sync_directory(&self.data_dir.join("profiles"))?;
            }
            let bytes = serde_yaml_ng::to_string(&plan.journal)?;
            ensure!(
                bytes.len() <= 2 * MAX_CONFIG_BYTES + 4096,
                "enhancement journal exceeds its size limit"
            );
            write_new(&temporary, bytes.as_bytes())?;
            fs::rename(&temporary, &path)?;
            sync_directory(&self.data_dir)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
            if !path.try_exists()?
                && let Some(content) = content
            {
                let _ = fs::remove_file(content);
            }
        }
        result
    }

    pub fn publish_enhancement(&mut self) -> Result<()> {
        let journal = self
            .enhancement_journal()?
            .context("no pending enhancement transaction")?;
        self.save(journal.candidate)
    }

    pub fn recover_enhancement(&mut self, committed: Option<&Revision>) -> Result<()> {
        let Some(journal) = self.enhancement_journal()? else {
            return Ok(());
        };
        let previous_link = journal.marker(&journal.previous)?;
        let candidate_link = journal.marker(&journal.candidate)?;
        let current_link = journal.marker(&self.profiles)?;
        ensure!(
            current_link == previous_link || current_link == candidate_link,
            "enhancement journal conflicts with catalog"
        );
        let accepted = match &journal.runtime_revision {
            Some(revision) => committed == Some(revision),
            None => current_link == candidate_link,
        };
        let desired = if accepted { candidate_link } else { previous_link };
        if current_link != desired {
            self.save(if accepted { journal.candidate } else { journal.previous })?;
        }
        fs::remove_file(self.data_dir.join("profile-merge.yaml"))?;
        sync_directory(&self.data_dir)
    }

    fn enhancement_journal(&self) -> Result<Option<EnhancementJournal>> {
        let path = self.data_dir.join("profile-merge.yaml");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.is_file() && metadata.len() <= (2 * MAX_CONFIG_BYTES + 4096) as u64,
            "unsafe enhancement journal"
        );
        let journal: EnhancementJournal = serde_yaml_ng::from_str(&fs::read_to_string(path)?)?;
        ensure!(journal.schema_version == 1, "unsupported enhancement journal version");
        for catalog in [&journal.previous, &journal.candidate] {
            let mut ids = HashSet::new();
            for row in catalog.items.iter().flatten() {
                let uid = row.uid.as_deref().context("enhancement journal UID missing")?;
                ensure!(!uid.is_empty() && ids.insert(uid), "duplicate enhancement journal UID");
                if let Some(file) = &row.file {
                    validate_profile_file(file)?;
                }
            }
            if !journal.global {
                Self::validate_base(item(catalog, &journal.uid)?)?;
            }
        }
        if journal.global {
            ensure!(
                matches!(journal.kind, EnhancementKind::Merge | EnhancementKind::Script)
                    && journal.uid == journal.kind.reserved(),
                "invalid global enhancement target"
            );
            let new = item(&journal.candidate, &journal.uid)?;
            ensure!(
                new.itype.as_deref() == Some(journal.kind.name())
                    && new.option.is_none()
                    && new.file.is_some()
                    && new.file.as_deref() == journal.new_file.as_deref(),
                "invalid global enhancement item"
            );
            ensure!(
                journal.marker(&journal.previous)? != journal.marker(&journal.candidate)?,
                "unchanged global enhancement file"
            );
            if let Ok(old) = item(&journal.previous, &journal.uid) {
                ensure!(
                    old.itype.as_deref() == Some(journal.kind.name()) && old.option.is_none() && old.file.is_some(),
                    "invalid previous global enhancement item"
                );
            }
            let mut previous = journal.previous.clone();
            let mut candidate = journal.candidate.clone();
            for catalog in [&mut previous, &mut candidate] {
                // None and an empty items list are equivalent when adding the first row.
                let rows = catalog.items.get_or_insert_default();
                rows.retain(|row| row.uid.as_deref() != Some(&journal.uid));
            }
            ensure!(
                serde_yaml_ng::to_value(previous)? == serde_yaml_ng::to_value(candidate)?,
                "global journal changes unrelated catalog rows"
            );
            let source = self.read_content(new)?;
            journal.kind.validate(&source)?;
            if journal.kind == EnhancementKind::Merge {
                super::super::runtime::generate(Mapping::new(), &serde_yaml_ng::from_str(&source)?)?;
            }
            return Ok(Some(journal));
        }
        let old = item(&journal.previous, &journal.uid)?;
        let new = item(&journal.candidate, &journal.uid)?;
        ensure!(
            old.file == new.file && old.url == new.url && journal.kind.link(old) != journal.kind.link(new),
            "invalid enhancement journal"
        );
        if let Some(uid) = journal.kind.link(new) {
            let merge = item(&journal.candidate, uid)?;
            ensure!(
                merge.itype.as_deref() == Some(journal.kind.name())
                    && merge.file.as_deref() == journal.new_file.as_deref(),
                "invalid enhancement item"
            );
            ensure!(
                merge.option.is_none(),
                "nested enhancement options are not supported yet"
            );
            journal.kind.validate(&self.read_content(merge)?)?;
        } else {
            ensure!(journal.new_file.is_none(), "detached enhancement has a content file");
        }
        Ok(Some(journal))
    }
}
