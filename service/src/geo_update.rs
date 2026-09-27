//! Explicit stopped-core replacement from integrity-pinned bundle resources.
use crate::{
    geo_resources::{self, Seed},
    geo_validation::{self, Validation},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Debug, Serialize)]
pub struct SeedInfo {
    pub name: String,
    pub current_sha256: Option<String>,
    pub seed_sha256: String,
    pub seed_bytes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    pub name: String,
    pub expected_current_sha256: Option<String>,
    pub expected_seed_sha256: String,
    #[serde(default)]
    pub accept_metadata_only: bool,
}

#[derive(Debug, Serialize)]
pub struct Receipt {
    pub previous_sha256: Option<String>,
    pub validation: Validation,
    pub changed: bool,
    pub durable: bool,
    pub cleanup_pending: bool,
}

fn current(data: &Path, name: &str) -> Result<Option<String>> {
    Ok(geo_validation::snapshot(data, name)?.map(|bytes| geo_validation::sha256(&bytes)))
}
pub(crate) fn info(data: &Path, name: &str, seed: &Seed) -> Result<SeedInfo> {
    Ok(SeedInfo {
        name: name.into(),
        current_sha256: current(data, name)?,
        seed_sha256: seed.sha256.to_ascii_lowercase(),
        seed_bytes: seed.bytes,
    })
}
fn hash_matches(expected: &str, actual: &str) -> bool {
    expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()) && expected.eq_ignore_ascii_case(actual)
}
fn check_current(data: &Path, request: &InstallRequest) -> Result<Option<String>> {
    let actual = current(data, &request.name)?;
    ensure!(
        match (&request.expected_current_sha256, &actual) {
            (None, None) => true,
            (Some(expected), Some(actual)) => hash_matches(expected, actual),
            _ => false,
        },
        "Geo file changed since update inspection; read update information again"
    );
    Ok(actual)
}

/// Caller owns the data lock and guarantees a stopped, reaped core. Rename is
/// the commit boundary; crash recovery only cleans unpublished/empty staging.
/// External writers must obey the same data lock; this is not an OS-level CAS.
pub(crate) fn install(source: &Path, data: &Path, seed: &Seed, request: &InstallRequest) -> Result<Receipt> {
    ensure!(
        hash_matches(&request.expected_seed_sha256, &seed.sha256),
        "bundled Geo pin changed; read update information again"
    );
    let previous = check_current(data, request)?;
    let stage = geo_resources::prepare_stage(data)?;
    let staged = (|| {
        geo_resources::copy(source, &stage, &request.name, seed)?;
        let validation = geo_validation::validate(&stage, &request.name)?;
        ensure!(
            hash_matches(&validation.sha256, &seed.sha256) && validation.bytes == seed.bytes,
            "staged Geo pin mismatch"
        );
        ensure!(
            validation.verified || request.accept_metadata_only,
            "bundled MMDB has an empty description and full structure is unverified; explicit accept_metadata_only is required"
        );
        check_current(data, request)?;
        Ok::<_, anyhow::Error>(validation)
    })();
    let result = staged.and_then(|validation| {
        // Identical content needs no file replacement, but still checks the source.
        let changed = previous.as_deref() != Some(validation.sha256.as_str());
        if changed {
            publish(&stage, data, &request.name, previous.is_some())?;
        }
        Ok(Receipt {
            previous_sha256: previous,
            validation,
            changed,
            durable: std::fs::File::open(data).and_then(|dir| dir.sync_all()).is_ok(),
            cleanup_pending: false,
        })
    });
    let cleaned = geo_resources::cleanup(&stage).and_then(|_| fs::remove_dir(&stage).map_err(Into::into));
    match result {
        Ok(mut receipt) => {
            receipt.cleanup_pending = cleaned.is_err();
            Ok(receipt)
        }
        Err(error) => Err(error),
    }
}

