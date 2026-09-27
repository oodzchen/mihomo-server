#![cfg(unix)]
use super::*;
use crate::core_release::{CoreRelease, PreparedCore};
use std::{
    os::unix::fs::{PermissionsExt as _, symlink},
    path::PathBuf,
};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let p = std::env::temp_dir().join(format!("ms-core-upgrade-{}", hex(&random)));
        create_directory(&p)?;
        fs::write(p.join("verge-mihomo"), b"previous working core")?;
        fs::set_permissions(p.join("verge-mihomo"), fs::Permissions::from_mode(0o700))?;
        Ok(Self(p))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn staged(dir: &Directory) -> Result<(PathBuf, StagedCore)> {
    let bytes = b"validated replacement core";
    let path = dir.0.join("staged");
    fs::write(&path, bytes)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    let version = "v1.2.3";
    let package_hash = "a".repeat(64);
    let config_sha256 = "b".repeat(64);
    let id = format!("{version}-{package_hash}");
    Ok((
        path,
        StagedCore {
            stage_id: format!("{id}-{config_sha256}"),
            prepared: PreparedCore {
                id,
                release: CoreRelease {
                    version: version.into(),
                    target: TARGET.into(),
                    asset: format!("mihomo-linux-amd64-v2-{version}.gz"),
                    bytes: 100,
                    sha256: package_hash,
                    download_url: format!(
                        "https://github.com/MetaCubeX/mihomo/releases/download/{version}/mihomo-linux-amd64-v2-{version}.gz"
                    ),
                },
            },
            executable_bytes: bytes.len() as u64,
            executable_sha256: config_hash(bytes),
            config_sha256,
            config_revision: None,
        },
    ))
}
#[test]
fn pending_switch_recovers_before_and_after_atomic_replacement_without_a_receipt() -> Result<()> {
    for published in [false, true] {
        let dir = Directory::new()?;
        let (source, staged) = staged(&dir)?;
        let old = fs::read(dir.0.join("verge-mihomo"))?;
        prepare(&dir.0, &source, &staged, true)?;
        assert!(pending(&dir.0)?);
        assert_eq!(
            fs::metadata(dir.0.join(TRANSACTION))?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(dir.0.join(TRANSACTION).join("journal.json"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        if published {
            publish(&dir.0)?;
            assert_ne!(fs::read(dir.0.join("verge-mihomo"))?, old);
        }
        assert_eq!(recover(&dir.0)?, Some(true));
        assert_eq!(fs::read(dir.0.join("verge-mihomo"))?, old);
        assert_eq!(recover(&dir.0)?, None);
        assert!(installation(&dir.0)?.is_none());
        assert!(!dir.0.join(TRANSACTION).exists());
    }
    Ok(())
}
#[test]
fn committed_switch_completes_receipt_and_cleanup_and_restores_previous_receipt_on_later_failure() -> Result<()> {
    let dir = Directory::new()?;
    let (source, staged) = staged(&dir)?;
    let receipt = prepare(&dir.0, &source, &staged, false)?;
    publish(&dir.0)?;
    commit(&dir.0)?;
    assert!(!pending(&dir.0)?);
    assert!(!dir.0.join(RECEIPT).exists());
    recover(&dir.0)?;
    assert_eq!(installation(&dir.0)?, Some(receipt.clone()));
    assert_eq!(fs::read(dir.0.join("verge-mihomo"))?, fs::read(&source)?);
    let mut next = staged.clone();
    fs::write(&source, b"second validated core")?;
    next.executable_bytes = 21;
    next.executable_sha256 = config_hash(b"second validated core");
    next.stage_id = next.stage_id.replace('a', "c");
    next.prepared.id = next.prepared.id.replace('a', "c");
    next.prepared.release.sha256 = "c".repeat(64);
    prepare(&dir.0, &source, &next, true)?;
    publish(&dir.0)?;
    recover(&dir.0)?;
    assert_eq!(installation(&dir.0)?, Some(receipt));
    fs::write(dir.0.join("verge-mihomo"), b"external change")?;
    assert!(installation(&dir.0).is_err());
    Ok(())
}
#[test]
fn interrupted_backup_and_rename_recovery_are_idempotent_and_preserve_file_mode() -> Result<()> {
    let dir = Directory::new()?;
    let live = dir.0.join("verge-mihomo");
    fs::set_permissions(&live, fs::Permissions::from_mode(0o755))?;
    let root = dir.0.join(TRANSACTION);
    create_directory(&root)?;
    file(&root.join("previous"))?.write_all(b"partial")?;
    file(&root.join("journal.next"))?.write_all(b"{")?;
    recover(&dir.0)?;
    assert_eq!(fs::read(&live)?, b"previous working core");
    assert!(!root.exists());
    let (source, staged) = staged(&dir)?;
    prepare(&dir.0, &source, &staged, false)?;
    publish(&dir.0)?;
    let j = journal(&root)?;
    copy(
        &root.join("previous"),
        &root.join("rollback"),
        &j.previous_sha256,
        j.previous_mode,
        true,
    )?;
    fs::rename(root.join("rollback"), &live)?; // Crash immediately after restoring the old inode.
    recover(&dir.0)?;
    assert_eq!(fs::metadata(live)?.permissions().mode() & 0o777, 0o755);
    Ok(())
}
#[test]
fn corrupt_missing_backup_unknown_live_and_committed_hash_conflicts_fail_without_overwriting() -> Result<()> {
    for case in ["backup", "unknown-live", "corrupt-journal", "committed-live"] {
        let dir = Directory::new()?;
        let (source, staged) = staged(&dir)?;
        prepare(&dir.0, &source, &staged, true)?;
        publish(&dir.0)?;
        let root = dir.0.join(TRANSACTION);
        match case {
            "backup" => fs::write(root.join("previous"), b"wrong")?,
            "unknown-live" => fs::write(dir.0.join("verge-mihomo"), b"operator replacement")?,
            "corrupt-journal" => fs::write(root.join("journal.json"), b"{broken")?,
            "committed-live" => {
                commit(&dir.0)?;
                fs::write(dir.0.join("verge-mihomo"), b"unexpected committed bytes")?;
            }
            _ => unreachable!(),
        }
        let current = fs::read(dir.0.join("verge-mihomo"))?;
        assert!(recover(&dir.0).is_err());
        assert_eq!(fs::read(dir.0.join("verge-mihomo"))?, current);
        assert!(root.exists());
    }
    Ok(())
}
#[test]
fn links_unknown_files_and_forged_records_are_not_followed_or_deleted() -> Result<()> {
    let dir = Directory::new()?;
    let outside = dir.0.join("outside");
    fs::write(&outside, b"private unrelated data")?;
    let root = dir.0.join(TRANSACTION);
    create_directory(&root)?;
    symlink(&outside, root.join("previous"))?;
    assert!(recover(&dir.0).is_err());
    assert_eq!(fs::read(&outside)?, b"private unrelated data");
    fs::remove_file(root.join("previous"))?;
    file(&root.join("unknown"))?.write_all(b"preserve me")?;
    assert!(recover(&dir.0).is_err());
    assert!(root.join("unknown").exists());
    fs::remove_file(root.join("unknown"))?;
    fs::remove_dir(&root)?;
    let (source, staged) = staged(&dir)?;
    prepare(&dir.0, &source, &staged, false)?;
    let mut j = journal(&root)?;
    j.installation.stage_id = "../../outside".into();
    write_journal(&root, &j)?;
    assert!(recover(&dir.0).is_err());
    assert_eq!(fs::read(&outside)?, b"private unrelated data");
    Ok(())
}
