//! The host's one system-wide TUN.
//!
//! The installer creates `tun.lock` (root:mihomo-tun 0640) beside the slot
//! registry. A TUN-group member's service holds an exclusive `flock` on it while
//! its core runs with TUN enabled; the first to enable TUN wins, and the kernel
//! releases the lock when that TUN is turned off or the service exits, so a
//! crashed holder never blocks the host. Others cannot take it over: only the
//! holder or root (by stopping the holder) ends it. The holder is found through
//! `/proc/locks`, which every user can read.
use std::{
    fs::File,
    os::{fd::AsRawFd as _, unix::fs::MetadataExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use parking_lot::Mutex;

pub const NAME: &str = "tun.lock";

#[derive(Debug)]
pub struct TunLock {
    path: PathBuf,
    held: Mutex<Option<File>>,
}

impl TunLock {
    /// The installation's lock beside REGISTRY, if the installer created one.
    pub fn beside(registry: &Path) -> Option<Self> {
        let path = registry.with_file_name(NAME);
        let metadata = std::fs::symlink_metadata(&path).ok()?;
        metadata.is_file().then(|| Self {
            path,
            held: Mutex::new(None),
        })
    }

    pub fn held(&self) -> bool {
        self.held.lock().is_some()
    }

    /// Take the system TUN, or explain who has it.
    pub fn acquire(&self) -> Result<()> {
        let mut held = self.held.lock();
        if held.is_some() {
            return Ok(());
        }
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                bail!(
                    "TUN is not available to this user: ask the administrator to add this user to the TUN group (mihomo-tun)"
                )
            }
            Err(error) => bail!("open {}: {error}", self.path.display()),
        };
        // SAFETY: flock on a descriptor this function owns.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                bail!(
                    "the system-wide TUN is in use by {}; it can be enabled here after they turn it off",
                    self.holder().map_or_else(|| "another user".to_owned(), describe)
                );
            }
            bail!("lock {}: {error}", self.path.display());
        }
        *held = Some(file);
        Ok(())
    }

    pub fn release(&self) {
        self.held.lock().take();
    }

    /// The UID whose service holds the system TUN, including this one.
    pub fn holder(&self) -> Option<u32> {
        let metadata = std::fs::metadata(&self.path).ok()?;
        let (major, minor) = (libc::major(metadata.dev()), libc::minor(metadata.dev()));
        let device = format!("{major:02x}:{minor:02x}:{}", metadata.ino());
        let locks = std::fs::read_to_string("/proc/locks").ok()?;
        // "1: FLOCK  ADVISORY  WRITE 4242 fd:00:1234 0 EOF"
        locks.lines().find_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let position = fields.iter().position(|field| *field == "FLOCK")?;
            let pid = fields.get(position + 3)?;
            (fields.get(position + 4) == Some(&device.as_str()))
                .then(|| {
                    std::fs::metadata(format!("/proc/{pid}"))
                        .ok()
                        .map(|process| process.uid())
                })
                .flatten()
        })
    }
}

/// "alice (uid 1000)", or just the UID when the account cannot be named.
pub fn describe(uid: u32) -> String {
    let mut buffer = vec![0_u8; 4096];
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    // SAFETY: getpwuid_r writes only into the provided entry and buffer.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status == 0 && !result.is_null() {
        // SAFETY: on success pw_name points into the buffer as a C string.
        let name = unsafe { std::ffi::CStr::from_ptr((*result).pw_name) };
        format!("{} (uid {uid})", name.to_string_lossy())
    } else {
        format!("uid {uid}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time_and_released_on_drop() -> Result<()> {
        let root = std::env::temp_dir().join(format!("ms-tun-lock-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let registry = root.join("slots");
        assert!(TunLock::beside(&registry).is_none());
        std::fs::write(root.join(NAME), b"")?;
        let first = TunLock::beside(&registry).expect("lock file present");
        let second = TunLock::beside(&registry).expect("lock file present");
        assert_eq!(first.holder(), None);
        first.acquire()?;
        first.acquire()?; // idempotent for the holder
        assert!(first.held());
        let uid = crate::secure_fs::euid();
        assert_eq!(second.holder(), Some(uid));
        let error = second.acquire().unwrap_err();
        assert!(format!("{error:#}").contains("in use by"), "{error:#}");
        first.release();
        assert_eq!(second.holder(), None);
        second.acquire()?;
        drop(second);
        first.acquire()?;
        first.release();
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
