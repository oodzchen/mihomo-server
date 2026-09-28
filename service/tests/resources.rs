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
    fn geo_manifest(&self, geo: serde_json::Value) -> Result<()> {
        fs::write(
            self.0.join("resources/manifest.json"),
            serde_json::to_vec(&json!({
                "schema_version":1,"target":TARGET,"core":{"version":"v1","sha256":hash(b"owned test core")},"geo":geo
            }))?,
        )?;
        Ok(())
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
fn geo_seed_publication_is_private_independent_and_never_overwrites_existing_data() -> Result<()> {
    let dir = Directory::new()?;
    fs::create_dir_all(dir.0.join("resources/geo"))?;
    fs::create_dir(dir.0.join("data"))?;
    fs::write(dir.0.join("resources/geo/geoip.metadb"), b"pinned Geo bytes")?;
    dir.geo_manifest(json!({"geoip.metadb":{"bytes":17,"sha256":hash(b"pinned Geo bytes")}}))?;
    // Pin must match exact size; an off-by-one is rejected before publication.
    assert!(dir.resources()?.initialize_geo(&dir.0.join("data")).is_err());
    assert!(!dir.0.join("data/geoip.metadb").exists());
    dir.geo_manifest(json!({"geoip.metadb":{"bytes":16,"sha256":hash(b"pinned Geo bytes")}}))?;
    assert_eq!(
        dir.resources()?.initialize_geo(&dir.0.join("data"))?,
        vec!["geoip.metadb"]
    );
    assert_eq!(fs::read(dir.0.join("data/geoip.metadb"))?, b"pinned Geo bytes");
    assert_eq!(
        fs::metadata(dir.0.join("data/geoip.metadb"))?.permissions().mode() & 0o777,
        0o600
    );
    assert!(!dir.0.join("data/.geo-seed").exists());
    fs::write(dir.0.join("data/geoip.metadb"), b"newer user Geo data")?;
    fs::remove_dir_all(dir.0.join("resources/geo"))?;
    assert!(dir.resources()?.initialize_geo(&dir.0.join("data"))?.is_empty());
    assert_eq!(fs::read(dir.0.join("data/geoip.metadb"))?, b"newer user Geo data");
    Ok(())
}

#[test]
fn bad_second_geo_digest_leaves_all_live_destinations_unchanged() -> Result<()> {
    let dir = Directory::new()?;
    fs::create_dir_all(dir.0.join("resources/geo"))?;
    fs::create_dir(dir.0.join("data"))?;
    for name in ["Country.mmdb", "geosite.dat"] {
        fs::write(dir.0.join("resources/geo").join(name), b"Geo fixture")?;
    }
    dir.geo_manifest(json!({"Country.mmdb":{"bytes":11,"sha256":hash(b"Geo fixture")},"geosite.dat":{"bytes":11,"sha256":"0".repeat(64)}}))?;
    assert!(dir.resources()?.initialize_geo(&dir.0.join("data")).is_err());
    assert_eq!(fs::read_dir(dir.0.join("data"))?.count(), 0);
    Ok(())
}

#[test]
fn geo_manifest_rejects_unsafe_names_invalid_pins_and_oversized_sets() -> Result<()> {
    let dir = Directory::new()?;
    for geo in [
        json!({"../escape":{"bytes":1,"sha256":hash(b"x")}}),
        json!({"geoip.metadb":{"bytes":0,"sha256":hash(b"")}}),
        json!({"geoip.metadb":{"bytes":128*1024*1024+1,"sha256":hash(b"x")}}),
        json!({"geoip.metadb":{"bytes":1,"sha256":"invalid"}}),
        json!({"geoip.metadb":{"bytes":1,"sha256":hash(b"x"),"url":"https://override.invalid"}}),
        json!({"Country.mmdb":{"bytes":128*1024*1024,"sha256":hash(b"x")},"ASN.mmdb":{"bytes":128*1024*1024,"sha256":hash(b"x")},"geoip.metadb":{"bytes":1,"sha256":hash(b"x")}}),
    ] {
        dir.geo_manifest(geo)?;
        assert!(dir.resources().is_err());
    }
    fs::write(dir.0.join("resources/manifest.json"), vec![b' '; 65537])?;
    assert!(dir.resources().is_err());
    Ok(())
}

#[test]
fn geo_links_special_files_and_unsafe_staging_do_not_publish_or_change_external_files() -> Result<()> {
    let dir = Directory::new()?;
    fs::create_dir_all(dir.0.join("resources/geo"))?;
    fs::create_dir(dir.0.join("data"))?;
    fs::write(dir.0.join("external"), "kept")?;
    dir.geo_manifest(json!({"geoip.metadb":{"bytes":4,"sha256":hash(b"kept")}}))?;
    symlink(dir.0.join("external"), dir.0.join("resources/geo/geoip.metadb"))?;
    assert!(dir.resources()?.initialize_geo(&dir.0.join("data")).is_err());
    fs::remove_file(dir.0.join("resources/geo/geoip.metadb"))?;
    fs::write(dir.0.join("resources/geo/geoip.metadb"), "kept")?;
    symlink(dir.0.join("external"), dir.0.join("data/geoip.metadb"))?;
    assert!(dir.resources()?.initialize_geo(&dir.0.join("data")).is_err());
    fs::remove_file(dir.0.join("data/geoip.metadb"))?;
    fs::remove_file(dir.0.join("resources/geo/geoip.metadb"))?;
    let fifo = std::ffi::CString::new(dir.0.join("resources/geo/geoip.metadb").as_os_str().as_encoded_bytes())?;
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(dir.resources()?.initialize_geo(&dir.0.join("data")).is_err());
    fs::remove_file(dir.0.join("resources/geo/geoip.metadb"))?;
    fs::write(dir.0.join("resources/geo/geoip.metadb"), "kept")?;
    fs::write(dir.0.join("data/geoip.metadb"), [])?;
    assert!(dir.resources()?.initialize_geo(&dir.0.join("data")).is_err());
    fs::remove_file(dir.0.join("data/geoip.metadb"))?;
    fs::create_dir(dir.0.join("data/.geo-seed"))?;
    fs::set_permissions(dir.0.join("data/.geo-seed"), fs::Permissions::from_mode(0o700))?;
    symlink(dir.0.join("external"), dir.0.join("data/.geo-seed/geoip.metadb"))?;
    assert!(dir.resources()?.initialize_geo(&dir.0.join("data")).is_err());
    assert_eq!(fs::read_to_string(dir.0.join("external"))?, "kept");
    assert!(!dir.0.join("data/geoip.metadb").exists());
    Ok(())
}

#[test]
fn interrupted_geo_publication_recovers_known_orphans_and_preserves_linked_live_file() -> Result<()> {
    let dir = Directory::new()?;
    fs::create_dir_all(dir.0.join("resources/geo"))?;
    fs::create_dir(dir.0.join("data"))?;
    fs::create_dir(dir.0.join("data/.geo-seed"))?;
    fs::set_permissions(dir.0.join("data/.geo-seed"), fs::Permissions::from_mode(0o700))?;
    fs::write(dir.0.join("data/.geo-seed/Country.mmdb"), "already published")?;
    fs::hard_link(
        dir.0.join("data/.geo-seed/Country.mmdb"),
        dir.0.join("data/Country.mmdb"),
    )?;
    fs::write(dir.0.join("data/.geo-seed/geosite.dat"), "partial")?;
    fs::write(dir.0.join("resources/geo/geosite.dat"), "verified")?;
    dir.geo_manifest(
        json!({"Country.mmdb":{"bytes":1,"sha256":hash(b"x")},"geosite.dat":{"bytes":8,"sha256":hash(b"verified")}}),
    )?;
    assert_eq!(
        dir.resources()?.initialize_geo(&dir.0.join("data"))?,
        vec!["geosite.dat"]
    );
    assert_eq!(
        fs::read_to_string(dir.0.join("data/Country.mmdb"))?,
        "already published"
    );
    assert_eq!(fs::read_to_string(dir.0.join("data/geosite.dat"))?, "verified");
    assert!(!dir.0.join("data/.geo-seed").exists());
    Ok(())
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

#[test]
fn manifest_with_license_inventory_is_exposed() -> Result<()> {
    let dir = Directory::new()?;
    fs::write(
        dir.0.join("resources/manifest.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "target": TARGET,
            "core": {"version": "v1.19.31", "sha256": hash(b"owned test core")},
            "licenses": {"primary": "LICENSE", "inventory": "LICENSES.txt"}
        }))?,
    )?;
    let resources = dir.resources()?;
    let licenses = resources.licenses().expect("license info present");
    assert_eq!(licenses.primary.as_deref(), Some("LICENSE"));
    assert_eq!(licenses.inventory.as_deref(), Some("LICENSES.txt"));
    Ok(())
}