fn publish(stage: &Path, data: &Path, name: &str, replacing: bool) -> Result<()> {
    use std::{
        ffi::CString,
        fs::OpenOptions,
        os::{fd::AsRawFd, unix::fs::OpenOptionsExt as _},
    };
    let directory = |path| {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
    };
    let source = directory(stage)?;
    let destination = directory(data)?;
    let name = CString::new(name)?;
    let result = unsafe {
        if replacing {
            libc::renameat(
                source.as_raw_fd(),
                name.as_ptr(),
                destination.as_raw_fd(),
                name.as_ptr(),
            )
        } else {
            libc::linkat(
                source.as_raw_fd(),
                name.as_ptr(),
                destination.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        }
    };
    ensure!(result == 0, "Geo publication failed; previous file retained");
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::geo_validation::tests::fixture_with_description;
    use std::{
        collections::BTreeMap,
        os::unix::fs::{PermissionsExt as _, symlink},
        path::PathBuf,
    };
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "ms-geo-update-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
            fs::create_dir(&path)?;
            fs::create_dir(path.join("source"))?;
            fs::create_dir(path.join("data"))?;
            Ok(Self(path))
        }
        fn setup(&self, description: bool) -> Result<(Seed, InstallRequest)> {
            let bytes = fixture_with_description(description);
            fs::write(self.0.join("source/Country.mmdb"), &bytes)?;
            let seed = Seed {
                bytes: bytes.len() as u64,
                sha256: geo_validation::sha256(&bytes),
            };
            Ok((
                seed.clone(),
                InstallRequest {
                    name: "Country.mmdb".into(),
                    expected_current_sha256: None,
                    expected_seed_sha256: seed.sha256,
                    accept_metadata_only: false,
                },
            ))
        }
        fn install(&self, seed: &Seed, request: &InstallRequest) -> Result<Receipt> {
            install(&self.0.join("source"), &self.0.join("data"), seed, request)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn missing_install_and_guarded_repair_are_atomic_private_and_idempotent() -> Result<()> {
        let dir = Directory::new()?;
        let (seed, mut request) = dir.setup(true)?;
        let receipt = dir.install(&seed, &request)?;
        assert!(receipt.changed && receipt.durable && receipt.validation.verified && !receipt.cleanup_pending);
        assert!(receipt.previous_sha256.is_none());
        let path = dir.0.join("data/Country.mmdb");
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        assert!(!dir.0.join("data/.geo-seed").exists());
        assert!(dir.install(&seed, &request).is_err()); // None never overwrites an existing file.
        request.expected_current_sha256 = Some(seed.sha256.clone());
        assert!(!dir.install(&seed, &request)?.changed);
        fs::write(&path, "corrupt old Geo")?;
        fs::hard_link(&path, dir.0.join("old-hardlink"))?;
        request.expected_current_sha256 = Some(geo_validation::sha256(b"corrupt old Geo"));
        let receipt = dir.install(&seed, &request)?;
        assert!(receipt.changed && receipt.validation.verified);
        assert_eq!(fs::read(dir.0.join("old-hardlink"))?, b"corrupt old Geo");
        assert_eq!(current(&dir.0.join("data"), "Country.mmdb")?, Some(seed.sha256));
        Ok(())
    }
    #[test]
    fn stale_inputs_bad_pins_formats_and_links_preserve_the_old_resource() -> Result<()> {
        let dir = Directory::new()?;
        let (seed, mut request) = dir.setup(true)?;
        let path = dir.0.join("data/Country.mmdb");
        fs::write(&path, b"old")?;
        assert!(dir.install(&seed, &request).is_err());
        request.expected_current_sha256 = Some(geo_validation::sha256(b"old"));
        request.expected_seed_sha256 = "0".repeat(64);
        assert!(dir.install(&seed, &request).is_err());
        request.expected_seed_sha256 = seed.sha256.clone();
        fs::write(dir.0.join("source/Country.mmdb"), b"invalid")?;
        assert!(dir.install(&seed, &request).is_err());
        let bad_seed = Seed {
            bytes: 7,
            sha256: geo_validation::sha256(b"invalid"),
        };
        request.expected_seed_sha256 = bad_seed.sha256.clone();
        assert!(dir.install(&bad_seed, &request).is_err()); // Hash integrity alone is insufficient.
        assert_eq!(fs::read(&path)?, b"old");
        assert!(!dir.0.join("data/.geo-seed").exists());
        fs::remove_file(dir.0.join("source/Country.mmdb"))?;
        symlink(&path, dir.0.join("source/Country.mmdb"))?;
        assert!(dir.install(&bad_seed, &request).is_err());
        fs::remove_file(&path)?;
        symlink("/etc/passwd", &path)?;
        assert!(dir.install(&seed, &request).is_err());
        request.name = "../Country.mmdb".into();
        assert!(dir.install(&seed, &request).is_err());
        Ok(())
    }
    #[test]
    fn metadata_only_candidate_requires_explicit_acceptance_and_retains_warning() -> Result<()> {
        let dir = Directory::new()?;
        let (seed, mut request) = dir.setup(false)?;
        assert!(dir.install(&seed, &request).is_err());
        assert!(!dir.0.join("data/Country.mmdb").exists());
        request.accept_metadata_only = true;
        let receipt = dir.install(&seed, &request)?;
        assert!(receipt.changed && !receipt.validation.verified);
        assert_eq!(
            receipt.validation.warning,
            Some("empty_description_structure_unverified")
        );
        Ok(())
    }
    #[test]
    fn startup_recovers_unpublished_update_without_replacing_authoritative_data() -> Result<()> {
        let dir = Directory::new()?;
        let (seed, _) = dir.setup(true)?;
        let data = dir.0.join("data");
        fs::write(data.join("Country.mmdb"), "authoritative old file")?;
        let stage = geo_resources::prepare_stage(&data)?;
        geo_resources::copy(&dir.0.join("source"), &stage, "Country.mmdb", &seed)?;
        let seeds = BTreeMap::from([("Country.mmdb".into(), seed)]);
        assert!(geo_resources::initialize(&dir.0.join("source"), &data, &seeds)?.is_empty());
        assert_eq!(fs::read(data.join("Country.mmdb"))?, b"authoritative old file");
        assert!(!stage.exists());
        let stage = geo_resources::prepare_stage(&data)?;
        fs::write(stage.join("unknown"), "retain")?;
        assert!(geo_resources::initialize(&dir.0.join("source"), &data, &seeds).is_err());
        assert_eq!(fs::read(stage.join("unknown"))?, b"retain");
        Ok(())
    }
}
