//! Portable service backup metadata; adapted from upstream's configuration ZIP export.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_ENTRIES: usize = 1024;
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_CONTENT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_ARCHIVE_BYTES: usize = 65 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// Read-only validation results. No source paths, profile names or credentials.
/// This establishes archive/configuration coherence, not executable restore readiness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupInspection {
    pub schema_version: u32,
    pub source: String,
    pub service_version: String,
    pub created_at: u64,
    pub archive_bytes: u64,
    pub archive_sha256: String,
    pub entry_count: usize,
    pub content_bytes: u64,
    pub profile_count: usize,
    pub active_profile_present: bool,
    pub runtime_revision_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupEntry {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    pub schema_version: u32,
    pub source: String,
    pub service_version: String,
    pub created_at: u64,
    pub active_profile: Option<String>,
    pub runtime_revision: Option<String>,
    /// The manifest itself is not listed recursively.
    pub entries: Vec<BackupEntry>,
}

/// Retains upstream filename/length metadata without exposing host paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupMetadata {
    pub filename: String,
    pub created_at: u64,
    pub content_length: u64,
    pub sha256: String,
}

pub fn validate_filename(name: &str) -> Result<()> {
    crate::config::profile_store::validate_profile_file(name)?;
    ensure!(
        name.len() <= 255 && !name.contains(':') && !name.chars().any(char::is_control),
        "invalid backup filename"
    );
    Ok(())
}
impl BackupManifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1 && self.source == "mihomo-server",
            "unsupported backup format"
        );
        ensure!(
            !self.service_version.trim().is_empty()
                && self.service_version.len() <= 64
                && !self.service_version.chars().any(char::is_control),
            "invalid backup service version"
        );
        if let Some(uid) = &self.active_profile {
            ensure!(
                !uid.is_empty() && uid.len() <= 256 && !uid.chars().any(char::is_control),
                "invalid backup active profile"
            );
        }
        if let Some(revision) = &self.runtime_revision {
            validate_filename(revision)?;
        }
        ensure!(self.entries.len() < MAX_ENTRIES, "backup has too many entries");
        let mut paths = HashSet::new();
        let mut total = 0u64;
        for entry in &self.entries {
            match entry.path.as_str() {
                "profiles.yaml" | "settings.yaml" | "runtime.yaml" => {}
                path => validate_filename(
                    path.strip_prefix("profiles/")
                        .ok_or_else(|| anyhow::anyhow!("invalid backup entry path"))?,
                )?,
            }
            ensure!(paths.insert(entry.path.as_str()), "duplicate backup entry");
            ensure!(entry.bytes <= MAX_FILE_BYTES, "backup entry exceeds 8 MiB");
            ensure!(
                entry.sha256.len() == 64
                    && entry
                        .sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid backup entry digest"
            );
            total = total
                .checked_add(entry.bytes)
                .ok_or_else(|| anyhow::anyhow!("backup size overflow"))?;
        }
        ensure!(total <= MAX_CONTENT_BYTES, "backup content exceeds 64 MiB");
        ensure!(
            ["profiles.yaml", "settings.yaml", "runtime.yaml"]
                .iter()
                .all(|name| paths.contains(name)),
            "backup is missing required configuration"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest() -> BackupManifest {
        BackupManifest {
            schema_version: 1,
            source: "mihomo-server".into(),
            service_version: "0.1.0".into(),
            created_at: 1,
            active_profile: None,
            runtime_revision: None,
            entries: ["profiles.yaml", "settings.yaml", "runtime.yaml"]
                .iter()
                .map(|path| BackupEntry {
                    path: (*path).into(),
                    bytes: 1,
                    sha256: "a".repeat(64),
                })
                .collect(),
        }
    }
    #[test]
    fn portable_manifest_roundtrip_and_invalid_paths_hashes_bounds_fail_closed() -> Result<()> {
        let good = manifest();
        good.validate()?;
        assert_eq!(
            serde_json::from_slice::<BackupManifest>(&serde_json::to_vec(&good)?)?,
            good
        );
        for path in [
            "profiles/../token",
            "profiles/a/b",
            "profiles/a\\b",
            "profiles/C:token",
            "profiles/a\n",
            "management-token",
            "manifest.json",
        ] {
            let mut bad = good.clone();
            bad.entries.push(BackupEntry {
                path: path.into(),
                bytes: 0,
                sha256: "b".repeat(64),
            });
            assert!(bad.validate().is_err());
        }
        for case in [
            "schema",
            "missing",
            "duplicate",
            "hash",
            "file-size",
            "total-size",
            "count",
        ] {
            let mut bad = good.clone();
            match case {
                "schema" => bad.schema_version = 2,
                "missing" => {
                    bad.entries.pop();
                }
                "duplicate" => bad.entries.push(bad.entries[0].clone()),
                "hash" => bad.entries[0].sha256 = "A".repeat(64),
                "file-size" => bad.entries[0].bytes = MAX_FILE_BYTES + 1,
                "total-size" => {
                    for n in 0..9 {
                        bad.entries.push(BackupEntry {
                            path: format!("profiles/{n}.yaml"),
                            bytes: MAX_FILE_BYTES,
                            sha256: "a".repeat(64),
                        });
                    }
                }
                "count" => {
                    for n in 0..MAX_ENTRIES {
                        bad.entries.push(BackupEntry {
                            path: format!("profiles/{n}.yaml"),
                            bytes: 0,
                            sha256: "a".repeat(64),
                        });
                    }
                }
                _ => unreachable!(),
            }
            assert!(bad.validate().is_err(), "{case}");
        }
        assert!(
            serde_json::from_str::<BackupEntry>(
                r#"{"path":"runtime.yaml","bytes":1,"sha256":"x","destination":"/tmp"}"#
            )
            .is_err()
        );
        Ok(())
    }
}
