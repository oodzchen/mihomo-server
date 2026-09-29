//! Explicit read-only MMDB/DAT verification; no database records are returned.
use anyhow::{Result, ensure};
use serde::Serialize;
use std::path::Path;

pub use crate::dat_validation::Statistics as DatStatistics;

pub const MMDB_FILES: [&str; 3] = ["Country.mmdb", "ASN.mmdb", "geoip.metadb"];
const MAX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_NODES: u32 = 2_000_000;

#[derive(Debug, Serialize)]
pub struct Validation {
    pub name: String,
    pub format: &'static str,
    pub verified: bool,
    pub warning: Option<&'static str>,
    pub bytes: u64,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_version: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dat: Option<DatStatistics>,
}

/// Caller serializes against service resource/lifecycle mutations. The snapshot is
/// bounded and read through no-follow descriptors; validation does not modify it.
#[cfg(unix)]
pub(crate) fn snapshot(root: &Path, name: &str) -> Result<Option<Vec<u8>>> {
    use std::{
        ffi::CString,
        fs::OpenOptions,
        io::Read as _,
        os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    };
    ensure!(
        MMDB_FILES.contains(&name) || crate::dat_validation::DAT_FILES.contains(&name),
        "unsupported Geo validation filename"
    );
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let leaf = CString::new(name)?;
    let mut file = match crate::secure_fs::open_at(
        &directory,
        &leaf,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        0,
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => anyhow::bail!("Geo file unreadable or linked"),
    };
    let before = file.metadata()?;
    ensure!(
        before.is_file() && before.len() <= MAX_BYTES,
        "Geo reads require a regular file up to 128 MiB"
    );
    let mut bytes = Vec::new();
    (&mut file).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    ensure!(
        bytes.len() as u64 == before.len()
            && before.len() == after.len()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec(),
        "Geo file changed while reading; retry validation"
    );
    Ok(Some(bytes))
}

pub(crate) use crate::secure_fs::sha256_hex as sha256;

#[cfg(unix)]
pub(crate) fn validate(root: &Path, name: &str) -> Result<Validation> {
    let bytes = snapshot(root, name)?.ok_or_else(|| anyhow::anyhow!("Geo file missing"))?;
    ensure!(!bytes.is_empty(), "Geo validation requires a nonempty file");
    let size = bytes.len() as u64;
    let hash = sha256(&bytes);
    if crate::dat_validation::DAT_FILES.contains(&name) {
        let dat = crate::dat_validation::validate(&bytes, name)?;
        let verified = dat.unknown_field_count == 0;
        return Ok(Validation {
            name: name.into(),
            format: "dat",
            verified,
            warning: Some(if !verified {
                "dat_unknown_fields_unverified"
            } else if !dat.has_cn_group {
                "dat_cn_group_missing"
            } else {
                "dat_core_matching_unverified"
            }),
            bytes: size,
            sha256: hash,
            ip_version: None,
            node_count: None,
            build_epoch: None,
            dat: Some(dat),
        });
    }
    let reader = maxminddb::Reader::from_source(bytes)
        .map_err(|_| anyhow::anyhow!("invalid MMDB metadata or search tree header"))?;
    ensure!(
        reader.metadata().node_count > 0 && reader.metadata().node_count <= MAX_NODES,
        "MMDB search tree exceeds validation limits or is empty"
    );
    // Mihomo's real MetaDB files can omit descriptions. The strict verifier
    // stops before checking their tree/records: report this limit, never success.
    let warning = match reader.verify() {
        Ok(()) => None,
        Err(maxminddb::MaxMindDbError::InvalidDatabase { message, .. })
            if message == "description - Expected: non-empty map Actual: {}" =>
        {
            Some("empty_description_structure_unverified")
        }
        Err(_) => anyhow::bail!("MMDB structural verification failed"),
    };
    let metadata = reader.metadata();
    Ok(Validation {
        name: name.into(),
        format: "mmdb",
        verified: warning.is_none(),
        warning,
        bytes: size,
        sha256: hash,
        ip_version: Some(metadata.ip_version),
        node_count: Some(metadata.node_count),
        build_epoch: Some(metadata.build_epoch),
        dat: None,
    })
}

