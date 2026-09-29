//! Data-directory scoped restore scratch space; callers hold the service data lock.
use crate::secure_fs;
use anyhow::{Context as _, Result, ensure};
use std::{
    ffi::{CString, OsStr, OsString},
    fs::{self, File, Metadata, OpenOptions},
    io,
    os::{
        unix::{
            ffi::OsStrExt as _,
            fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _},
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const ROOT: &str = "restore-candidates";
const LEASE: &str = ".lease";
const MAX_ENTRIES: usize = 4096;

fn private_dir(meta: &Metadata) -> bool {
    meta.is_dir() && meta.uid() == secure_fs::euid() && meta.mode() & 0o7077 == 0
}
fn owned_file(meta: &Metadata) -> bool {
    meta.is_file() && meta.uid() == secure_fs::euid() && meta.nlink() == 1 && meta.mode() & 0o7177 == 0
}
fn writable(dir: &File) -> Result<()> {
    ensure!(
        secure_fs::fchmod(dir, 0o700).is_ok(),
        "restore scratch permissions could not be recovered"
    );
    Ok(())
}
fn identity(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}
fn name(value: &OsStr) -> Result<CString> {
    Ok(CString::new(value.as_bytes())?)
}
pub(super) fn open_at(parent: &File, child: &OsStr, directory: bool) -> io::Result<File> {
    let child = CString::new(child.as_bytes()).map_err(io::Error::other)?;
    let flags = libc::O_RDONLY
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | libc::O_CLOEXEC
        | if directory { libc::O_DIRECTORY } else { 0 };
    secure_fs::open_at(parent, &child, flags, 0)
}
fn unlink_at(parent: &File, child: &OsStr, directory: bool) -> Result<()> {
    let child = name(child)?;
    secure_fs::unlink_at(parent, &child, if directory { libc::AT_REMOVEDIR } else { 0 })
        .map_err(|error| anyhow::anyhow!("restore scratch removal failed: {error}"))
}
pub(super) fn names(dir: &File) -> Result<Vec<OsString>> {
    secure_fs::dir_names(dir, MAX_ENTRIES)
        .context("restore scratch enumeration failed")?
        .context("restore scratch entry limit exceeded")
}
fn root(data: &Path, create: bool) -> Result<Option<File>> {
    let path = data.join(ROOT);
    if create {
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {
                File::open(data)?.sync_all()?;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if !create && e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("unsafe restore scratch root"),
    };
    ensure!(
        private_dir(&file.metadata()?) && identity(&file.metadata()?, &fs::symlink_metadata(&path)?),
        "restore scratch root must be an owned private directory"
    );
    Ok(Some(file))
}
fn candidate_name(child: &OsStr) -> bool {
    child
        .as_bytes()
        .strip_prefix(b"ms-restore-")
        .is_some_and(|id| id.len() == 24 && id.iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c)))
}
fn lock(file: &File) -> Result<bool> {
    Ok(secure_fs::try_lock_exclusive(file)?)
}
struct Budget {
    remaining: usize,
    deadline: Instant,
}
impl Budget {
    fn new() -> Self {
        Self {
            remaining: MAX_ENTRIES,
            deadline: Instant::now() + Duration::from_secs(15),
        }
    }
    fn check(&mut self, depth: usize) -> Result<()> {
        ensure!(
            depth <= 16 && self.remaining > 0 && Instant::now() < self.deadline,
            "restore scratch cleanup budget exceeded"
        );
        self.remaining -= 1;
        Ok(())
    }
}
fn remove_contents(dir: &File, depth: usize, keep_lease: bool, budget: &mut Budget) -> Result<()> {
    for child in names(dir)? {
        if keep_lease && child == LEASE {
            continue;
        }
        budget.check(depth)?;
        match open_at(dir, &child, true) {
            Ok(nested) => {
                ensure!(
                    nested.metadata()?.uid() == secure_fs::euid()
                        && nested.metadata()?.dev() == dir.metadata()?.dev(),
                    "unsafe restore scratch child directory"
                );
                writable(&nested)?;
                remove_contents(&nested, depth + 1, false, budget)?;
                let current = open_at(dir, &child, true)?;
                ensure!(
                    identity(&nested.metadata()?, &current.metadata()?),
                    "restore scratch child changed"
                );
                unlink_at(dir, &child, true)?;
            }
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) => {
                // unlinkat removes symlinks/FIFOs themselves and never follows their targets.
                unlink_at(dir, &child, false)?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn remove_candidate(root: &File, dir: &File, child: &OsStr, leased: bool, budget: &mut Budget) -> Result<()> {
    remove_contents(dir, 0, leased, budget)?;
    let current = open_at(root, child, true)?;
    ensure!(
        identity(&dir.metadata()?, &current.metadata()?),
        "restore scratch candidate changed"
    );
    if leased {
        unlink_at(dir, OsStr::new(LEASE), false)?;
    }
    unlink_at(root, child, true)?;
    root.sync_all()?;
    Ok(())
}

/// Retain unsafe/unrecognized/leased entries. Root ownership violations fail closed.
/// Empty recognized directories cover a crash between mkdir and lease creation.
pub(crate) fn cleanup(data: &Path) -> Result<()> {
    let Some(root) = root(data, false)? else {
        return Ok(());
    };
    writable(&root)?;
    let mut budget = Budget::new();
    for child in names(&root)? {
        budget.check(0)?;
        if !candidate_name(&child) {
            continue;
        }
        let Ok(dir) = open_at(&root, &child, true) else {
            continue;
        };
        if !private_dir(&dir.metadata()?) {
            continue;
        }
        match open_at(&dir, OsStr::new(LEASE), false) {
            Ok(lease) if owned_file(&lease.metadata()?) && lock(&lease)? => {
                writable(&dir)?;
                remove_candidate(&root, &dir, &child, true, &mut budget)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound && names(&dir)?.is_empty() => {
                writable(&dir)?;
                remove_candidate(&root, &dir, &child, false, &mut budget)?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) struct PrivateDirectory(pub PathBuf, File, File, File);
impl PrivateDirectory {
    pub fn new(data: &Path) -> Result<Self> {
        let root = root(data, true)?.context("restore scratch root missing")?;
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| anyhow::anyhow!("restore candidate ID failed"))?;
        let child = format!("ms-restore-{}", &super::hash(&random)[..24]);
        let child_c = name(OsStr::new(&child))?;
        ensure!(
            secure_fs::mkdir_at(&root, &child_c, 0o700).is_ok(),
            "restore candidate creation failed"
        );
        let dir = open_at(&root, OsStr::new(&child), true)?;
        let lease_c = name(OsStr::new(LEASE))?;
        let lease = secure_fs::open_at(
            &dir,
            &lease_c,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
        .map_err(|_| anyhow::anyhow!("restore candidate lease creation failed"))?;
        ensure!(lock(&lease)?, "restore candidate lease busy");
        lease.sync_all()?;
        dir.sync_all()?;
        root.sync_all()?;
        Ok(Self(data.join(ROOT).join(child), root, dir, lease))
    }
    pub fn cleanup(&self) -> Result<()> {
        // Keep the lease until all contents are gone, including on partial failure.
        let _lease = &self.3;
        remove_candidate(
            &self.1,
            &self.2,
            self.0.file_name().context("restore candidate name missing")?,
            true,
            &mut Budget::new(),
        )
        .context("restore candidate cleanup failed")
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Result<Self> {
            let mut random = [0; 16];
            getrandom::fill(&mut random).unwrap();
            let path = std::env::temp_dir().join(format!("ms-candidate-tests-{}", crate::backup::hash(&random)));
            fs::DirBuilder::new().mode(0o700).create(&path)?;
            Ok(Self(path))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn orphan(data: &Path, digit: char, leased: bool) -> Result<PathBuf> {
        let _root = root(data, true)?;
        let path = data
            .join(ROOT)
            .join(format!("ms-restore-{}", digit.to_string().repeat(24)));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        if leased {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path.join(LEASE))?;
        }
        Ok(path)
    }
    #[test]
    fn cleanup_scopes_to_owned_namespace_preserves_live_leases_and_handles_empty_creation_gap() -> Result<()> {
        let a = Temp::new()?;
        let b = Temp::new()?;
        let live = PrivateDirectory::new(&a.0)?;
        fs::write(live.0.join("live.yaml"), b"live")?;
        let stale = orphan(&a.0, 'a', true)?;
        fs::write(stale.join("runtime.yaml"), b"old")?;
        let empty = orphan(&a.0, 'b', false)?;
        let other = orphan(&b.0, 'a', true)?;
        fs::write(a.0.join("runtime.yaml"), b"live state")?;
        fs::create_dir(a.0.join(ROOT).join("unrecognized"))?;
        cleanup(&a.0)?;
        assert!(!stale.exists() && !empty.exists());
        assert_eq!(fs::read(live.0.join("live.yaml"))?, b"live");
        assert_eq!(fs::read(a.0.join("runtime.yaml"))?, b"live state");
        assert!(other.exists() && a.0.join(ROOT).join("unrecognized").exists());
        live.cleanup()?;
        assert!(!live.0.exists());
        cleanup(&a.0)?;
        Ok(())
    }
    #[test]
    fn unsafe_root_and_unverifiable_candidates_are_retained_without_following_links() -> Result<()> {
        let data = Temp::new()?;
        let outside = Temp::new()?;
        fs::write(outside.0.join("sentinel"), b"keep")?;
        symlink(&outside.0, data.0.join(ROOT))?;
        assert!(cleanup(&data.0).is_err());
        assert!(PrivateDirectory::new(&data.0).is_err());
        fs::remove_file(data.0.join(ROOT))?;
        root(&data.0, true)?;
        fs::set_permissions(data.0.join(ROOT), fs::Permissions::from_mode(0o755))?;
        assert!(cleanup(&data.0).is_err());
        fs::set_permissions(data.0.join(ROOT), fs::Permissions::from_mode(0o700))?;
        let no_lease = orphan(&data.0, 'a', false)?;
        fs::write(no_lease.join("unknown"), b"keep")?;
        let public = orphan(&data.0, 'b', true)?;
        fs::set_permissions(&public, fs::Permissions::from_mode(0o777))?;
        let shared = orphan(&data.0, 'c', true)?;
        fs::hard_link(shared.join(LEASE), outside.0.join("shared-lease"))?;
        let linked = orphan(&data.0, 'd', false)?;
        symlink(outside.0.join("sentinel"), linked.join(LEASE))?;
        let link = data.0.join(ROOT).join(format!("ms-restore-{}", "e".repeat(24)));
        symlink(&outside.0, &link)?;
        cleanup(&data.0)?;
        for path in [no_lease, public, shared, linked, link] {
            assert!(fs::symlink_metadata(path).is_ok());
        }
        assert_eq!(fs::read(outside.0.join("sentinel"))?, b"keep");
        Ok(())
    }
    #[test]
    fn anchored_recursive_cleanup_unlinks_symlinks_and_fifos_without_touching_targets() -> Result<()> {
        let data = Temp::new()?;
        let outside = Temp::new()?;
        fs::write(outside.0.join("sentinel"), b"keep")?;
        let stale = orphan(&data.0, 'a', true)?;
        fs::create_dir(stale.join("core-created"))?;
        fs::write(stale.join("core-created/cache.db"), b"scratch")?;
        symlink(&outside.0, stale.join("outside"))?;
        let fifo = name(stale.join("pipe").as_os_str())?;
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        cleanup(&data.0)?;
        assert!(!stale.exists());
        assert_eq!(fs::read(outside.0.join("sentinel"))?, b"keep");
        Ok(())
    }
    #[test]
    fn orphan_private_read_only_permissions_are_recovered_without_changing_live_candidates() -> Result<()> {
        let data = Temp::new()?;
        let live = PrivateDirectory::new(&data.0)?;
        fs::set_permissions(&live.0, fs::Permissions::from_mode(0o500))?;
        let stale = orphan(&data.0, 'a', true)?;
        fs::create_dir(stale.join("cache"))?;
        fs::write(stale.join("cache/scratch"), b"data")?;
        fs::set_permissions(stale.join("cache"), fs::Permissions::from_mode(0o500))?;
        fs::set_permissions(&stale, fs::Permissions::from_mode(0o500))?;
        cleanup(&data.0)?;
        assert!(!stale.exists());
        assert_eq!(fs::metadata(&live.0)?.mode() & 0o777, 0o500);
        fs::set_permissions(&live.0, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
    #[test]
    fn depth_budget_failure_retains_lease_for_retry() -> Result<()> {
        let data = Temp::new()?;
        let stale = orphan(&data.0, 'a', true)?;
        let mut nested = stale.clone();
        for _ in 0..18 {
            nested = nested.join("deep");
            fs::create_dir(&nested)?;
        }
        fs::write(nested.join("scratch"), b"data")?;
        assert!(cleanup(&data.0).is_err());
        assert!(stale.join(LEASE).exists());
        fs::remove_dir_all(stale.join("deep"))?;
        cleanup(&data.0)?;
        assert!(!stale.exists());
        Ok(())
    }
}
