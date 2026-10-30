//! The host's one system-wide TUN.
//!
//! The installer creates `tun.lock` (root:mihomo-tun 0640) beside the slot
//! registry. A TUN-group member's service holds an exclusive `flock` on it while
//! its core runs with TUN enabled, and the kernel releases the lock when that
//! TUN is turned off or the service exits, so a crashed holder never blocks the
//! host. The holder is found through `/proc/locks`, which every user can read.
//!
//! The member who turned TUN on last gets it. A member who turns TUN on while
//! another holds it writes its UID and the time into `tun.request`
//! (root:mihomo-tun 0660) and waits for the lock; the holder's service sees the
//! request, turns its own TUN off and so releases the lock. Installations
//! without that file keep the TUN with its first holder.
use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    os::{fd::AsRawFd as _, unix::fs::MetadataExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use parking_lot::Mutex;

pub const NAME: &str = "tun.lock";
pub const REQUEST: &str = "tun.request";

/// This account is not in the TUN group; an administrator must add it.
#[derive(Debug)]
pub struct Unauthorized;

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "TUN is not available to this user: ask the administrator to add this user to the TUN group (mihomo-tun)",
        )
    }
}

impl std::error::Error for Unauthorized {}

/// Another account holds the system TUN.
#[derive(Debug)]
pub struct Busy {
    pub holder: Option<u32>,
    /// It was asked to hand the TUN over and did not in time.
    pub asked: bool,
}

impl std::fmt::Display for Busy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let holder = self.holder.map_or_else(|| "another user".to_owned(), describe);
        if self.asked {
            write!(
                formatter,
                "{holder} did not hand over the system-wide TUN in time; try again"
            )
        } else {
            write!(
                formatter,
                "the system-wide TUN is in use by {holder}; it can be enabled here after they turn it off"
            )
        }
    }
}

impl std::error::Error for Busy {}

#[derive(Debug)]
struct Held {
    _file: File,
    /// When this service took the TUN or last took a request: requests up to
    /// then were meant for someone else or are being handled.
    since: u128,
}

#[derive(Debug)]
pub struct TunLock {
    path: PathBuf,
    held: Mutex<Option<Held>>,
}

/// Nanoseconds since boot: one clock for every service on the host, unaffected
/// by wall-clock changes.
fn now() -> u128 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes only into the provided timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) };
    time.tv_sec as u128 * 1_000_000_000 + time.tv_nsec as u128
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

    /// Take the system TUN; refused with [`Unauthorized`] or [`Busy`].
    pub fn acquire(&self) -> Result<()> {
        let mut held = self.held.lock();
        if held.is_some() {
            return Ok(());
        }
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Err(Unauthorized.into()),
            Err(error) => bail!("open {}: {error}", self.path.display()),
        };
        // SAFETY: flock on a descriptor this function owns.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Err(Busy {
                    holder: self.holder(),
                    asked: false,
                }
                .into());
            }
            bail!("lock {}: {error}", self.path.display());
        }
        *held = Some(Held {
            _file: file,
            since: now(),
        });
        Ok(())
    }

    fn request_path(&self) -> PathBuf {
        self.path.with_file_name(REQUEST)
    }

    /// Ask the holder to hand the TUN over; false if this installation has no
    /// request file (the TUN then stays with its holder).
    pub fn request_takeover(&self) -> Result<bool> {
        let path = self.request_path();
        let mut file = match OpenOptions::new().write(true).truncate(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Err(Unauthorized.into()),
            Err(error) => bail!("open {}: {error}", path.display()),
        };
        file.write_all(format!("{} {}\n", crate::secure_fs::euid(), now()).as_bytes())
            .map_err(|error| anyhow::anyhow!("write {}: {error}", path.display()))?;
        Ok(true)
    }

    /// The account that asked for the TUN since this service took it, once
    /// per request.
    pub fn take_request(&self) -> Option<u32> {
        let mut held = self.held.lock();
        let held = held.as_mut()?;
        let request = std::fs::read_to_string(self.request_path()).ok()?;
        let (uid, at) = request.trim().split_once(' ')?;
        let (uid, at) = (uid.parse::<u32>().ok()?, at.parse::<u128>().ok()?);
        if at <= held.since || uid == crate::secure_fs::euid() {
            return None;
        }
        held.since = at;
        Some(uid)
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
        assert_eq!(error.downcast_ref::<Busy>().map(|busy| busy.holder), Some(Some(uid)));
        // Without the installer's request file nobody can be asked to hand over.
        assert!(!second.request_takeover()?);
        first.release();
        assert_eq!(second.holder(), None);
        second.acquire()?;
        drop(second);
        first.acquire()?;
        first.release();
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn takeover_requests_reach_only_the_current_holder() -> Result<()> {
        let root = std::env::temp_dir().join(format!("ms-tun-request-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let registry = root.join("slots");
        std::fs::write(root.join(NAME), b"")?;
        std::fs::write(root.join(REQUEST), b"")?;
        let holder = TunLock::beside(&registry).expect("lock file present");
        assert_eq!(holder.take_request(), None, "not held");
        holder.acquire()?;
        assert_eq!(holder.take_request(), None, "empty request");
        // A request from before this service took the TUN was for someone else.
        let other = crate::secure_fs::euid().wrapping_add(1);
        std::fs::write(root.join(REQUEST), format!("{other} 1\n"))?;
        assert_eq!(holder.take_request(), None);
        std::fs::write(root.join(REQUEST), format!("{other} {}\n", now()))?;
        assert_eq!(holder.take_request(), Some(other));
        assert_eq!(holder.take_request(), None, "reported once");
        // Its own request (written by `request_takeover`) never asks itself.
        assert!(holder.request_takeover()?);
        assert_eq!(holder.take_request(), None);
        holder.release();
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
