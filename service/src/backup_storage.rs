//! Bounded, private local archive storage. The actor owns data lock and admission.
use anyhow::{Context as _, Result, ensure};
use headless_core::backup::{
    BackupDeletionReceipt, BackupMetadata, MAX_ARCHIVE_BYTES, RetainedBackup, RetainedBackupList, RetainedBackupReceipt,
};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File, Metadata, OpenOptions},
    io::{Read as _, Write as _},
    os::{
        unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _},
    },
    path::Path,
    time::{Duration, Instant},
};
use tokio::sync::watch;
const ROOT: &str = "backups";
pub(crate) const MAX_ARCHIVES: usize = 32;
pub(crate) const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_STORAGE_ENTRIES: usize = 128;
#[derive(Debug)]
pub(crate) struct StorageFull;
impl std::fmt::Display for StorageFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("local backup capacity reached")
    }
}
impl std::error::Error for StorageFull {}
#[derive(Debug)]
pub(crate) struct Missing;
impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("local backup not found")
    }
}
impl std::error::Error for Missing {}
fn hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub(crate) fn valid_id(s: &str) -> bool {
    hex(s, 24)
}
fn parse(filename: &str) -> Option<RetainedBackup> {
    let mut fields = filename.strip_prefix("backup-")?.strip_suffix(".zip")?.split('-');
    let id = fields.next()?;
    let timestamp = fields.next()?;
    let sha = fields.next()?;
    if fields.next().is_some() || !valid_id(id) || !hex(timestamp, 16) || !hex(sha, 64) {
        return None;
    }
    Some(RetainedBackup {
        id: id.into(),
        created_at: u64::from_str_radix(timestamp, 16).ok()?,
        content_length: 0,
        sha256: sha.into(),
    })
}
fn filename(row: &RetainedBackup) -> String {
    format!("backup-{}-{:016x}-{}.zip", row.id, row.created_at, row.sha256)
}
fn safe_file(meta: &Metadata) -> bool {
    meta.is_file()
        && meta.uid() == crate::secure_fs::euid()
        && meta.nlink() == 1
        && meta.mode() & 0o7177 == 0
        && meta.len() <= MAX_ARCHIVE_BYTES as u64
}
fn root(data: &Path, create: bool) -> Result<Option<File>> {
    let path = data.join(ROOT);
    if create {
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => File::open(data)?.sync_all()?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let meta = file.metadata()?;
    let current = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir()
            && meta.uid() == crate::secure_fs::euid()
            && meta.mode() & 0o7077 == 0
            && meta.dev() == current.dev()
            && meta.ino() == current.ino(),
        "unsafe local backup directory"
    );
    Ok(Some(file))
}
struct Budget {
    deadline: Instant,
    stop: watch::Receiver<bool>,
}
impl Budget {
    fn new(stop: watch::Receiver<bool>) -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(15),
            stop,
        }
    }
    fn check(&self) -> Result<()> {
        ensure!(
            !*self.stop.borrow() && Instant::now() < self.deadline,
            "local backup operation cancelled or timed out"
        );
        Ok(())
    }
}
fn entries(root: &File) -> Result<Vec<std::ffi::OsString>> {
    let names = super::candidates::names(root)?;
    ensure!(
        names.len() <= MAX_STORAGE_ENTRIES,
        "local backup directory entry limit exceeded"
    );
    Ok(names)
}
fn scan(root: &File, budget: &Budget) -> Result<RetainedBackupList> {
    let mut archives = Vec::new();
    let mut total = 0u64;
    let mut ids = std::collections::BTreeSet::new();
    for name in entries(root)? {
        budget.check()?;
        let Some(mut row) = name.to_str().and_then(parse) else {
            continue;
        };
        ensure!(ids.insert(row.id.clone()), "duplicate local backup ID");
        let file = super::candidates::open_at(root, &name, false)?;
        let meta = file.metadata()?;
        ensure!(safe_file(&meta), "unsafe local backup archive");
        row.content_length = meta.len();
        total = total.checked_add(meta.len()).context("backup size overflow")?;
        archives.push(row);
        ensure!(
            archives.len() <= MAX_ARCHIVES && total <= MAX_TOTAL_BYTES,
            "local backup store exceeds limits"
        );
    }
    archives.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
    Ok(RetainedBackupList {
        archives,
        total_bytes: total,
        max_archives: MAX_ARCHIVES,
        max_total_bytes: MAX_TOTAL_BYTES,
    })
}
fn unlink(root: &File, name: &str) -> Result<()> {
    let name = CString::new(name)?;
    ensure!(
        crate::secure_fs::unlink_at(root, &name, 0).is_ok(),
        "local backup removal failed"
    );
    Ok(())
}
struct Part<'a> {
    root: &'a File,
    name: String,
}
impl Drop for Part<'_> {
    fn drop(&mut self) {
        let _ = unlink(self.root, &self.name);
    }
}
/// Only private, single-link service-named partial files are eligible. Archives are never pruned.
pub(crate) fn recover(data: &Path) -> Result<()> {
    let budget = Budget::new(watch::channel(false).1);
    let Some(root) = root(data, false)? else {
        return Ok(());
    };
    for name in entries(&root)? {
        budget.check()?;
        let Some(id) = name
            .to_str()
            .and_then(|s| s.strip_prefix('.'))
            .and_then(|s| s.strip_suffix(".part"))
        else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        let Ok(file) = super::candidates::open_at(&root, &name, false) else {
            continue;
        };
        if safe_file(&file.metadata()?) {
            unlink(&root, name.to_str().unwrap())?;
        }
    }
    root.sync_all()?;
    Ok(())
}
pub(crate) enum Output {
    Created(RetainedBackupReceipt),
    Listed(RetainedBackupList),
    Downloaded(BackupMetadata, Vec<u8>),
    Deleted(BackupDeletionReceipt),
}
pub(crate) fn run(
    data: &Path,
    op: super::RetainedOperation,
    snapshot: Option<(BackupMetadata, Vec<u8>)>,
    stop: watch::Receiver<bool>,
) -> Result<Output> {
    let budget = Budget::new(stop);
    budget.check()?;
    let root = root(data, matches!(op, super::RetainedOperation::Create))?;
    let empty = RetainedBackupList {
        archives: Vec::new(),
        total_bytes: 0,
        max_archives: MAX_ARCHIVES,
        max_total_bytes: MAX_TOTAL_BYTES,
    };
    let listing = match &root {
        Some(root) => scan(root, &budget)?,
        None => empty,
    };
    let deleting = matches!(&op, super::RetainedOperation::Delete(_));
    match op {
        super::RetainedOperation::List => Ok(Output::Listed(listing)),
        super::RetainedOperation::Create => {
            let root = root.context("local backup directory missing")?;
            let (metadata, bytes) = snapshot.context("backup snapshot missing")?;
            ensure!(
                bytes.len() <= MAX_ARCHIVE_BYTES
                    && metadata.content_length == bytes.len() as u64
                    && super::hash(&bytes) == metadata.sha256,
                "backup snapshot changed"
            );
            if listing.archives.len() >= MAX_ARCHIVES || listing.total_bytes + bytes.len() as u64 > MAX_TOTAL_BYTES {
                return Err(StorageFull.into());
            }
            let mut random = [0; 16];
            getrandom::fill(&mut random).map_err(|_| anyhow::anyhow!("backup ID generation failed"))?;
            let id = super::hash(&random)[..24].to_owned();
            ensure!(!listing.archives.iter().any(|row| row.id == id), "backup ID collision");
            let row = RetainedBackup {
                id,
                created_at: metadata.created_at,
                content_length: metadata.content_length,
                sha256: metadata.sha256,
            };
            let part_text = format!(".{}.part", row.id);
            let part_name = CString::new(part_text.as_str())?;
            let mut file = crate::secure_fs::open_at(
                &root,
                &part_name,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
            .map_err(|_| anyhow::anyhow!("local backup partial creation failed"))?;
            let _part = Part {
                root: &root,
                name: part_text,
            };
            for chunk in bytes.chunks(64 * 1024) {
                budget.check()?;
                file.write_all(chunk)?;
            }
            file.sync_all()?;
            budget.check()?;
            let final_name = CString::new(filename(&row))?;
            ensure!(
                crate::secure_fs::rename_noreplace_at(&root, &part_name, &final_name).is_ok(),
                "local backup commit failed"
            );
            // Rename is the logical commit; a subsequent directory fsync cannot undo it.
            let durability_pending = root.sync_all().is_err();
            Ok(Output::Created(RetainedBackupReceipt {
                committed: true,
                durability_pending,
                backup: row,
            }))
        }
        super::RetainedOperation::Download(id) | super::RetainedOperation::Delete(id) => {
            ensure!(valid_id(&id), "invalid local backup ID");
            let row = listing.archives.into_iter().find(|row| row.id == id);
            // IDs must remain unique even after administrator edits.
            let Some(row) = row else {
                return if deleting {
                    Ok(Output::Deleted(BackupDeletionReceipt {
                        deleted: false,
                        durability_pending: false,
                    }))
                } else {
                    Err(Missing.into())
                };
            };
            let root = root.context("local backup directory missing")?;
            let filename = filename(&row);
            let file = super::candidates::open_at(&root, OsStr::new(&filename), false)?;
            let before = file.metadata()?;
            ensure!(safe_file(&before), "unsafe local backup archive");
            budget.check()?;
            if deleting {
                unlink(&root, &filename)?;
                return Ok(Output::Deleted(BackupDeletionReceipt {
                    deleted: true,
                    durability_pending: root.sync_all().is_err(),
                }));
            }
            let mut file = file;
            let mut bytes = Vec::new();
            let mut chunk = [0; 64 * 1024];
            loop {
                budget.check()?;
                let count = file.read(&mut chunk)?;
                if count == 0 {
                    break;
                }
                ensure!(
                    bytes.len() + count <= MAX_ARCHIVE_BYTES,
                    "local backup archive exceeds limits"
                );
                bytes.extend_from_slice(&chunk[..count]);
            }
            let after = file.metadata()?;
            let current = super::candidates::open_at(&root, OsStr::new(&filename), false)?.metadata()?;
            ensure!(
                before.dev() == after.dev()
                    && before.ino() == after.ino()
                    && before.len() == after.len()
                    && before.mtime() == after.mtime()
                    && before.mtime_nsec() == after.mtime_nsec()
                    && before.ctime() == after.ctime()
                    && before.ctime_nsec() == after.ctime_nsec()
                    && before.dev() == current.dev()
                    && before.ino() == current.ino()
                    && bytes.len() as u64 == row.content_length
                    && super::hash(&bytes) == row.sha256,
                "local backup archive changed"
            );
            super::inspect::verify(&bytes, budget.stop.clone(), budget.stop.clone())?;
            budget.check()?;
            Ok(Output::Downloaded(
                BackupMetadata {
                    filename: format!("mihomo-server-{}.zip", row.id),
                    created_at: row.created_at,
                    content_length: row.content_length,
                    sha256: row.sha256,
                },
                bytes,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new() -> Result<Self> {
            let mut random = [0; 16];
            getrandom::fill(&mut random).unwrap();
            let path = std::env::temp_dir().join(format!("ms-storage-tests-{}", crate::backup::hash(&random)));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn row(id: char) -> RetainedBackup {
        RetainedBackup {
            id: id.to_string().repeat(24),
            created_at: 42,
            content_length: 0,
            sha256: "0".repeat(64),
        }
    }
    fn file(data: &Path, name: &str, length: u64) -> Result<()> {
        root(data, true)?;
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(data.join(ROOT).join(name))?;
        output.set_len(length)?;
        Ok(())
    }
    #[test]
    fn total_capacity_and_directory_enumeration_are_bounded_without_pruning() -> Result<()> {
        let data = Temp::new()?;
        for id in ['a', 'b', 'c', 'd'] {
            file(&data.0, &filename(&row(id)), 64 * 1024 * 1024)?;
        }
        let bytes = b"snapshot".to_vec();
        let metadata = BackupMetadata {
            filename: "ignored.zip".into(),
            created_at: 1,
            content_length: bytes.len() as u64,
            sha256: super::super::hash(&bytes),
        };
        let result = run(
            &data.0,
            super::super::RetainedOperation::Create,
            Some((metadata, bytes)),
            watch::channel(false).1,
        );
        assert!(result.err().unwrap().downcast_ref::<StorageFull>().is_some());
        assert_eq!(fs::read_dir(data.0.join(ROOT))?.count(), 4);
        for i in 0..125 {
            file(&data.0, &format!("unknown-{i}"), 0)?;
        }
        assert!(
            run(
                &data.0,
                super::super::RetainedOperation::List,
                None,
                watch::channel(false).1
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(data.0.join(ROOT))?.count(), 129);
        Ok(())
    }
    #[test]
    fn startup_partial_recovery_preserves_committed_unknown_and_unsafe_files() -> Result<()> {
        let data = Temp::new()?;
        let outside = Temp::new()?;
        let partial = format!(".{}.part", "a".repeat(24));
        file(&data.0, &partial, 10)?;
        let committed = filename(&row('b'));
        file(&data.0, &committed, 10)?;
        file(&data.0, ".unknown.part", 10)?;
        let shared = format!(".{}.part", "c".repeat(24));
        file(&data.0, &shared, 10)?;
        fs::hard_link(data.0.join(ROOT).join(&shared), outside.0.join("shared"))?;
        let linked = format!(".{}.part", "d".repeat(24));
        fs::write(outside.0.join("target"), b"keep")?;
        symlink(outside.0.join("target"), data.0.join(ROOT).join(&linked))?;
        recover(&data.0)?;
        assert!(!data.0.join(ROOT).join(partial).exists());
        for name in [committed, ".unknown.part".into(), shared, linked] {
            assert!(fs::symlink_metadata(data.0.join(ROOT).join(name)).is_ok());
        }
        assert_eq!(fs::read(outside.0.join("target"))?, b"keep");
        Ok(())
    }
    #[test]
    fn unsafe_root_duplicate_ids_and_shutdown_fail_without_mutating_store() -> Result<()> {
        let data = Temp::new()?;
        let outside = Temp::new()?;
        symlink(&outside.0, data.0.join(ROOT))?;
        assert!(recover(&data.0).is_err());
        assert!(
            run(
                &data.0,
                super::super::RetainedOperation::List,
                None,
                watch::channel(false).1
            )
            .is_err()
        );
        fs::remove_file(data.0.join(ROOT))?;
        root(&data.0, true)?;
        fs::set_permissions(data.0.join(ROOT), fs::Permissions::from_mode(0o755))?;
        assert!(recover(&data.0).is_err());
        fs::set_permissions(data.0.join(ROOT), fs::Permissions::from_mode(0o700))?;
        let first = row('a');
        let mut second = first.clone();
        second.created_at += 1;
        file(&data.0, &filename(&first), 10)?;
        file(&data.0, &filename(&second), 10)?;
        assert!(
            run(
                &data.0,
                super::super::RetainedOperation::List,
                None,
                watch::channel(false).1
            )
            .is_err()
        );
        assert!(
            run(
                &data.0,
                super::super::RetainedOperation::Delete(first.id),
                None,
                watch::channel(true).1
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(data.0.join(ROOT))?.count(), 2);
        Ok(())
    }
}
