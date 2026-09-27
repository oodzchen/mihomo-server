#![cfg(target_os = "linux")]
use anyhow::Result;
use headless_core::config::resource_paths::{metadata_below, prepare, prepare_owned, validate, validate_owned};
use serde_yaml_ng::{Mapping, Value};
use std::{fs, os::unix::fs::symlink, path::PathBuf};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ms-provider-paths-{}-{stamp}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn prepare(&self, yaml: &str) -> Result<Mapping> {
        prepare(serde_yaml_ng::from_str(yaml)?, &self.0, &[])
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn owned_caches_isolate_revisions_and_reuse_unchanged_sources_without_claiming_old_files() -> Result<()> {
    let dir = Directory::new()?;
    fs::write(dir.0.join("old.yaml"), "unclaimed old source")?;
    let raw: Mapping =
        serde_yaml_ng::from_str("proxy-providers: {a: {type: http, path: old.yaml, url: 'https://one.invalid'}}")?;
    let first = prepare_owned(raw.clone(), &dir.0, &[])?;
    let path = first["proxy-providers"]["a"]["path"].as_str().unwrap();
    assert!(path.starts_with("provider-cache/v1/"));
    assert!(!dir.0.join(path).exists());
    assert_eq!(fs::read_to_string(dir.0.join("old.yaml"))?, "unclaimed old source");
    assert_eq!(raw["proxy-providers"]["a"]["path"], "old.yaml");
    assert_eq!(prepare_owned(first.clone(), &dir.0, &[])?, first);
    validate_owned(&first, &dir.0, &[])?;
    let mut changed = first.clone();
    changed["proxy-providers"]["a"]["url"] = "https://two.invalid".into();
    assert!(validate_owned(&changed, &dir.0, &[]).is_err());
    let second = prepare_owned(changed, &dir.0, &[])?;
    assert_ne!(second["proxy-providers"]["a"]["path"], path);
    let renamed: Mapping = serde_yaml_ng::from_str(
        "proxy-providers: {renamed: {type: http, path: different.yaml, url: 'https://one.invalid', interval: 120, filter: example}}",
    )?;
    let renamed = prepare_owned(renamed, &dir.0, &[])?;
    assert_eq!(renamed["proxy-providers"]["renamed"]["path"], path);
    let remaining = prepare_owned(raw, &dir.0, &[])?;
    assert_eq!(remaining["proxy-providers"]["a"]["path"], path);
    Ok(())
}

#[test]
fn cache_identity_tracks_headers_transport_and_parser_but_not_mapping_order() -> Result<()> {
    let dir = Directory::new()?;
    let raw: Mapping = serde_yaml_ng::from_str(
        "proxy-providers: {a: {type: http, url: 'https://one.invalid/private-token', header: {Authorization: [secret], Accept: [yaml]}}}",
    )?;
    let first = prepare_owned(raw.clone(), &dir.0, &[])?;
    let path = &first["proxy-providers"]["a"]["path"];
    assert!(!path.as_str().unwrap().contains("secret"));
    let reordered: Mapping = serde_yaml_ng::from_str(
        "proxy-providers: {a: {header: {Accept: [yaml], Authorization: [secret]}, url: 'https://one.invalid/private-token', type: http}}",
    )?;
    assert_eq!(
        prepare_owned(reordered, &dir.0, &[])?["proxy-providers"]["a"]["path"],
        *path
    );
    for (key, value) in [
        ("header", "{Authorization: [other]}"),
        ("proxy", "DIRECT"),
        ("format", "json"),
        ("behavior", "classical"),
    ] {
        let mut changed = raw.clone();
        changed["proxy-providers"]["a"][key] = serde_yaml_ng::from_str(value)?;
        assert_ne!(
            prepare_owned(changed, &dir.0, &[])?["proxy-providers"]["a"]["path"],
            *path
        );
    }
    let mut rule = Mapping::new();
    rule.insert("rule-providers".into(), raw["proxy-providers"].clone());
    assert_ne!(prepare_owned(rule, &dir.0, &[])?["rule-providers"]["a"]["path"], *path);
    Ok(())
}

#[test]
fn owned_namespace_rejects_local_claims_legacy_start_and_later_filesystem_changes() -> Result<()> {
    let dir = Directory::new()?;
    let raw: Mapping = serde_yaml_ng::from_str("proxy-providers: {a: {type: http, url: 'https://one.invalid'}}")?;
    assert!(validate_owned(&raw, &dir.0, &[]).is_err());
    let prepared = prepare_owned(raw, &dir.0, &[])?;
    let path = prepared["proxy-providers"]["a"]["path"].as_str().unwrap();
    let local: Mapping = serde_yaml_ng::from_str(&format!("proxy-providers: {{a: {{type: file, path: {path}}}}}"))?;
    assert!(prepare_owned(local, &dir.0, &[]).is_err());
    assert!(prepare_owned(prepared.clone(), &dir.0, &[dir.0.join("provider-cache")]).is_err());
    fs::create_dir_all(dir.0.join("provider-cache/v1"))?;
    fs::write(dir.0.join("original"), "cache")?;
    fs::hard_link(dir.0.join("original"), dir.0.join(path))?;
    assert!(validate_owned(&prepared, &dir.0, &[]).is_err());
    fs::remove_file(dir.0.join(path))?;
    symlink("/etc/passwd", dir.0.join(path))?;
    assert!(validate_owned(&prepared, &dir.0, &[]).is_err());
    fs::remove_file(dir.0.join(path))?;
    fs::remove_dir(dir.0.join("provider-cache/v1"))?;
    symlink("/etc", dir.0.join("provider-cache/v1"))?;
    assert!(validate_owned(&prepared, &dir.0, &[]).is_err());
    Ok(())
}

#[test]
fn remote_cache_allocation_is_stable_order_independent_and_leaves_source_unchanged() -> Result<()> {
    let dir = Directory::new()?;
    let yaml = "proxy-providers:\n  a: {type: http, path: ./providers/shared.yaml, url: 'https://one.invalid/private-a'}\n  b: {type: http, path: providers/shared.yaml, url: 'https://two.invalid/private-b'}\nrule-providers:\n  c: {type: http, path: providers/shared.yaml, url: 'https://one.invalid/private-a'}\n";
    let original: Mapping = serde_yaml_ng::from_str(yaml)?;
    assert!(validate(&original, &dir.0, &[]).is_err());
    let candidate = prepare(original.clone(), &dir.0, &[])?;
    let a = candidate["proxy-providers"]["a"]["path"].as_str().unwrap();
    let b = candidate["proxy-providers"]["b"]["path"].as_str().unwrap();
    assert_ne!(a, b);
    assert_eq!(candidate["rule-providers"]["c"]["path"], a);
    assert!(a.starts_with("providers/cvr-") && a.ends_with(".yaml"));
    assert!(!a.contains("private-a"));
    assert_eq!(
        candidate["proxy-providers"]["a"]["url"],
        original["proxy-providers"]["a"]["url"]
    );
    assert_eq!(original["proxy-providers"]["a"]["path"], "./providers/shared.yaml");
    validate(&candidate, &dir.0, &[])?;
    assert_eq!(prepare(candidate.clone(), &dir.0, &[])?, candidate);
    let mut reversed = original;
    let providers = reversed.get_mut("proxy-providers").unwrap().as_mapping_mut().unwrap();
    let a_entry = providers.remove("a").unwrap();
    providers.insert(Value::String("a".into()), a_entry);
    let reordered = prepare(reversed, &dir.0, &[])?;
    assert_eq!(reordered["proxy-providers"]["a"]["path"], a);
    assert_eq!(reordered["proxy-providers"]["b"]["path"], b);
    Ok(())
}

#[test]
fn allocator_does_not_claim_another_declared_resource_destination() -> Result<()> {
    let dir = Directory::new()?;
    let original: Mapping = serde_yaml_ng::from_str(
        "proxy-providers:\n  a: {type: http, path: cache/shared.yaml, url: 'https://one.invalid'}\n  b: {type: http, path: cache/shared.yaml, url: 'https://two.invalid'}",
    )?;
    let expected = prepare(original.clone(), &dir.0, &[])?;
    let reserved = expected["proxy-providers"]["a"]["path"].as_str().unwrap();
    let mut config = original;
    config["proxy-providers"].as_mapping_mut().unwrap().insert(
        Value::String("local".into()),
        serde_yaml_ng::from_str(&format!("{{type: file, path: {reserved}}}"))?,
    );
    let candidate = prepare(config, &dir.0, &[])?;
    assert_ne!(candidate["proxy-providers"]["a"]["path"], reserved);
    assert!(
        candidate["proxy-providers"]["a"]["path"]
            .as_str()
            .unwrap()
            .ends_with("-1.yaml")
    );
    assert_eq!(candidate["proxy-providers"]["local"]["path"], reserved);
    Ok(())
}

#[test]
fn shared_local_files_and_identical_remote_urls_remain_compatible() -> Result<()> {
    let dir = Directory::new()?;
    let yaml = format!(
        "proxy-providers:\n  a: {{type: file, path: {}/providers/local.yaml}}\n  b: {{type: file, path: ./providers/local.yaml}}\n  remote: {{type: http, path: ./cache/http.yaml, url: 'https://same.invalid'}}\n  implicit: {{type: http, url: 'https://implicit.invalid'}}\n  inline: {{type: inline, path: ../ignored, payload: []}}\nrule-providers:\n  remote: {{type: http, path: cache/http.yaml, url: 'https://same.invalid'}}",
        dir.0.display()
    );
    let candidate = dir.prepare(&yaml)?;
    assert_eq!(candidate["proxy-providers"]["a"]["path"], "providers/local.yaml");
    assert_eq!(candidate["proxy-providers"]["b"]["path"], "providers/local.yaml");
    assert_eq!(candidate["proxy-providers"]["remote"]["path"], "cache/http.yaml");
    assert!(candidate["proxy-providers"]["implicit"].get("path").is_none());
    assert_eq!(candidate["proxy-providers"]["inline"]["path"], "../ignored");
    assert!(dir.prepare("proxy-providers: {a: {type: file, path: cache/shared.yaml}, b: {type: http, path: cache/shared.yaml, url: 'https://remote.invalid'}}").is_err());
    Ok(())
}

#[test]
fn traversal_links_special_files_and_service_owned_destinations_are_rejected_before_io() -> Result<()> {
    let dir = Directory::new()?;
    fs::write(dir.0.join("regular"), "source")?;
    fs::hard_link(dir.0.join("regular"), dir.0.join("hardlink"))?;
    symlink("/etc/passwd", dir.0.join("link"))?;
    symlink("/etc", dir.0.join("linked-parent"))?;
    symlink("/missing", dir.0.join("dangling"))?;
    fs::create_dir(dir.0.join("directory"))?;
    let pipe = std::ffi::CString::new(dir.0.join("fifo").as_os_str().as_encoded_bytes())?;
    assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
    for path in [
        "../escape",
        "/etc/passwd",
        "linked-parent/passwd",
        "linked-parent/missing",
        "link",
        "dangling",
        "directory",
        "fifo",
        "config/revisions/overwrite.yaml",
        "profiles/source.yaml",
        "core/verge-mihomo",
        "run/controller.sock",
        "backups/overwrite.zip",
        "restore-candidates/runtime.yaml",
        "management-token",
        "settings.yaml",
        "profiles.yaml",
        "profile-refresh.yaml",
        "settings-transaction.yaml",
        "cache.db",
        "Country.mmdb",
        "geoip.metadb",
        ".mihomo-server.lock",
        "cache/.hidden",
        "cache/../escape",
        "",
        "C:drive",
        "a\\b",
    ] {
        let yaml = format!(
            "proxy-providers: {{a: {{type: http, path: {}, url: 'https://remote.invalid'}}}}",
            serde_json::to_string(path)?
        );
        assert!(dir.prepare(&yaml).is_err(), "accepted unsafe path {path}");
    }
    assert!(
        dir.prepare("proxy-providers: {a: {type: http, path: hardlink, url: 'https://remote.invalid'}}")
            .is_err()
    );
    dir.prepare("proxy-providers: {a: {type: file, path: hardlink}}")?;
    assert!(metadata_below(&dir.0, std::path::Path::new("../outside")).is_err());
    assert_eq!(fs::read_to_string(dir.0.join("regular"))?, "source");
    let config: Mapping = serde_yaml_ng::from_str(
        "proxy-providers: {a: {type: http, path: bootstrap.yaml, url: 'https://remote.invalid'}}",
    )?;
    assert!(prepare(config, &dir.0, &[dir.0.join("bootstrap.yaml")]).is_err());
    let config: Mapping = serde_yaml_ng::from_str(
        "proxy-providers: {a: {type: http, path: custom-core/core-cache.yaml, url: 'https://remote.invalid'}}",
    )?;
    assert!(prepare(config, &dir.0, &[dir.0.join("custom-core")]).is_err());
    Ok(())
}

#[test]
fn declarations_are_bounded_and_filesystem_changes_are_rechecked_before_start() -> Result<()> {
    let dir = Directory::new()?;
    let candidate =
        dir.prepare("proxy-providers: {a: {type: http, path: providers/cache.yaml, url: 'https://remote.invalid'}}")?;
    fs::create_dir(dir.0.join("providers"))?;
    symlink("/etc/passwd", dir.0.join("providers/cache.yaml"))?;
    assert!(validate(&candidate, &dir.0, &[]).is_err());
    for yaml in [
        "proxy-providers: []",
        "rule-providers: {1: {path: x}}",
        "proxy-providers: {a: {path: []}}",
    ] {
        assert!(dir.prepare(yaml).is_err());
    }
    let yaml = format!(
        "proxy-providers:\n{}",
        (0..513)
            .map(|n| format!("  p{n}: {{type: inline}}\n"))
            .collect::<String>()
    );
    assert!(dir.prepare(&yaml).is_err());
    Ok(())
}
