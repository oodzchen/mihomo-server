//! New service imports create missing upstream auxiliaries in one catalog commit.
use super::super::PrfOption;
use super::*;
use serde::{Deserialize, Serialize};

const JOURNAL: &str = "profile-import.yaml";
const EMPTY_MERGE: &str = "# Profile Enhancement Merge Template for Clash Verge\n\n";
const RULES: &str = "# Profile Enhancement Rules Template for Clash Verge\n\nprepend: []\n\nappend: []\n\ndelete: []\n";
const PROXIES: &str =
    "# Profile Enhancement Proxies Template for Clash Verge\n\nprepend: []\n\nappend: []\n\ndelete: []\n";
const GROUPS: &str =
    "# Profile Enhancement Groups Template for Clash Verge\n\nprepend: []\n\nappend: []\n\ndelete: []\n";

fn template(kind: &str) -> Result<&'static str> {
    Ok(match kind {
        "merge" => EMPTY_MERGE,
        "script" => DEFAULT_GLOBAL_SCRIPT,
        "rules" => RULES,
        "proxies" => PROXIES,
        "groups" => GROUPS,
        _ => bail!("invalid import auxiliary kind"),
    })
}
fn hash(source: &str) -> String {
    super::sha256_hex(source.as_bytes())
}
fn filename(transaction: &str, kind: &str) -> String {
    format!(
        "import-{transaction}-{kind}.{}",
        if kind == "script" { "js" } else { "yaml" }
    )
}
fn link<'a>(option: &'a PrfOption, kind: &str) -> Option<&'a str> {
    match kind {
        "merge" => option.merge.as_deref(),
        "script" => option.script.as_deref(),
        "rules" => option.rules.as_deref(),
        "proxies" => option.proxies.as_deref(),
        "groups" => option.groups.as_deref(),
        _ => None,
    }
}
fn set_link(option: &mut PrfOption, kind: &str, uid: &str) {
    let target = match kind {
        "merge" => &mut option.merge,
        "script" => &mut option.script,
        "rules" => &mut option.rules,
        "proxies" => &mut option.proxies,
        "groups" => &mut option.groups,
        _ => unreachable!(),
    };
    *target = Some(uid.into());
}
const KINDS: [&str; 5] = ["merge", "script", "rules", "proxies", "groups"];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportJournal {
    schema_version: u32,
    transaction: String,
    items: Vec<PrfItem>,
    hashes: Vec<String>,
    reused: Vec<ReusedLink>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReusedLink {
    uid: String,
    kind: String,
    file: String,
    hash: String,
}

/// Opaque plan; content and allocated filenames cannot be edited by callers.
pub struct ImportPlan {
    journal: ImportJournal,
    sources: Vec<String>,
}
impl ImportPlan {
    pub fn profile(&self) -> &PrfItem {
        &self.journal.items[0]
    }
}

impl ProfileStore {
    /// Service workflow. Low-level import_local remains available for legacy catalog migration.
    pub fn import_local_with_defaults(&mut self, name: &str, yaml: &str) -> Result<PrfItem> {
        let plan = self.prepare_local_import(name, yaml)?;
        self.commit_import(plan)
    }
    pub fn import_remote_with_defaults(&mut self, remote: RemoteProfile) -> Result<PrfItem> {
        let plan = self.prepare_remote_import(remote)?;
        self.commit_import(plan)
    }
    pub fn prepare_local_import(&self, name: &str, yaml: &str) -> Result<ImportPlan> {
        self.prepare_import(Self::local_item(name, yaml)?, yaml)
    }
    pub fn prepare_remote_import(&self, remote: RemoteProfile) -> Result<ImportPlan> {
        let yaml = validate_yaml(&remote.yaml)?.to_owned();
        self.prepare_import(Self::remote_item(remote)?, &yaml)
    }
    fn prepare_import(&self, mut base: PrfItem, yaml: &str) -> Result<ImportPlan> {
        self.ensure_import_admission()?;
        let transaction = unique_id()?;
        let prefix = if base.itype.as_deref() == Some("local") {
            "L"
        } else {
            "R"
        };
        base.uid = Some(format!("{prefix}{transaction}").into());
        base.file = Some(filename(&transaction, "raw").into());
        let mut option = base.option.take().unwrap_or_default();
        let mut sources = vec![yaml.to_owned()];
        let mut auxiliaries = Vec::new();
        let mut reused = Vec::new();
        for kind in KINDS {
            if let Some(uid) = link(&option, kind) {
                let item = self.get_item(uid)?;
                ensure!(
                    item.itype.as_deref() == Some(kind) && item.option.is_none(),
                    "invalid reused import auxiliary"
                );
                let source = self.read_content(item)?;
                reused.push(ReusedLink {
                    uid: uid.into(),
                    kind: kind.into(),
                    file: item.file.as_deref().context("auxiliary file missing")?.into(),
                    hash: hash(&source),
                });
            } else {
                let uid = format!("{}{transaction}", &kind[..1]);
                auxiliaries.push(PrfItem {
                    uid: Some(uid.clone().into()),
                    itype: Some(kind.into()),
                    file: Some(filename(&transaction, kind).into()),
                    updated: base.updated,
                    ..Default::default()
                });
                set_link(&mut option, kind, &uid);
                sources.push(template(kind)?.into());
            }
        }
        base.option = Some(option);
        let mut items = vec![base];
        items.extend(auxiliaries);
        Ok(ImportPlan {
            journal: ImportJournal {
                schema_version: 1,
                transaction,
                items,
                hashes: sources.iter().map(|source| hash(source)).collect(),
                reused,
            },
            sources,
        })
    }
    fn commit_import(&mut self, plan: ImportPlan) -> Result<PrfItem> {
        let item = plan.profile().clone();
        let result = self.begin_import(plan).and_then(|()| self.publish_import());
        let recovery = self.recover_import();
        result.map_err(|error| error.context(format!("profile import recovery: {recovery:?}")))?;
        recovery.context("profile imported; journal cleanup is pending")?;
        Ok(item)
    }

    pub(super) fn ensure_import_admission(&self) -> Result<()> {
        for name in [
            JOURNAL,
            "profile-refresh.yaml",
            "profile-merge.yaml",
            "profile-delete.yaml",
            "backup-restore.yaml",
        ] {
            ensure!(!self.data_dir.join(name).try_exists()?, "profile recovery is pending");
        }
        Ok(())
    }

    /// Persist intent before any content file. Caller holds the service data lock.
    pub fn begin_import(&mut self, plan: ImportPlan) -> Result<()> {
        self.ensure_import_admission()?;
        self.check_import_links(&plan.journal)?;
        for item in &plan.journal.items {
            ensure!(
                self.get_item(item.uid.as_deref().context("new UID missing")?).is_err(),
                "import UID already exists"
            );
            let path = self
                .data_dir
                .join("profiles")
                .join(item.file.as_deref().context("new file missing")?);
            match fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => bail!("import filename already exists or cannot be inspected"),
            }
        }
        let yaml = serde_yaml_ng::to_string(&plan.journal)?;
        ensure!(yaml.len() <= 64 * 1024, "import metadata exceeds 64 KiB");
        let temporary = self.data_dir.join(format!("profile-import-{}.tmp", unique_id()?));
        let result = (|| {
            write_new(&temporary, yaml.as_bytes())?;
            fs::rename(&temporary, self.data_dir.join(JOURNAL))?;
            sync_directory(&self.data_dir)?;
            for (item, source) in plan.journal.items.iter().zip(&plan.sources) {
                write_new(
                    &self
                        .data_dir
                        .join("profiles")
                        .join(item.file.as_deref().context("new file missing")?),
                    source.as_bytes(),
                )?;
            }
            sync_directory(&self.data_dir.join("profiles"))
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
    pub fn publish_import(&mut self) -> Result<()> {
        let journal = self.import_journal()?.context("no pending profile import")?;
        self.check_import_links(&journal)?;
        self.check_import_files(&journal)?;
        ensure!(
            journal
                .items
                .iter()
                .all(|item| self.get_item(item.uid.as_deref().unwrap()).is_err()),
            "import catalog already contains allocated UID"
        );
        let mut catalog = self.profiles.clone();
        catalog.items.get_or_insert_default().extend(journal.items);
        self.save(catalog)
    }
    pub fn recover_import(&mut self) -> Result<()> {
        let Some(journal) = self.import_journal()? else {
            return Ok(());
        };
        let base_uid = journal.items[0].uid.as_deref().unwrap();
        if self.get_item(base_uid).is_ok() {
            for item in &journal.items {
                ensure!(
                    serde_json::to_value(self.get_item(item.uid.as_deref().unwrap())?)? == serde_json::to_value(item)?,
                    "import journal conflicts with committed catalog"
                );
            }
            self.check_import_links(&journal)?;
            self.check_import_files(&journal)?;
        } else {
            ensure!(
                self.profiles.current.as_deref() != Some(base_uid),
                "uncommitted import is current"
            );
            for item in &journal.items {
                ensure!(
                    self.get_item(item.uid.as_deref().unwrap()).is_err(),
                    "partial import catalog commit"
                );
                let file = item.file.as_deref().unwrap();
                ensure!(
                    !self
                        .profiles
                        .items
                        .iter()
                        .flatten()
                        .any(|row| row.file.as_deref() == Some(file)),
                    "uncommitted import file referenced by catalog"
                );
            }
            for item in &journal.items {
                let path = self.data_dir.join("profiles").join(item.file.as_deref().unwrap());
                match fs::symlink_metadata(&path) {
                    Ok(metadata) => {
                        ensure!(metadata.is_file(), "import cleanup requires regular file");
                        fs::remove_file(path)?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            sync_directory(&self.data_dir.join("profiles"))?;
        }
        fs::remove_file(self.data_dir.join(JOURNAL))?;
        sync_directory(&self.data_dir)
    }
    fn check_import_links(&self, journal: &ImportJournal) -> Result<()> {
        for saved in &journal.reused {
            let item = self.get_item(&saved.uid)?;
            ensure!(
                item.itype.as_deref() == Some(saved.kind.as_str())
                    && item.option.is_none()
                    && item.file.as_deref() == Some(saved.file.as_str())
                    && hash(&self.read_content(item)?) == saved.hash,
                "reused import auxiliary changed"
            );
        }
        Ok(())
    }
    fn check_import_files(&self, journal: &ImportJournal) -> Result<()> {
        for (item, expected) in journal.items.iter().zip(&journal.hashes) {
            let source = self.read_content(item)?;
            ensure!(
                hash(&source) == *expected,
                "import content changed before catalog commit"
            );
            if item
                .itype
                .as_deref()
                .is_some_and(|kind| matches!(kind, "local" | "remote"))
            {
                serde_yaml_ng::from_str::<Mapping>(&source).context("invalid imported YAML mapping")?;
            }
        }
        Ok(())
    }
    fn import_journal(&self) -> Result<Option<ImportJournal>> {
        let path = self.data_dir.join(JOURNAL);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.is_file() && metadata.len() <= 64 * 1024,
            "unsafe import journal"
        );
        let journal: ImportJournal = serde_yaml_ng::from_str(&fs::read_to_string(path)?)?;
        let segments: Vec<_> = journal.transaction.split('-').collect();
        ensure!(
            journal.schema_version == 1
                && journal.transaction.len() <= 128
                && segments.len() == 3
                && segments
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_hexdigit())),
            "invalid import transaction"
        );
        ensure!(
            (1..=6).contains(&journal.items.len())
                && journal.hashes.len() == journal.items.len()
                && journal.reused.len() <= 5
                && journal.items.len() + journal.reused.len() == 6,
            "invalid import item count"
        );
        let base = &journal.items[0];
        let kind = base.itype.as_deref().context("import base type missing")?;
        ensure!(matches!(kind, "local" | "remote"), "invalid import base type");
        let prefix = if kind == "local" { "L" } else { "R" };
        ensure!(
            base.uid.as_deref() == Some(format!("{prefix}{}", journal.transaction).as_str())
                && base.file.as_deref() == Some(filename(&journal.transaction, "raw").as_str())
                && base.selected.is_none()
                && base
                    .name
                    .as_deref()
                    .is_some_and(|name| !name.trim().is_empty() && name.len() <= 256),
            "invalid imported base"
        );
        if kind == "remote" {
            subscription_url(base.url.as_deref().context("remote URL missing")?)?;
        }
        let option = base.option.as_ref().context("import option missing")?;
        let mut kinds = HashSet::new();
        for (item, expected) in journal.items.iter().zip(&journal.hashes).skip(1) {
            let kind = item.itype.as_deref().context("import auxiliary type missing")?;
            ensure!(
                KINDS.contains(&kind) && kinds.insert(kind),
                "invalid import auxiliary type"
            );
            let uid = format!("{}{}", &kind[..1], journal.transaction);
            let desired = PrfItem {
                uid: Some(uid.clone().into()),
                itype: Some(kind.into()),
                file: Some(filename(&journal.transaction, kind).into()),
                updated: base.updated,
                ..Default::default()
            };
            ensure!(
                serde_json::to_value(item)? == serde_json::to_value(desired)?
                    && link(option, kind) == Some(uid.as_str())
                    && *expected == hash(template(kind)?),
                "invalid imported auxiliary row"
            );
        }
        for saved in &journal.reused {
            ensure!(
                KINDS.contains(&saved.kind.as_str())
                    && kinds.insert(&saved.kind)
                    && !saved.uid.is_empty()
                    && saved.uid.len() <= 256
                    && link(option, &saved.kind) == Some(saved.uid.as_str()),
                "invalid reused import link"
            );
            validate_profile_file(&saved.file)?;
        }
        ensure!(
            kinds.len() == 5
                && journal
                    .hashes
                    .iter()
                    .chain(journal.reused.iter().map(|saved| &saved.hash))
                    .all(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())),
            "invalid import hashes or links"
        );
        Ok(Some(journal))
    }
}
