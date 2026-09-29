//! Crash-recoverable rollback record for a live Geo replacement.
//! The data-directory lock is held by the caller. A pending marker is durable
//! before publication; removing that marker is the commit point.
use super::validation;
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

const DIR: &str = ".geo-live";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    name: String,
    previous_sha256: Option<String>,
    candidate_sha256: String,
}

fn hash(data: &Path, name: &str) -> Result<Option<String>> {
    Ok(validation::snapshot(data, name)?.map(|bytes| validation::sha256(&bytes)))
}

use crate::secure_fs::sync_directory;

fn directory(data: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    let path = data.join(DIR);
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(&path) {
        Ok(()) => sync_directory(data)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let meta = fs::symlink_metadata(&path)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink() && meta.permissions().mode() & 0o077 == 0,
        "Geo live journal must be a private directory"
    );
    Ok(path)
}

fn validate_name(name: &str) -> Result<()> {
    ensure!(
        validation::MMDB_FILES.contains(&name) || crate::geo::dat::DAT_FILES.contains(&name),
        "unsupported live Geo filename"
    );
    Ok(())
}

fn validate_hash(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid live Geo digest"
    );
    Ok(())
}

pub(crate) fn begin(data: &Path, name: &str, previous: Option<&str>, candidate: &str) -> Result<()> {
    validate_name(name)?;
    validate_hash(candidate)?;
    if let Some(previous) = previous {
        validate_hash(previous)?;
    }
    ensure!(
        hash(data, name)?.as_deref() == previous,
        "Geo changed before live publication"
    );
    let path = directory(data)?;
    ensure!(
        fs::read_dir(&path)?.next().is_none(),
        "unfinished Geo live journal requires recovery"
    );
    if let Some(previous) = previous {
        // A copy is required: Mihomo may still mutate its Geo inode before it is reaped.
        let bytes = validation::snapshot(data, name)?.context("Geo rollback source disappeared")?;
        ensure!(
            validation::sha256(&bytes) == previous,
            "Geo changed while copying rollback file"
        );
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut old = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path.join(name))?;
        old.write_all(&bytes)?;
        old.sync_all()?;
        ensure!(
            hash(&path, name)?.as_deref() == Some(previous),
            "Geo rollback copy changed"
        );
        sync_directory(&path)?;
    }
    let pending = Pending {
        name: name.into(),
        previous_sha256: previous.map(str::to_owned),
        candidate_sha256: candidate.into(),
    };
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path.join("pending"))?;
    marker.write_all(&serde_json::to_vec(&pending)?)?;
    marker.sync_all()?;
    sync_directory(&path)?;
    Ok(())
}

fn clean(path: &Path, data: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("invalid Geo journal entry")?;
        validate_name(name)?;
        fs::remove_file(entry.path())?;
    }
    fs::remove_dir(path)?;
    sync_directory(data)
}

/// Returns (journal directory durable, post-commit cleanup pending).
/// Once the marker is removed, a cleanup/sync error cannot safely trigger rollback.
pub(crate) fn commit(data: &Path) -> Result<(bool, bool)> {
    let path = directory(data)?;
    ensure!(path.join("pending").try_exists()?, "Geo live journal is missing");
    fs::remove_file(path.join("pending"))?;
    let durable = sync_directory(&path).is_ok();
    Ok((durable, clean(&path, data).is_err()))
}

pub(crate) fn recover(data: &Path) -> Result<bool> {
    let path = data.join(DIR);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let path = directory(data)?;
    let marker = path.join("pending");
    match fs::symlink_metadata(&marker) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            clean(&path, data)?;
            return Ok(false);
        }
        Err(error) => return Err(error.into()),
        Ok(meta) => ensure!(
            meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 512,
            "unsafe Geo live journal marker"
        ),
    }
    let bytes = fs::read(&marker)?;
    ensure!(bytes.len() <= 512, "Geo live journal is oversized");
    let pending: Pending = serde_json::from_slice(&bytes).context("invalid Geo live journal")?;
    validate_name(&pending.name)?;
    validate_hash(&pending.candidate_sha256)?;
    if let Some(old) = &pending.previous_sha256 {
        validate_hash(old)?;
    }
    let actual = hash(data, &pending.name)?;
    ensure!(
        actual.as_deref() == pending.previous_sha256.as_deref() || actual.as_deref() == Some(&pending.candidate_sha256),
        "Geo live file changed outside recovery"
    );
    if let Some(old) = &pending.previous_sha256 {
        ensure!(
            hash(&path, &pending.name)?.as_deref() == Some(old),
            "Geo rollback copy is missing or changed"
        );
        if actual.as_deref() != Some(old) {
            fs::rename(path.join(&pending.name), data.join(&pending.name))?;
            sync_directory(data)?;
        }
    } else if actual.is_some() {
        fs::remove_file(data.join(&pending.name))?;
        sync_directory(data)?;
    }
    fs::remove_file(path.join("pending"))?;
    sync_directory(&path)?;
    clean(&path, data)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Data(PathBuf);
    impl Data {
        fn new() -> Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "ms-geo-live-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }
    impl Drop for Data {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn pending_replacement_rolls_back_after_interruption() -> Result<()> {
        let data = Data::new()?;
        let name = "Country.mmdb";
        let old = validation::sha256(b"previous");
        let new = validation::sha256(b"candidate");
        fs::write(data.0.join(name), b"previous")?;
        begin(&data.0, name, Some(&old), &new)?;
        fs::write(data.0.join("candidate"), b"candidate")?;
        fs::rename(data.0.join("candidate"), data.0.join(name))?;
        assert!(recover(&data.0)?);
        assert_eq!(fs::read(data.0.join(name))?, b"previous");
        assert!(!data.0.join(DIR).exists());
        Ok(())
    }

    #[test]
    fn missing_previous_rolls_back_and_committed_candidate_survives() -> Result<()> {
        let data = Data::new()?;
        let name = "geoip.dat";
        let new = validation::sha256(b"candidate");
        begin(&data.0, name, None, &new)?;
        fs::write(data.0.join(name), b"candidate")?;
        assert!(recover(&data.0)?);
        assert!(!data.0.join(name).exists());
        begin(&data.0, name, None, &new)?;
        fs::write(data.0.join(name), b"candidate")?;
        assert_eq!(commit(&data.0)?, (true, false));
        assert!(!recover(&data.0)?);
        assert_eq!(fs::read(data.0.join(name))?, b"candidate");
        Ok(())
    }

    #[test]
    fn outside_change_blocks_automatic_rollback() -> Result<()> {
        let data = Data::new()?;
        let name = "geosite.dat";
        let old = validation::sha256(b"previous");
        let new = validation::sha256(b"candidate");
        fs::write(data.0.join(name), b"previous")?;
        begin(&data.0, name, Some(&old), &new)?;
        fs::write(data.0.join("other"), b"external")?;
        fs::rename(data.0.join("other"), data.0.join(name))?;
        assert!(recover(&data.0).is_err());
        assert_eq!(fs::read(data.0.join(name))?, b"external");
        assert!(data.0.join(DIR).exists());
        Ok(())
    }
}
