//! Bounded ZIP export, inspection and disposable restore rehearsal. No archive is retained.
use headless_core::backup::BackupMetadata;

#[path = "backup_inspect.rs"]
pub(crate) mod inspect;

#[cfg(unix)]
#[path = "backup_restore.rs"]
pub(crate) mod restore;

pub(crate) fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub struct BackupDownload {
    pub metadata: BackupMetadata,
    pub bytes: Vec<u8>,
    pub(crate) permit: tokio::sync::OwnedSemaphorePermit,
}

#[derive(Debug)]
pub(crate) struct RestoreNeedsStopped;
impl std::fmt::Display for RestoreNeedsStopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stop the core before restoring a backup")
    }
}
impl std::error::Error for RestoreNeedsStopped {}

#[cfg(unix)]
pub(crate) mod export {
    pub(crate) use super::hash;
    use anyhow::{Context as _, Result, ensure};
    use headless_core::{
        backup::{
            BackupEntry, BackupManifest, MAX_ARCHIVE_BYTES, MAX_CONTENT_BYTES, MAX_ENTRIES, MAX_FILE_BYTES,
            validate_filename,
        },
        config::{IProfiles, runtime, settings::ServiceSettings},
    };
    use std::{
        collections::BTreeSet,
        ffi::CString,
        fs::{File, Metadata, OpenOptions},
        io::{Cursor, Read as _, Seek, SeekFrom, Write},
        os::{
            fd::{AsRawFd as _, FromRawFd as _},
            unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
        },
        path::{Path, PathBuf},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use tokio::sync::watch;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    pub(crate) struct Snapshot {
        pub data_dir: PathBuf,
        pub runtime_path: PathBuf,
        pub profiles: IProfiles,
        pub settings: ServiceSettings,
        pub runtime_revision: Option<String>,
        pub active_profile: Option<String>,
    }
    struct Budget {
        deadline: Instant,
        shutdown: watch::Receiver<bool>,
    }
    impl Budget {
        fn check(&self) -> Result<()> {
            ensure!(!*self.shutdown.borrow(), "backup cancelled during shutdown");
            ensure!(Instant::now() < self.deadline, "backup export deadline exceeded");
            Ok(())
        }
    }
    fn safe_metadata(file: &File, directory: bool) -> Result<Metadata> {
        let meta = file.metadata()?;
        ensure!(
            meta.uid() == unsafe { libc::geteuid() }
                && meta.permissions().mode() & 0o7022 == 0
                && if directory {
                    meta.is_dir()
                } else {
                    meta.is_file() && meta.nlink() == 1
                },
            "unsafe backup source"
        );
        Ok(meta)
    }
    fn open(path: &Path, directory: bool) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(
                libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | if directory { libc::O_DIRECTORY } else { 0 },
            )
            .open(path)?;
        safe_metadata(&file, directory)?;
        Ok(file)
    }
    fn open_at(parent: &File, name: &str, directory: bool) -> Result<File> {
        let name = CString::new(name)?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC
                    | if directory { libc::O_DIRECTORY } else { 0 },
            )
        };
        ensure!(fd >= 0, "cannot open backup source");
        let file = unsafe { File::from_raw_fd(fd) };
        safe_metadata(&file, directory)?;
        Ok(file)
    }
    fn same(a: &Metadata, b: &Metadata) -> bool {
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    fn read(mut file: File, budget: &Budget) -> Result<(Vec<u8>, Metadata)> {
        let before = safe_metadata(&file, false)?;
        ensure!(before.len() <= MAX_FILE_BYTES, "backup entry exceeds 8 MiB");
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        loop {
            budget.check()?;
            let count = file.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            ensure!(
                bytes.len() + count <= MAX_FILE_BYTES as usize,
                "backup entry exceeds 8 MiB"
            );
            bytes.extend_from_slice(&chunk[..count]);
        }
        ensure!(
            same(&before, &safe_metadata(&file, false)?) && bytes.len() as u64 == before.len(),
            "backup source changed during read"
        );
        Ok((bytes, before))
    }
    struct LimitedCursor(Cursor<Vec<u8>>);
    impl Write for LimitedCursor {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.position().saturating_add(bytes.len() as u64) > MAX_ARCHIVE_BYTES as u64 {
                return Err(std::io::Error::other("backup archive exceeds 65 MiB"));
            }
            self.0.write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }
    impl Seek for LimitedCursor {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.0.seek(pos)
        }
    }
    pub(crate) fn build(
        snapshot: Snapshot,
        shutdown: watch::Receiver<bool>,
    ) -> Result<(super::BackupMetadata, Vec<u8>)> {
        let budget = Budget {
            deadline: Instant::now() + Duration::from_secs(15),
            shutdown,
        };
        budget.check()?;
        snapshot.settings.validate()?;
        let root = open(&snapshot.data_dir, true)?;
        let profiles = open_at(&root, "profiles", true)?;
        let profile_identity = profiles.metadata()?;
        let mut names = BTreeSet::new();
        for item in snapshot.profiles.items.iter().flatten() {
            budget.check()?;
            if let Some(name) = &item.file {
                validate_filename(name)?;
                names.insert(name.to_string());
            }
            ensure!(names.len() + 4 <= MAX_ENTRIES, "backup has too many entries");
        }
        let (runtime_bytes, runtime_identity) = read(open(&snapshot.runtime_path, false)?, &budget)?;
        let yaml = std::str::from_utf8(&runtime_bytes).context("invalid runtime encoding")?;
        let clean_runtime = serde_yaml_ng::to_string(&runtime::parse(yaml)?)?.into_bytes();
        let mut contents = vec![
            (
                "profiles.yaml".to_owned(),
                serde_yaml_ng::to_string(&snapshot.profiles)?.into_bytes(),
            ),
            (
                "settings.yaml".to_owned(),
                serde_yaml_ng::to_string(&snapshot.settings)?.into_bytes(),
            ),
            ("runtime.yaml".to_owned(), clean_runtime),
        ];
        let mut total = contents.iter().map(|(_, b)| b.len() as u64).sum::<u64>();
        let mut identities = Vec::new();
        for name in names {
            let source = open_at(&profiles, &name, false)?;
            ensure!(
                total + source.metadata()?.len() <= MAX_CONTENT_BYTES,
                "backup content exceeds 64 MiB"
            );
            let (bytes, identity) = read(source, &budget)?;
            total += bytes.len() as u64;
            ensure!(total <= MAX_CONTENT_BYTES, "backup content exceeds 64 MiB");
            contents.push((format!("profiles/{name}"), bytes));
            identities.push((name, identity));
        }
        let created_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let manifest = BackupManifest {
            schema_version: 1,
            source: "mihomo-server".into(),
            service_version: env!("CARGO_PKG_VERSION").into(),
            created_at,
            active_profile: snapshot.active_profile,
            runtime_revision: snapshot.runtime_revision,
            entries: contents
                .iter()
                .map(|(path, bytes)| BackupEntry {
                    path: path.clone(),
                    bytes: bytes.len() as u64,
                    sha256: hash(bytes),
                })
                .collect(),
        };
        manifest.validate()?;
        contents.push(("manifest.json".into(), serde_json::to_vec(&manifest)?));
        let mut zip = ZipWriter::new(LimitedCursor(Cursor::new(Vec::new())));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o600);
        for (name, bytes) in contents {
            budget.check()?;
            zip.start_file(name, options)?;
            for chunk in bytes.chunks(64 * 1024) {
                budget.check()?;
                zip.write_all(chunk)?;
            }
        }
        let bytes = zip.finish()?.0.into_inner();
        budget.check()?;
        // Detect external source replacement/mutation before publishing a completed archive.
        ensure!(
            same(
                &runtime_identity,
                &safe_metadata(&open(&snapshot.runtime_path, false)?, false)?
            ),
            "runtime changed during backup"
        );
        ensure!(
            same(&profile_identity, &open_at(&root, "profiles", true)?.metadata()?),
            "profiles directory changed during backup"
        );
        for (name, identity) in identities {
            budget.check()?;
            ensure!(
                same(&identity, &safe_metadata(&open_at(&profiles, &name, false)?, false)?),
                "profile changed during backup"
            );
        }
        let mut random = [0; 8];
        getrandom::fill(&mut random).map_err(|_| anyhow::anyhow!("backup ID generation failed"))?;
        let id: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let metadata = super::BackupMetadata {
            filename: format!("mihomo-server-backup-{created_at}-{id}.zip"),
            created_at,
            content_length: bytes.len() as u64,
            sha256: hash(&bytes),
        };
        Ok((metadata, bytes))
    }
}

#[cfg(all(test, unix))]
#[path = "backup_tests.rs"]
mod tests;
