#![cfg(unix)]
use anyhow::Result;
use mihomo_server::resources::{Resources, TARGET};
use ring::digest::{SHA256, digest};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::{PermissionsExt as _, symlink},
    path::PathBuf,
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = Self(std::env::temp_dir().join(format!("ms-resources-{}-{stamp:x}", std::process::id())));
        fs::create_dir_all(directory.0.join("resources/core"))?;
        fs::write(directory.0.join("resources/core/verge-mihomo"), b"owned test core")?;
        fs::write(directory.0.join("resources/minimal.yaml"), "mode: rule\n")?;
        directory.manifest(TARGET, &hash(b"owned test core"))?;
        Ok(directory)
    }
    fn manifest(&self, target: &str, hash: &str) -> Result<()> {
        fs::write(
            self.0.join("resources/manifest.json"),
            serde_json::to_vec(&json!({
                "schema_version":1, "target":target, "core":{"version":"v1.19.31","sha256":hash}
            }))?,
        )?;
        Ok(())
    }
    fn resources(&self) -> Result<Resources> {
        Resources::open(&self.0.join("resources"))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn hash(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn initialization_verifies_and_publishes_a_private_independent_executable() -> Result<()> {
    let directory = Directory::new()?;
    let resources = directory.resources()?;
    let core = resources.initialize_core(&directory.0.join("data/core"))?;
    assert_eq!(fs::read(&core)?, b"owned test core");
    assert_eq!(fs::metadata(&core)?.permissions().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(core.parent().unwrap())?.permissions().mode() & 0o777,
        0o700
    );
    fs::write(directory.0.join("resources/core/verge-mihomo"), "changed source")?;
    assert_eq!(fs::read(&core)?, b"owned test core");
    assert!(resources.bootstrap()?.is_absolute());
    Ok(())
}
#[test]
fn checksum_failure_never_publishes_a_partial_managed_core() -> Result<()> {
    let directory = Directory::new()?;
    directory.manifest(TARGET, &"0".repeat(64))?;
    let core_dir = directory.0.join("core");
    assert!(directory.resources()?.initialize_core(&core_dir).is_err());
    assert_eq!(fs::read_dir(&core_dir)?.count(), 0);
    fs::write(directory.0.join("resources/core/verge-mihomo"), [])?;
    directory.manifest(TARGET, &hash(&[]))?;
    assert!(directory.resources()?.initialize_core(&core_dir).is_err());
    assert_eq!(fs::read_dir(core_dir)?.count(), 0);
    Ok(())
}
#[test]
fn existing_upgraded_core_survives_changed_or_missing_bundle_seed() -> Result<()> {
    let directory = Directory::new()?;
    let core_dir = directory.0.join("core");
    let core = directory.resources()?.initialize_core(&core_dir)?;
    fs::write(&core, "upgraded independent core")?;
    fs::remove_file(directory.0.join("resources/core/verge-mihomo"))?;
    directory.manifest(TARGET, &"0".repeat(64))?;
    assert_eq!(directory.resources()?.initialize_core(&core_dir)?, core);
    assert_eq!(fs::read_to_string(&core)?, "upgraded independent core");
    Ok(())
}
#[test]
fn invalid_manifest_and_external_resource_links_are_rejected() -> Result<()> {
    let directory = Directory::new()?;
    directory.manifest("wrong-platform", &hash(b"owned test core"))?;
    assert!(directory.resources().is_err());
    directory.manifest(TARGET, "malformed")?;
    assert!(directory.resources().is_err());
    for manifest in [
        json!({"schema_version":2, "target":TARGET, "core":{"version":"v1", "sha256":hash(b"owned test core")}}),
        json!({"schema_version":1, "target":TARGET, "core":{"version":"", "sha256":hash(b"owned test core")}}),
        json!({"schema_version":1, "target":TARGET, "core":{"version":"v1", "sha256":hash(b"owned test core")}, "unknown":true}),
    ] {
        fs::write(
            directory.0.join("resources/manifest.json"),
            serde_json::to_vec(&manifest)?,
        )?;
        assert!(directory.resources().is_err());
    }
    directory.manifest(TARGET, &hash(b"owned test core"))?;
    let resources = directory.resources()?;
    fs::write(directory.0.join("external"), "owned test core")?;
    fs::remove_file(directory.0.join("resources/core/verge-mihomo"))?;
    symlink(
        directory.0.join("external"),
        directory.0.join("resources/core/verge-mihomo"),
    )?;
    assert!(resources.initialize_core(&directory.0.join("core")).is_err());
    fs::remove_file(directory.0.join("resources/minimal.yaml"))?;
    symlink(directory.0.join("external"), directory.0.join("resources/minimal.yaml"))?;
    assert!(resources.bootstrap().is_err());
    fs::remove_file(directory.0.join("resources/manifest.json"))?;
    symlink(
        directory.0.join("external"),
        directory.0.join("resources/manifest.json"),
    )?;
    assert!(directory.resources().is_err());
    Ok(())
}
#[test]
fn unsafe_managed_directories_and_links_are_rejected_but_broken_regular_cores_remain_repairable() -> Result<()> {
    let directory = Directory::new()?;
    let resources = directory.resources()?;
    let core_dir = directory.0.join("core");
    fs::create_dir(&core_dir)?;
    fs::set_permissions(&core_dir, fs::Permissions::from_mode(0o755))?;
    assert!(resources.initialize_core(&core_dir).is_err());
    fs::set_permissions(&core_dir, fs::Permissions::from_mode(0o700))?;
    symlink(
        directory.0.join("resources/core/verge-mihomo"),
        core_dir.join("verge-mihomo"),
    )?;
    assert!(resources.initialize_core(&core_dir).is_err());
    fs::remove_file(core_dir.join("verge-mihomo"))?;
    fs::write(core_dir.join("verge-mihomo"), "not executable")?;
    fs::set_permissions(core_dir.join("verge-mihomo"), fs::Permissions::from_mode(0o600))?;
    assert_eq!(resources.initialize_core(&core_dir)?, core_dir.join("verge-mihomo"));
    assert_eq!(fs::read(core_dir.join("verge-mihomo"))?, b"not executable");
    fs::write(core_dir.join("verge-mihomo"), [])?;
    fs::set_permissions(core_dir.join("verge-mihomo"), fs::Permissions::from_mode(0o0))?;
    assert_eq!(resources.initialize_core(&core_dir)?, core_dir.join("verge-mihomo"));
    assert_eq!(fs::metadata(core_dir.join("verge-mihomo"))?.len(), 0);
    fs::remove_file(core_dir.join("verge-mihomo"))?;
    symlink(&core_dir, directory.0.join("core-link"))?;
    assert!(resources.initialize_core(&directory.0.join("core-link")).is_err());
    Ok(())
}
