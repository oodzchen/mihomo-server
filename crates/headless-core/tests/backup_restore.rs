#![cfg(unix)]
use anyhow::Result;
use headless_core::config::{
    profile_store::ProfileStore,
    runtime::{RuntimeStore, parse},
    settings::{ServiceSettings, SettingsStore},
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt as _, symlink},
    path::PathBuf,
};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-restore-journal-{}-{stamp:x}", std::process::id()));
        fs::create_dir(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Fixture {
    live: Directory,
    _candidate: Directory,
    profiles: ProfileStore,
    settings: SettingsStore,
    runtime: RuntimeStore,
    candidate: ProfileStore,
    target: ServiceSettings,
    old_uid: String,
    new_uid: String,
}
impl Fixture {
    fn new() -> Result<Self> {
        let live = Directory::new()?;
        let candidate_dir = Directory::new()?;
        let mut profiles = ProfileStore::open(&live.0)?;
        let mut candidate = ProfileStore::open(&candidate_dir.0)?;
        profiles.ensure_global_defaults()?;
        candidate.ensure_global_defaults()?;
        let old = profiles.import_local_with_defaults("old", "# old\nmode: direct\n")?;
        let new = candidate.import_local_with_defaults("new", "# exact archived bytes\r\nmode: global\r\n")?;
        let old_uid = old.uid.unwrap().to_string();
        let new_uid = new.uid.unwrap().to_string();
        profiles.set_current(Some(&old_uid))?;
        candidate.set_current(Some(&new_uid))?;
        let mut runtime = RuntimeStore::open(&live.0)?;
        let old_revision = runtime.stage(parse("mode: direct\n")?)?;
        runtime.begin_profile(old_revision, Some(old_uid.clone()))?;
        runtime.commit()?;
        let settings = SettingsStore::open(&live.0)?;
        let mut target = ServiceSettings::default();
        target.runtime.mixed_port = Some(12345);
        Ok(Self {
            live,
            _candidate: candidate_dir,
            profiles,
            settings,
            runtime,
            candidate,
            target,
            old_uid,
            new_uid,
        })
    }
    fn begin(&mut self) -> Result<()> {
        let revision = self.runtime.stage(parse("mode: global\nmixed-port: 12345\n")?)?;
        let plan = self.profiles.prepare_restore(
            &self.candidate,
            &self.settings,
            self.target.clone(),
            &self.runtime,
            revision.clone(),
        )?;
        assert_eq!(plan.active_profile(), Some(self.new_uid.as_str()));
        self.runtime.begin_profile(revision, Some(self.new_uid.clone()))?;
        self.profiles.begin_restore(plan, &self.settings, &self.runtime)
    }
    fn journal(&self) -> Result<serde_yaml_ng::Value> {
        Ok(serde_yaml_ng::from_slice(&fs::read(
            self.live.0.join("backup-restore.yaml"),
        )?)?)
    }
    fn reopen_recover(&mut self) -> Result<()> {
        self.runtime = RuntimeStore::open(&self.live.0)?;
        self.profiles = ProfileStore::open(&self.live.0)?;
        self.settings = SettingsStore::open(&self.live.0)?;
        self.profiles.recover_restore(&mut self.settings, &self.runtime)?;
        self.profiles.recover_restore(&mut self.settings, &self.runtime)
    }
    fn allocations(&self) -> Result<Vec<PathBuf>> {
        Ok(self.journal()?["files"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|f| self.live.0.join("profiles").join(f["file"].as_str().unwrap()))
            .collect())
    }
}
#[test]
fn restart_rolls_back_every_precommit_publication_boundary() -> Result<()> {
    for phase in 0..5 {
        let mut f = Fixture::new()?;
        let previous = serde_json::to_value(f.profiles.snapshot())?;
        let original = f.profiles.read_raw(&f.old_uid)?;
        let old_bytes = fs::read(f.live.0.join("profiles").join(&original.revision))?;
        f.begin()?;
        let files = f.allocations()?;
        let j = f.journal()?;
        match phase {
            0 => {
                // crash while writing the first source, before rename
                for path in &files {
                    fs::remove_file(path)?;
                }
                let part = files[0].with_extension("part");
                fs::write(&part, b"partial")?;
                fs::set_permissions(part, fs::Permissions::from_mode(0o600))?;
            }
            1 => {} // durable files, no pointer publication
            2 => {
                fs::write(
                    f.live.0.join("profiles.yaml"),
                    serde_yaml_ng::to_string(&j["candidate"])?,
                )?;
            }
            3 => {
                f.settings.replace(f.target.clone())?;
            } // settings-only interruption
            4 => f.profiles.publish_restore(&mut f.settings, &f.runtime)?,
            _ => unreachable!(),
        }
        f.reopen_recover()?;
        assert_eq!(serde_json::to_value(f.profiles.snapshot())?, previous);
        assert_eq!(f.settings.snapshot(), ServiceSettings::default());
        assert_eq!(f.runtime.state().active_profile.as_deref(), Some(f.old_uid.as_str()));
        assert!(f.runtime.state().pending.is_none());
        assert!(files.iter().all(|p| !p.exists() && !p.with_extension("part").exists()));
        assert_eq!(fs::read(f.live.0.join("profiles").join(original.revision))?, old_bytes);
        assert!(!f.live.0.join("backup-restore.yaml").exists());
    }
    Ok(())
}
#[test]
fn committed_restore_rolls_forward_and_preserves_sources_links_selections() -> Result<()> {
    let mut f = Fixture::new()?;
    f.candidate.record_selection(&f.new_uid, "Main", "DIRECT")?;
    f.begin()?;
    f.profiles.publish_restore(&mut f.settings, &f.runtime)?;
    f.profiles.publish_restore(&mut f.settings, &f.runtime)?;
    let new = f.profiles.read_raw(&f.new_uid)?;
    assert!(new.revision.starts_with("restore-"));
    assert_eq!(new.yaml, "# exact archived bytes\r\nmode: global\r\n");
    assert_eq!(f.profiles.selections(&f.new_uid)?.len(), 1);
    assert_eq!(f.profiles.read_mapping(&f.new_uid)?["mode"].as_str(), Some("global"));
    f.runtime.commit()?;
    // Simulate pointer cleanup interrupted after a committed manifest.
    let j = f.journal()?;
    fs::write(
        f.live.0.join("profiles.yaml"),
        serde_yaml_ng::to_string(&j["previous"])?,
    )?;
    f.settings.replace(ServiceSettings::default())?;
    f.reopen_recover()?;
    assert_eq!(f.profiles.read_raw(&f.new_uid)?.yaml, new.yaml);
    assert_eq!(f.settings.snapshot(), f.target);
    assert_eq!(f.runtime.state().active_profile.as_deref(), Some(f.new_uid.as_str()));
    assert!(f.profiles.get_item(&f.old_uid).is_err());
    assert!(!f.live.0.join("backup-restore.yaml").exists());
    Ok(())
}
#[test]
fn altered_restored_sources_fail_closed_before_any_recovery_write() -> Result<()> {
    for committed in [false, true] {
        let mut f = Fixture::new()?;
        f.begin()?;
        f.profiles.publish_restore(&mut f.settings, &f.runtime)?;
        if committed {
            f.runtime.commit()?;
        } else {
            f.runtime.abort()?;
        }
        let path = f.allocations()?[0].clone();
        fs::write(&path, b"changed")?;
        let catalog = fs::read(f.live.0.join("profiles.yaml"))?;
        let settings = fs::read(f.live.0.join("settings.yaml"))?;
        assert!(f.profiles.recover_restore(&mut f.settings, &f.runtime).is_err());
        assert_eq!(fs::read(f.live.0.join("profiles.yaml"))?, catalog);
        assert_eq!(fs::read(f.live.0.join("settings.yaml"))?, settings);
        assert_eq!(fs::read(&path)?, b"changed");
        assert!(f.live.0.join("backup-restore.yaml").exists());
    }
    Ok(())
}
#[test]
fn journal_paths_symlinks_hardlinks_modes_and_unbounded_sizes_fail_closed() -> Result<()> {
    for attack in 0..7 {
        let mut f = Fixture::new()?;
        f.begin()?;
        f.runtime.abort()?;
        let journal = f.live.0.join("backup-restore.yaml");
        let path = f.allocations()?[0].clone();
        let before = fs::read(f.live.0.join("profiles.yaml"))?;
        match attack {
            0 => {
                let mut j = f.journal()?;
                j["files"][0]["file"] = "../../victim".into();
                fs::write(&journal, serde_yaml_ng::to_string(&j)?)?;
            }
            1 => {
                let mut j = f.journal()?;
                j["files"][0]["bytes"] = u64::MAX.into();
                fs::write(&journal, serde_yaml_ng::to_string(&j)?)?;
            }
            2 => {
                fs::remove_file(&path)?;
                symlink(f.live.0.join("settings.yaml"), &path)?;
            }
            3 => {
                fs::hard_link(&path, f.live.0.join("alias"))?;
            }
            4 => {
                fs::set_permissions(&journal, fs::Permissions::from_mode(0o644))?;
            }
            5 => {
                fs::remove_file(&journal)?;
                symlink(f.live.0.join("settings.yaml"), &journal)?;
            }
            6 => {
                let mut j = f.journal()?;
                j["schema_version"] = 2.into();
                fs::write(&journal, serde_yaml_ng::to_string(&j)?)?;
            }
            _ => unreachable!(),
        }
        assert!(f.profiles.recover_restore(&mut f.settings, &f.runtime).is_err());
        assert_eq!(fs::read(f.live.0.join("profiles.yaml"))?, before);
        assert!(fs::symlink_metadata(&journal).is_ok());
    }
    Ok(())
}
#[test]
fn stale_plans_store_root_mismatches_and_other_transactions_are_rejected() -> Result<()> {
    let mut f = Fixture::new()?;
    let revision = f.runtime.stage(parse("mode: global")?)?;
    let plan = f.profiles.prepare_restore(
        &f.candidate,
        &f.settings,
        f.target.clone(),
        &f.runtime,
        revision.clone(),
    )?;
    f.profiles.record_selection(&f.old_uid, "Main", "REJECT")?;
    f.runtime.begin_profile(revision, Some(f.new_uid.clone()))?;
    assert!(f.profiles.begin_restore(plan, &f.settings, &f.runtime).is_err());
    assert!(!f.live.0.join("backup-restore.yaml").exists());
    f.runtime.abort()?;
    let other_runtime = RuntimeStore::open(&f._candidate.0)?;
    let other_revision = other_runtime.stage(parse("mode: global")?)?;
    assert!(
        f.profiles
            .prepare_restore(
                &f.candidate,
                &f.settings,
                f.target.clone(),
                &other_runtime,
                other_revision
            )
            .is_err()
    );
    fs::write(f.live.0.join("profile-import.yaml"), "pending")?;
    let revision = f.runtime.stage(parse("mode: global")?)?;
    assert!(
        f.profiles
            .prepare_restore(&f.candidate, &f.settings, f.target.clone(), &f.runtime, revision)
            .is_err()
    );
    Ok(())
}
#[test]
fn conflicting_disk_catalog_settings_or_runtime_retain_intent() -> Result<()> {
    for change in 0..3 {
        let mut f = Fixture::new()?;
        f.begin()?;
        f.runtime.abort()?;
        match change {
            0 => {
                let mut c = f.profiles.snapshot();
                c.items.as_mut().unwrap()[0].name = Some("unexpected".into());
                fs::write(f.live.0.join("profiles.yaml"), serde_yaml_ng::to_string(&c)?)?;
                f.profiles = ProfileStore::open(&f.live.0)?;
            }
            1 => {
                let mut s = f.settings.snapshot();
                s.runtime.port = Some(23456);
                f.settings.replace(s)?;
            }
            2 => {
                let r = f.runtime.stage(parse("mode: rule")?)?;
                f.runtime.begin_profile(r, Some(f.old_uid.clone()))?;
                f.runtime.commit()?;
            }
            _ => unreachable!(),
        }
        assert!(f.profiles.recover_restore(&mut f.settings, &f.runtime).is_err());
        assert!(f.live.0.join("backup-restore.yaml").exists());
        assert!(f.allocations()?.iter().all(|p| p.exists()));
    }
    Ok(())
}
#[test]
fn changed_candidate_runtime_is_rejected_before_publication() -> Result<()> {
    let mut f = Fixture::new()?;
    f.begin()?;
    fs::write(f.runtime.path(&f.runtime.state().pending.unwrap())?, "mode: rule\n")?;
    assert!(f.profiles.publish_restore(&mut f.settings, &f.runtime).is_err());
    assert_eq!(f.settings.snapshot(), ServiceSettings::default());
    assert_eq!(f.profiles.snapshot().current.as_deref(), Some(f.old_uid.as_str()));
    f.runtime.abort()?;
    f.profiles.recover_restore(&mut f.settings, &f.runtime)?;
    Ok(())
}
#[test]
fn restore_intent_blocks_other_catalog_writes_and_default_initialization() -> Result<()> {
    let mut f = Fixture::new()?;
    f.begin()?;
    assert!(f.profiles.ensure_global_defaults().is_err());
    assert!(f.profiles.prepare_local_import("conflict", "mode: rule").is_err());
    assert!(f.profiles.edit_profile(&f.old_uid, Default::default()).is_err());
    assert!(
        f.settings
            .begin(f.target.clone(), f.runtime.state().pending.unwrap())
            .is_err()
    );
    Ok(())
}

#[test]
fn missing_committed_file_or_unsafe_parent_never_removes_intent() -> Result<()> {
    for attack in 0..3 {
        let mut f = Fixture::new()?;
        f.begin()?;
        f.profiles.publish_restore(&mut f.settings, &f.runtime)?;
        f.runtime.commit()?;
        match attack {
            0 => fs::remove_file(&f.allocations()?[0])?,
            1 => {
                let dir = f.live.0.join("profiles");
                fs::rename(&dir, f.live.0.join("actual-profiles"))?;
                symlink(f.live.0.join("actual-profiles"), dir)?;
            }
            2 => {
                let p = f.allocations()?[0].with_extension("part");
                fs::write(&p, b"partial")?;
                fs::set_permissions(p, fs::Permissions::from_mode(0o600))?;
            }
            _ => unreachable!(),
        }
        let before = fs::read(f.live.0.join("profiles.yaml"))?;
        assert!(f.profiles.recover_restore(&mut f.settings, &f.runtime).is_err());
        assert_eq!(fs::read(f.live.0.join("profiles.yaml"))?, before);
        assert!(f.live.0.join("backup-restore.yaml").exists());
    }
    Ok(())
}
#[test]
fn empty_bootstrap_catalog_and_shared_raw_files_have_exact_allocation_coverage() -> Result<()> {
    for shared in [false, true] {
        let mut f = Fixture::new()?;
        if shared {
            let second = f.candidate.import_local_with_defaults("shared", "mode: global")?;
            let file = f.candidate.get_item(&f.new_uid)?.file.clone();
            let mut c = f.candidate.snapshot();
            c.items
                .as_mut()
                .unwrap()
                .iter_mut()
                .find(|i| i.uid == second.uid)
                .unwrap()
                .file = file;
            fs::write(f._candidate.0.join("profiles.yaml"), serde_yaml_ng::to_string(&c)?)?;
            f.candidate = ProfileStore::open(&f._candidate.0)?;
        } else {
            fs::write(f._candidate.0.join("profiles.yaml"), "current: null\nitems: null\n")?;
            f.candidate = ProfileStore::open(&f._candidate.0)?;
        }
        let revision = f.runtime.stage(parse("mode: global")?)?;
        let plan = f.profiles.prepare_restore(
            &f.candidate,
            &f.settings,
            f.target.clone(),
            &f.runtime,
            revision.clone(),
        )?;
        let active = plan.active_profile().map(str::to_owned);
        f.runtime.begin_profile(revision, active.clone())?;
        f.profiles.begin_restore(plan, &f.settings, &f.runtime)?;
        assert_eq!(f.allocations()?.len(), if shared { 13 } else { 0 });
        f.profiles.publish_restore(&mut f.settings, &f.runtime)?;
        f.runtime.commit()?;
        f.reopen_recover()?;
        assert_eq!(f.profiles.snapshot().current.as_deref(), active.as_deref());
        if !shared {
            assert!(f.profiles.snapshot().items.is_none());
        }
    }
    Ok(())
}

#[test]
fn first_restore_into_bootstrap_without_a_committed_revision_recovers_both_outcomes() -> Result<()> {
    for committed in [false, true] {
        let mut f = Fixture::new()?;
        fs::remove_file(f.live.0.join("profiles.yaml"))?;
        fs::remove_file(f.live.0.join("config/state.yaml"))?;
        f.profiles = ProfileStore::open(&f.live.0)?;
        f.runtime = RuntimeStore::open(&f.live.0)?;
        f.begin()?;
        f.profiles.publish_restore(&mut f.settings, &f.runtime)?;
        if committed {
            f.runtime.commit()?;
        }
        f.reopen_recover()?;
        assert_eq!(f.runtime.state().current.is_some(), committed);
        assert_eq!(f.profiles.snapshot().current.is_some(), committed);
        assert_eq!(
            f.settings.snapshot().runtime.mixed_port,
            if committed { Some(12345) } else { None }
        );
        assert!(!f.live.0.join("backup-restore.yaml").exists());
    }
    Ok(())
}
