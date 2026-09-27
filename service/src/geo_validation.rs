//! Explicit, read-only MMDB structural verification. No database records are returned.
use anyhow::{Result, ensure};
use serde::Serialize;
use std::path::Path;

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
    pub ip_version: u16,
    pub node_count: u32,
    pub build_epoch: u64,
}

/// Caller serializes against service resource/lifecycle mutations. The snapshot is
/// bounded and read through no-follow descriptors; validation does not modify it.
#[cfg(unix)]
pub(crate) fn validate(root: &Path, name: &str) -> Result<Validation> {
    use ring::digest::{SHA256, digest};
    use std::{
        ffi::CString,
        fs::{File, OpenOptions},
        io::Read as _,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::{MetadataExt as _, OpenOptionsExt as _},
        },
    };
    ensure!(
        MMDB_FILES.contains(&name),
        "Geo format validation supports only Country.mmdb, ASN.mmdb and geoip.metadb"
    );
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let leaf = CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            leaf.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    ensure!(fd >= 0, "Geo file missing, unreadable or linked");
    let mut file = unsafe { File::from_raw_fd(fd) };
    let before = file.metadata()?;
    ensure!(
        before.is_file() && before.len() > 0 && before.len() <= MAX_BYTES,
        "Geo validation requires a nonempty regular file up to 128 MiB"
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
    let hash = digest(&SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
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
        bytes: before.len(),
        sha256: hash,
        ip_version: metadata.ip_version,
        node_count: metadata.node_count,
        build_epoch: metadata.build_epoch,
    })
}

#[cfg(not(unix))]
pub(crate) fn validate(_root: &Path, _name: &str) -> Result<Validation> {
    anyhow::bail!("Geo validation requires Unix")
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
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
    fn fixture_with_description(description: bool) -> Vec<u8> {
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
        assert_eq!(report.node_count, 1);
        assert_eq!(report.ip_version, 4);
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