#[cfg(not(unix))]
pub(crate) fn validate(_root: &Path, _name: &str) -> Result<Validation> {
    anyhow::bail!("Geo validation requires Unix")
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink, path::PathBuf};
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "ms-geo-validation-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn string(value: &str) -> Vec<u8> {
        let mut result = vec![0x40 | value.len() as u8];
        result.extend(value.as_bytes());
        result
    }
    pub(crate) fn fixture_with_description(description: bool) -> Vec<u8> {
        let mut bytes = vec![0, 0, 17, 0, 0, 1];
        bytes.extend([0; 16]);
        bytes.extend(string("CN"));
        bytes.extend(b"\xab\xcd\xefMaxMind.com");
        bytes.push(0xe9);
        for (key, value) in [
            ("binary_format_major_version", vec![0xa1, 2]),
            ("binary_format_minor_version", vec![0xa0]),
            ("build_epoch", vec![1, 2, 1]),
            ("database_type", string("fixture")),
            (
                "description",
                if description {
                    [vec![0xe1], string("en"), string("fixture")].concat()
                } else {
                    vec![0xe0]
                },
            ),
            ("ip_version", vec![0xa1, 4]),
            ("languages", vec![0, 4]),
            ("node_count", vec![0xc1, 1]),
            ("record_size", vec![0xa1, 24]),
        ] {
            bytes.extend(string(key));
            bytes.extend(value);
        }
        bytes
    }
    #[test]
    fn verifies_tree_records_and_hash_without_mutating_files() -> Result<()> {
        let directory = Directory::new()?;
        let bytes = fixture_with_description(true);
        maxminddb::Reader::from_source(bytes.clone())?;
        let path = directory.0.join("Country.mmdb");
        fs::write(&path, &bytes)?;
        let report = validate(&directory.0, "Country.mmdb")?;
        assert!(report.verified);
        assert!(report.warning.is_none());
        assert_eq!(report.node_count, Some(1));
        assert_eq!(report.ip_version, Some(4));
        assert_eq!(report.sha256.len(), 64);
        assert_eq!(fs::read(&path)?, bytes);
        let mut corrupt = bytes.clone();
        corrupt[2] = 255;
        fs::write(&path, corrupt)?;
        assert!(validate(&directory.0, "Country.mmdb").is_err());
        let mut corrupt = bytes.clone();
        corrupt[22] = 0x5d; // Truncated string record, valid metadata.
        fs::write(&path, corrupt)?;
        assert!(validate(&directory.0, "Country.mmdb").is_err());
        let mut corrupt = bytes;
        corrupt[6] = 1; // Invalid data separator.
        fs::write(&path, corrupt)?;
        assert!(validate(&directory.0, "Country.mmdb").is_err());
        Ok(())
    }
    #[test]
    fn empty_description_is_a_compatibility_warning_not_verified_structure() -> Result<()> {
        let directory = Directory::new()?;
        fs::write(directory.0.join("geoip.metadb"), fixture_with_description(false))?;
        let report = validate(&directory.0, "geoip.metadb")?;
        assert!(!report.verified);
        assert_eq!(report.warning, Some("empty_description_structure_unverified"));
        Ok(())
    }

    #[test]
    fn dat_snapshot_reports_hash_counts_and_limits_without_mmdb_metadata() -> Result<()> {
        let directory = Directory::new()?;
        for (name, fixture) in [
            ("geoip.dat", crate::dat_validation::fixtures::geoip()),
            ("geosite.dat", crate::dat_validation::fixtures::geosite()),
        ] {
            let path = directory.0.join(name);
            fs::write(&path, &fixture)?;
            let report = validate(&directory.0, name)?;
            assert!(report.verified && report.dat.is_some());
            assert_eq!(report.format, "dat");
            assert_eq!(report.warning, Some("dat_core_matching_unverified"));
            assert_eq!(report.sha256, sha256(&fixture));
            assert_eq!(report.bytes, fixture.len() as u64);
            let json = serde_json::to_value(&report)?;
            for absent in ["ip_version", "node_count", "build_epoch"] {
                assert!(json.get(absent).is_none());
            }
            assert!(!json.to_string().contains("exact.dat.test"));
            assert_eq!(fs::read(&path)?, fixture);
            let record = if name == "geoip.dat" {
                crate::dat_validation::fixtures::cidr(&[192, 0, 2, 0], 24)
            } else {
                crate::dat_validation::fixtures::domain(3, b"example.test")
            };
            let missing_cn = crate::dat_validation::fixtures::group(b"custom", &[record]);
            fs::write(&path, missing_cn)?;
            let missing = validate(&directory.0, name)?;
            assert!(missing.verified);
            assert_eq!(missing.warning, Some("dat_cn_group_missing"));
            assert!(!missing.dat.unwrap().has_cn_group);
            let mut future = fixture;
            future.extend([0x20, 1]);
            fs::write(&path, &future)?;
            let report = validate(&directory.0, name)?;
            assert!(!report.verified);
            assert_eq!(report.warning, Some("dat_unknown_fields_unverified"));
            assert_eq!(report.dat.unwrap().unknown_field_count, 1);
            fs::remove_file(&path)?;
            symlink("/etc/passwd", &path)?;
            assert!(validate(&directory.0, name).is_err());
            fs::remove_file(&path)?;
            let pipe = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
            assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
            assert!(validate(&directory.0, name).is_err());
            fs::remove_file(&path)?;
            fs::write(&path, [])?;
            assert!(validate(&directory.0, name).is_err());
            fs::OpenOptions::new().write(true).open(&path)?.set_len(MAX_BYTES + 1)?;
            assert!(validate(&directory.0, name).is_err());
        }
        Ok(())
    }

    #[test]
    fn rejects_links_special_files_limits_and_unrecognized_names() -> Result<()> {
        let directory = Directory::new()?;
        for name in [
            "../Country.mmdb",
            "/etc/passwd",
            "geosite.dat",
            "geoip.dat",
            "GeoSite.dat",
        ] {
            assert!(validate(&directory.0, name).is_err());
        }
        let path = directory.0.join("Country.mmdb");
        symlink("/etc/passwd", &path)?;
        assert!(validate(&directory.0, "Country.mmdb").is_err());
        fs::remove_file(&path)?;
        let pipe = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
        assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
        assert!(validate(&directory.0, "Country.mmdb").is_err());
        fs::remove_file(&path)?;
        fs::write(&path, [])?;
        assert!(validate(&directory.0, "Country.mmdb").is_err());
        fs::OpenOptions::new().write(true).open(&path)?.set_len(MAX_BYTES + 1)?;
        assert!(validate(&directory.0, "Country.mmdb").is_err());
        fs::write(&path, "not a database")?;
        assert!(validate(&directory.0, "Country.mmdb").is_err());
        Ok(())
    }
}
