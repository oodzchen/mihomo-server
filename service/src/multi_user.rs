//! Slot claims for a shared, multi-user installation.
//!
//! The administrator creates one root-owned sticky directory (like `/tmp`).
//! Each user claims a slot by creating a file named after it; the sticky bit
//! keeps other users from deleting that claim. A claim is kept until its owner
//! removes it, so the user's ports and TUN routing stay stable across restarts.
use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::Path,
};

use anyhow::{Context as _, Result, bail, ensure};
use headless_core::enhance::isolation::SLOTS;

pub const DEFAULT_SLOT_REGISTRY: &str = "/var/lib/mihomo-server/slots";
/// Beside the registry: the UID of the installing user, whose TUN is system-wide.
pub const TUN_OWNER: &str = "tun-owner";

/// The installation's system TUN owner, written by the installer; `None` keeps
/// every TUN scoped to its own user. Trusted like the registry: root-owned
/// (or owned by this user, for private registries) and not writable by others.
pub fn tun_owner(registry: &Path, uid: u32) -> Result<Option<u32>> {
    let path = registry.with_file_name(TUN_OWNER);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    ensure!(
        metadata.is_file() && (metadata.uid() == 0 || metadata.uid() == uid) && metadata.mode() & 0o022 == 0,
        "{} must be a regular file owned by root and writable only by it",
        path.display()
    );
    let owner = fs::read_to_string(&path)?;
    let owner = owner
        .trim()
        .parse()
        .with_context(|| format!("{} must contain one UID", path.display()))?;
    Ok(Some(owner))
}

/// The lowest slot this user already owns, or a newly claimed free one.
pub fn claim_slot(registry: &Path, uid: u32) -> Result<u16> {
    let metadata = fs::symlink_metadata(registry)
        .with_context(|| format!("open multi-user slot registry {}", registry.display()))?;
    ensure!(metadata.is_dir(), "slot registry must be a real directory");
    ensure!(
        metadata.uid() == 0 || metadata.uid() == uid,
        "slot registry must be owned by root"
    );
    ensure!(
        metadata.mode() & 0o1000 != 0,
        "slot registry must have the sticky bit so claims cannot be removed by others"
    );
    for slot in 0..SLOTS {
        if let Ok(claim) = fs::symlink_metadata(registry.join(slot.to_string()))
            && claim.is_file()
            && claim.uid() == uid
        {
            return Ok(slot);
        }
    }
    for slot in 0..SLOTS {
        let claim = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(registry.join(slot.to_string()));
        match claim {
            Ok(mut file) => {
                writeln!(file, "{uid}")?;
                file.sync_all()?;
                crate::secure_fs::sync_directory(registry)?;
                return Ok(slot);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context("claim multi-user slot"),
        }
    }
    bail!("all {SLOTS} multi-user slots are claimed; ask the administrator to release unused ones")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn registry(name: &str) -> Result<std::path::PathBuf> {
        let path = std::env::temp_dir().join(format!("ms-slots-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o1777))?;
        Ok(path)
    }

    #[test]
    fn claims_are_stable_and_skip_other_entries() -> Result<()> {
        let path = registry("stable")?;
        let uid = crate::secure_fs::euid();
        fs::create_dir(path.join("0"))?; // not a regular file: never reused
        assert_eq!(claim_slot(&path, uid)?, 1);
        assert_eq!(claim_slot(&path, uid)?, 1);
        assert_eq!(fs::read_to_string(path.join("1"))?, format!("{uid}\n"));
        // A registry owned by an ordinary user is trusted only by that user.
        let error = claim_slot(&path, uid.wrapping_add(1)).unwrap_err();
        assert!(format!("{error:#}").contains("owned by root"), "{error:#}");
        fs::remove_dir_all(&path)?;
        Ok(())
    }

    #[test]
    fn tun_owner_is_optional_and_must_be_trusted() -> Result<()> {
        let path = registry("owner")?;
        let uid = crate::secure_fs::euid();
        let owner = path.with_file_name(TUN_OWNER);
        let _ = fs::remove_file(&owner);
        assert_eq!(tun_owner(&path, uid)?, None);
        fs::write(&owner, "1000\n")?;
        fs::set_permissions(&owner, fs::Permissions::from_mode(0o644))?;
        assert_eq!(tun_owner(&path, uid)?, Some(1000));
        fs::set_permissions(&owner, fs::Permissions::from_mode(0o666))?;
        assert!(tun_owner(&path, uid).is_err());
        fs::set_permissions(&owner, fs::Permissions::from_mode(0o644))?;
        fs::write(&owner, "alice\n")?;
        assert!(tun_owner(&path, uid).is_err());
        fs::remove_file(&owner)?;
        fs::remove_dir_all(&path)?;
        Ok(())
    }

    #[test]
    fn registry_without_sticky_bit_is_rejected() -> Result<()> {
        let path = registry("sticky")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o777))?;
        let error = claim_slot(&path, crate::secure_fs::euid()).unwrap_err();
        assert!(format!("{error:#}").contains("sticky"), "{error:#}");
        fs::remove_dir_all(&path)?;
        Ok(())
    }

    #[test]
    fn full_registry_reports_exhaustion() -> Result<()> {
        let path = registry("full")?;
        for slot in 0..SLOTS {
            fs::create_dir(path.join(slot.to_string()))?;
        }
        let error = claim_slot(&path, crate::secure_fs::euid()).unwrap_err();
        assert!(format!("{error:#}").contains("all 64"), "{error:#}");
        fs::remove_dir_all(&path)?;
        Ok(())
    }
}
