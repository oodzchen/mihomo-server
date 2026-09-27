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

#[test]
fn repair_preserves_broken_inode_mode_and_recovers_every_pending_boundary() -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    for (bytes, mode) in [
        (b"".as_slice(), 0o700),
        (b"unreadable original".as_slice(), 0),
        (b"nonexecutable original".as_slice(), 0o600),
    ] {
        for boundary in ["prepared", "published", "restored", "committed"] {
            let dir = Directory::new()?;
            let live = dir.0.join("verge-mihomo");
            fs::write(&live, bytes)?;
            fs::set_permissions(&live, fs::Permissions::from_mode(mode))?;
            let inode = fs::metadata(&live)?.ino();
            let (source, staged) = staged(&dir)?;
            let receipt = prepare_with_repair(&dir.0, &source, &staged, false, true)?;
            assert_eq!(fs::metadata(&live)?.ino(), inode);
            assert_eq!(journal(&dir.0.join(TRANSACTION))?.schema_version, 2);
            if boundary != "prepared" {
                publish(&dir.0)?;
            }
            if boundary == "restored" {
                // Crash after rollback rename but before receipt/cleanup publication.
                fs::rename(dir.0.join(TRANSACTION).join("previous"), &live)?;
            }
            if boundary == "committed" {
                commit(&dir.0)?;
                assert_eq!(recover(&dir.0)?, None);
                assert_eq!(installation(&dir.0)?, Some(receipt));
                assert_eq!(fs::read(&live)?, fs::read(source)?);
            } else {
                assert_eq!(recover(&dir.0)?, Some(false));
                assert_eq!(fs::metadata(&live)?.ino(), inode);
                assert_eq!(fs::metadata(&live)?.permissions().mode() & 0o777, mode);
                fs::set_permissions(&live, fs::Permissions::from_mode(0o600))?;
                assert_eq!(fs::read(&live)?, bytes);
                assert!(installation(&dir.0)?.is_none());
            }
            assert!(!dir.0.join(TRANSACTION).exists());
            assert_eq!(recover(&dir.0)?, None);
        }
    }
    Ok(())
}
#[test]
fn repair_restores_unverified_previous_receipt_and_replaces_it_only_after_commit() -> Result<()> {
    let dir = Directory::new()?;
    let (source, staged) = staged(&dir)?;
    prepare(&dir.0, &source, &staged, false)?;
    publish(&dir.0)?;
    commit(&dir.0)?;
    recover(&dir.0)?;
    let receipt = fs::read(dir.0.join(RECEIPT))?;
    fs::write(dir.0.join("verge-mihomo"), [])?;
    assert!(installation(&dir.0).is_err());
    prepare_with_repair(&dir.0, &source, &staged, false, true)?;
    publish(&dir.0)?;
    recover(&dir.0)?;
    assert_eq!(fs::metadata(dir.0.join("verge-mihomo"))?.len(), 0);
    assert_eq!(fs::read(dir.0.join(RECEIPT))?, receipt);
    assert!(installation(&dir.0).is_err());
    let repaired = prepare_with_repair(&dir.0, &source, &staged, false, true)?;
    publish(&dir.0)?;
    commit(&dir.0)?;
    recover(&dir.0)?;
    assert_eq!(installation(&dir.0)?, Some(repaired));
    Ok(())
}
#[test]
fn repair_identity_conflicts_and_unsafe_files_fail_without_overwriting() -> Result<()> {
    for case in [
        "live-inode",
        "backup",
        "symlink",
        "shared",
        "privileged",
        "unknown-live",
        "receipt-link",
        "receipt-malformed",
    ] {
        let dir = Directory::new()?;
        let live = dir.0.join("verge-mihomo");
        let (source, staged) = staged(&dir)?;
        if case == "symlink" {
            fs::remove_file(&live)?;
            symlink(&source, &live)?;
            assert!(prepare_with_repair(&dir.0, &source, &staged, false, true).is_err());
        } else if case == "shared" {
            fs::hard_link(&live, dir.0.join("unrelated"))?;
            assert!(prepare_with_repair(&dir.0, &source, &staged, false, true).is_err());
        } else if case.starts_with("receipt-") {
            if case == "receipt-link" {
                symlink(dir.0.join("missing"), dir.0.join(RECEIPT))?;
            } else {
                fs::write(dir.0.join(RECEIPT), b"{broken")?;
                fs::set_permissions(dir.0.join(RECEIPT), fs::Permissions::from_mode(0o600))?;
            }
            assert!(prepare_with_repair(&dir.0, &source, &staged, false, true).is_err());
            assert!(!dir.0.join(TRANSACTION).exists());
        } else if case == "privileged" {
            fs::set_permissions(&live, fs::Permissions::from_mode(0o4700))?;
            assert!(prepare_with_repair(&dir.0, &source, &staged, false, true).is_err());
        } else {
            prepare_with_repair(&dir.0, &source, &staged, false, true)?;
            if case == "live-inode" {
                let replacement = dir.0.join("replacement");
                fs::write(&replacement, b"previous working core")?;
                fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700))?;
                fs::rename(replacement, &live)?;
                assert!(publish(&dir.0).is_err());
            } else {
                publish(&dir.0)?;
                fs::write(
                    if case == "backup" {
                        dir.0.join(TRANSACTION).join("previous")
                    } else {
                        live.clone()
                    },
                    b"operator replacement",
                )?;
            }
            let before = fs::read(&live)?;
            assert!(recover(&dir.0).is_err());
            assert_eq!(fs::read(&live)?, before);
            assert!(dir.0.join(TRANSACTION).exists());
        }
    }
    Ok(())
}
