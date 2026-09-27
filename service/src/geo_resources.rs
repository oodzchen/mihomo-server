//! Integrity-pinned first-use Geo seeding. Existing data is authoritative.
use anyhow::{Context as _, Result, ensure};
use headless_core::config::resource_paths::GEO_ASSETS;
use ring::digest::{Context, SHA256};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::Path,
};

pub(crate) const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const STAGE: &str = ".geo-seed";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Seed {
    pub bytes: u64,
    pub sha256: String,
}

pub(crate) fn validate_manifest(seeds: &BTreeMap<String, Seed>) -> Result<()> {
    ensure!(seeds.len() <= GEO_ASSETS.len(), "too many Geo seed entries");
    let mut total = 0;
    for (name, seed) in seeds {
        ensure!(GEO_ASSETS.contains(&name.as_str()), "unsupported Geo seed filename");
        ensure!(
            seed.bytes > 0 && seed.bytes <= MAX_FILE_BYTES,
            "Geo seed size must be 1 byte to 128 MiB"
        );
        ensure!(
            seed.sha256.len() == 64 && seed.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid Geo seed SHA-256"
        );
        total += seed.bytes;
    }
    ensure!(total <= MAX_TOTAL_BYTES, "Geo seeds exceed 256 MiB total");
    Ok(())
}

/// Caller owns the data-directory lock. All needed seeds are verified before publication.
/// SHA-256 verifies pinned content integrity, not a Geo format's semantic validity.
pub(crate) fn initialize(source: &Path, data: &Path, seeds: &BTreeMap<String, Seed>) -> Result<Vec<String>> {
    let mut needed = Vec::new();
    for (name, seed) in seeds {
        if !existing(&data.join(name))? {
            needed.push((name, seed));
        }
    }
    let stage = data.join(STAGE);
    if needed.is_empty() && !stage.try_exists()? {
        return Ok(Vec::new());
    }
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(&stage) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&stage)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink() && metadata.permissions().mode() & 0o077 == 0,
        "Geo staging directory must be private and real"
    );
    cleanup(&stage)?;
    let result = (|| {
        for (name, seed) in &needed {
            copy(source, &stage, name, seed)?;
        }
        let mut installed = Vec::new();
        for (name, _) in &needed {
            match fs::hard_link(stage.join(name), data.join(name)) {
                Ok(()) => {
                    installed.push((*name).clone());
                    File::open(data)?.sync_all()?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    existing(&data.join(name))?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(installed)
    })();
    let cleaned = cleanup(&stage).and_then(|_| fs::remove_dir(&stage).map_err(Into::into));
    match (result, cleaned) {
        (Ok(installed), Ok(())) => Ok(installed),
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error.context("Geo staging cleanup failed")),
    }
}

fn existing(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() > 0
                    && metadata.len() <= MAX_FILE_BYTES,
                "existing Geo resource must be a nonempty regular file up to 128 MiB"
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn cleanup(stage: &Path) -> Result<()> {
    // Fixed six-entry namespace. Inspect every entry before removing any orphan.
    let entries: Vec<_> = fs::read_dir(stage)?
        .take(GEO_ASSETS.len() + 1)
        .collect::<std::io::Result<_>>()?;
    ensure!(entries.len() <= GEO_ASSETS.len(), "unexpected Geo staging entries");
    for entry in &entries {
        let name = entry.file_name();
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            name.to_str().is_some_and(|name| GEO_ASSETS.contains(&name))
                && metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= MAX_FILE_BYTES,
            "unsafe Geo staging entry"
        );
    }
    for entry in entries {
        fs::remove_file(entry.path())?;
    }
    Ok(())
}

fn copy(source: &Path, stage: &Path, name: &str, seed: &Seed) -> Result<()> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt as _,
        },
    };
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(source)
        .context("open bundled Geo directory")?;
    let name_c = CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name_c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    ensure!(fd >= 0, "cannot open bundled Geo file without following links");
    let mut input = unsafe { File::from_raw_fd(fd) };
    let metadata = input.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() == seed.bytes,
        "bundled Geo file size/type mismatch"
    );
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(stage.join(name))?;
    let mut hash = Context::new(&SHA256);
    let mut buffer = [0_u8; 65536];
    let mut bytes = 0;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        ensure!(bytes <= seed.bytes, "bundled Geo file changed size during copy");
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    let actual: String = hash
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    ensure!(
        bytes == seed.bytes && actual.eq_ignore_ascii_case(&seed.sha256),
        "bundled Geo SHA-256 mismatch"
    );
    output.sync_all()?;
    Ok(())
}
