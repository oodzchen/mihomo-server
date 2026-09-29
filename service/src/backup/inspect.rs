//! Validate our narrow Stored ZIP format before any future restore transaction.
//! Pinned ranges are borrowed from the bounded upload; nothing is extracted or executed.
use anyhow::{Context as _, Result, ensure};
use headless_core::{
    backup::{
        BackupInspection, BackupManifest, MAX_ARCHIVE_BYTES, MAX_CONTENT_BYTES, MAX_ENTRIES, MAX_FILE_BYTES,
        MAX_MANIFEST_BYTES, validate_filename,
    },
    config::{
        IProfiles,
        profile_store::SequenceKind,
        runtime,
        settings::{MAX_SETTINGS_BYTES, ServiceSettings},
    },
};
use serde_yaml_ng::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read as _},
    time::{Duration, Instant},
};
use tokio::sync::watch;

struct Budget {
    deadline: Instant,
    shutdown: watch::Receiver<bool>,
    closing: watch::Receiver<bool>,
}
impl Budget {
    fn check(&self) -> Result<()> {
        ensure!(
            !*self.shutdown.borrow() && !*self.closing.borrow(),
            "backup inspection cancelled"
        );
        ensure!(Instant::now() < self.deadline, "backup inspection deadline exceeded");
        Ok(())
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Result<usize> {
    Ok(u16::from_le_bytes(bytes.get(at..at + 2).context("truncated ZIP")?.try_into()?) as usize)
}
fn u32_at(bytes: &[u8], at: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(bytes.get(at..at + 4).context("truncated ZIP")?.try_into()?) as usize)
}
fn signature(bytes: &[u8], at: usize, expected: usize) -> Result<()> {
    ensure!(u32_at(bytes, at)? == expected, "invalid ZIP structure");
    Ok(())
}
struct Entry<'a> {
    name: &'a str,
    data: &'a [u8],
}

/// Check the central directory *before* ZipArchive indexes names (and hides duplicates).
/// Contiguous matching local records exclude prefix/trailer data, overlaps, aliases,
/// data descriptors, comments, extras, ZIP64, encryption and multi-disk variants.
fn structure<'a>(bytes: &'a [u8], budget: &Budget) -> Result<Vec<Entry<'a>>> {
    ensure!(bytes.len() <= MAX_ARCHIVE_BYTES, "backup archive exceeds 65 MiB");
    let end = bytes.len().checked_sub(22).context("truncated ZIP footer")?;
    signature(bytes, end, 0x06054b50)?;
    ensure!(
        u16_at(bytes, end + 4)? == 0 && u16_at(bytes, end + 6)? == 0 && u16_at(bytes, end + 20)? == 0,
        "unsupported ZIP footer"
    );
    let count = u16_at(bytes, end + 10)?;
    ensure!(
        (4..=MAX_ENTRIES).contains(&count) && u16_at(bytes, end + 8)? == count,
        "invalid ZIP entry count"
    );
    let central = u32_at(bytes, end + 16)?;
    ensure!(
        central.checked_add(u32_at(bytes, end + 12)?) == Some(end),
        "invalid ZIP directory range"
    );
    let mut cursor = central;
    let mut local = 0usize;
    let mut names = BTreeSet::new();
    let mut total = 0u64;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        budget.check()?;
        signature(bytes, cursor, 0x02014b50)?;
        ensure!(cursor + 46 <= end, "truncated ZIP directory");
        let name_len = u16_at(bytes, cursor + 28)?;
        let next = cursor + 46 + name_len;
        ensure!(
            next <= end && (1..=264).contains(&name_len),
            "invalid ZIP filename length"
        );
        let name_bytes = &bytes[cursor + 46..next];
        let name = std::str::from_utf8(name_bytes).context("invalid ZIP filename encoding")?;
        if !matches!(
            name,
            "manifest.json" | "profiles.yaml" | "settings.yaml" | "runtime.yaml"
        ) {
            validate_filename(name.strip_prefix("profiles/").context("unexpected backup entry")?)?;
        }
        ensure!(names.insert(name), "duplicate ZIP entry");
        let needed = u16_at(bytes, cursor + 6)?;
        let flags = u16_at(bytes, cursor + 8)?;
        ensure!(
            matches!(needed, 10 | 20)
                && matches!(flags, 0 | 2048)
                && (name.is_ascii() || flags == 2048)
                && u16_at(bytes, cursor + 10)? == 0,
            "unsupported ZIP encoding"
        );
        ensure!(
            u16_at(bytes, cursor + 4)? >> 8 == 3 && u32_at(bytes, cursor + 38)? == (0o100600 << 16),
            "backup entries must be private regular files"
        );
        ensure!(
            [30, 32, 34, 36]
                .iter()
                .all(|offset| u16_at(bytes, cursor + offset).is_ok_and(|value| value == 0)),
            "unsupported ZIP entry metadata"
        );
        let size = u32_at(bytes, cursor + 24)?;
        ensure!(
            u32_at(bytes, cursor + 20)? == size
                && size
                    <= if name == "manifest.json" {
                        MAX_MANIFEST_BYTES
                    } else {
                        MAX_FILE_BYTES as usize
                    },
            "invalid ZIP entry size"
        );
        if name != "manifest.json" {
            total += size as u64;
            ensure!(total <= MAX_CONTENT_BYTES, "backup content exceeds 64 MiB");
        }
        ensure!(u32_at(bytes, cursor + 42)? == local, "noncontiguous ZIP local records");
        signature(bytes, local, 0x04034b50)?;
        ensure!(local + 30 + name_len <= central, "invalid ZIP local header range");
        // Version, flags, compression, timestamps, CRC and both lengths must agree.
        ensure!(
            bytes[local + 4..local + 26] == bytes[cursor + 6..cursor + 28]
                && u16_at(bytes, local + 26)? == name_len
                && u16_at(bytes, local + 28)? == 0
                && bytes[local + 30..local + 30 + name_len] == *name_bytes,
            "ZIP local/directory mismatch"
        );
        let start = local + 30 + name_len;
        local = start.checked_add(size).context("ZIP range overflow")?;
        ensure!(local <= central, "overlapping ZIP entry range");
        entries.push(Entry {
            name,
            data: &bytes[start..local],
        });
        cursor = next;
    }
    ensure!(local == central && cursor == end, "unaccounted ZIP records");
    Ok(entries)
}

fn text(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).context("invalid backup text encoding")
}
fn keys(value: &Value, allowed: &[&str]) -> Result<()> {
    let mapping = value.as_mapping().context("backup catalog requires mappings")?;
    ensure!(
        mapping
            .keys()
            .all(|key| key.as_str().is_some_and(|key| allowed.contains(&key))),
        "unknown backup catalog field"
    );
    Ok(())
}
fn catalog(bytes: &[u8]) -> Result<IProfiles> {
    let value: Value = serde_yaml_ng::from_str(text(bytes)?)?;
    keys(&value, &["current", "items"])?;
    if let Some(items) = value.get("items").filter(|value| !value.is_null()) {
        let items = items.as_sequence().context("invalid catalog items")?;
        ensure!(items.len() <= MAX_ENTRIES, "too many catalog items");
        for item in items {
            keys(
                item,
                &[
                    "uid", "type", "name", "file", "desc", "url", "selected", "extra", "updated", "option", "home",
                ],
            )?;
            if let Some(option) = item.get("option").filter(|value| !value.is_null()) {
                keys(
                    option,
                    &[
                        "user_agent",
                        "with_proxy",
                        "self_proxy",
                        "update_interval",
                        "timeout_seconds",
                        "danger_accept_invalid_certs",
                        "allow_auto_update",
                        "merge",
                        "script",
                        "rules",
                        "proxies",
                        "groups",
                    ],
                )?;
            }
            if let Some(extra) = item.get("extra").filter(|value| !value.is_null()) {
                keys(extra, &["upload", "download", "total", "expire"])?;
            }
            if let Some(selected) = item.get("selected").filter(|value| !value.is_null()) {
                for selected in selected.as_sequence().context("invalid selection records")? {
                    keys(selected, &["name", "now"])?;
                }
            }
        }
    }
    Ok(serde_yaml_ng::from_value(value)?)
}

fn configurations(entries: &BTreeMap<&str, &[u8]>, manifest: &BackupManifest, budget: &Budget) -> Result<usize> {
    let profiles = catalog(entries["profiles.yaml"])?;
    ensure!(
        entries["settings.yaml"].len() <= MAX_SETTINGS_BYTES,
        "backup settings exceeds 64 KiB"
    );
    let settings: ServiceSettings = serde_yaml_ng::from_str(text(entries["settings.yaml"])?)?;
    settings.validate()?;
    runtime::parse(text(entries["runtime.yaml"])?)?;
    budget.check()?;
    let mut uids = BTreeMap::new();
    let mut files = BTreeMap::new();
    let mut base_count = 0;
    for item in profiles.items.iter().flatten() {
        budget.check()?;
        let uid = item.uid.as_deref().context("missing catalog UID")?;
        ensure!(
            !uid.trim().is_empty()
                && uid.len() <= 256
                && !uid.chars().any(char::is_control)
                && uids.insert(uid, item).is_none(),
            "invalid or duplicate catalog UID"
        );
        let kind = item.itype.as_deref().context("missing catalog type")?;
        ensure!(
            matches!(
                kind,
                "local" | "remote" | "merge" | "script" | "rules" | "proxies" | "groups"
            ),
            "unsupported catalog type"
        );
        for (reserved, required) in [
            ("Merge", "merge"),
            ("Script", "script"),
            ("Rules", "rules"),
            ("Proxies", "proxies"),
            ("Groups", "groups"),
        ] {
            ensure!(uid != reserved || kind == required, "invalid reserved catalog type");
        }
        if matches!(kind, "local" | "remote") {
            base_count += 1;
        }
        if kind == "remote" {
            headless_core::config::remote::subscription_url(item.url.as_deref().context("missing subscription URL")?)?;
        }
        let file = item.file.as_deref().context("missing catalog file")?;
        validate_filename(file)?;
        let path = format!("profiles/{file}");
        if let Some(previous) = files.insert(path.clone(), kind) {
            ensure!(previous == kind, "conflicting catalog file types");
            continue; // Shared sources of the same type need validation only once.
        }
        let source = text(entries.get(path.as_str()).context("missing referenced profile file")?)?;
        match kind {
            "local" | "remote" => {
                runtime::parse_profile(source)?;
            }
            "merge" => {
                serde_yaml_ng::from_str::<serde_yaml_ng::Mapping>(source)?;
            }
            "rules" => SequenceKind::Rules.validate_source(source)?,
            "proxies" => SequenceKind::Proxies.validate_source(source)?,
            "groups" => SequenceKind::Groups.validate_source(source)?,
            "script" => headless_core::enhance::script::validate_source(source)?, // Bounds only; never execute uploaded scripts.
            _ => unreachable!(),
        }
    }
    ensure!(
        profiles.current.as_deref() == manifest.active_profile.as_deref(),
        "active profile/catalog mismatch"
    );
    if let Some(uid) = profiles.current.as_deref() {
        ensure!(
            uids.get(uid)
                .is_some_and(|item| matches!(item.itype.as_deref(), Some("local" | "remote"))),
            "invalid active profile reference"
        );
    }
    for item in profiles.items.iter().flatten() {
        if let Some(option) = &item.option {
            for (uid, kind) in [
                (&option.merge, "merge"),
                (&option.script, "script"),
                (&option.rules, "rules"),
                (&option.proxies, "proxies"),
                (&option.groups, "groups"),
            ] {
                if let Some(uid) = uid {
                    ensure!(
                        uids.get(uid.as_str())
                            .is_some_and(|item| item.itype.as_deref() == Some(kind)),
                        "invalid linked enhancement reference"
                    );
                }
            }
        }
    }
    ensure!(
        settings.profile_dns.keys().all(|uid| uids
            .get(uid.as_str())
            .is_some_and(|item| matches!(item.itype.as_deref(), Some("local" | "remote")))),
        "invalid settings profile reference"
    );
    ensure!(
        entries
            .keys()
            .filter(|name| name.starts_with("profiles/"))
            .all(|name| files.contains_key(*name)),
        "unreferenced profile file in archive"
    );
    Ok(base_count)
}

pub(crate) struct VerifiedArchive<'a> {
    pub report: BackupInspection,
    pub manifest: BackupManifest,
    pub contents: BTreeMap<&'a str, &'a [u8]>,
}

pub(crate) fn verify(
    bytes: &[u8],
    shutdown: watch::Receiver<bool>,
    closing: watch::Receiver<bool>,
) -> Result<VerifiedArchive<'_>> {
    let budget = Budget {
        deadline: Instant::now() + Duration::from_secs(15),
        shutdown,
        closing,
    };
    budget.check()?;
    let entries = structure(bytes, &budget)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    ensure!(archive.len() == entries.len(), "ZIP index mismatch");
    let mut contents = BTreeMap::new();
    // The pinned library's reader verifies CRC at EOF, without allocating file contents.
    let mut chunk = [0; 64 * 1024];
    for (index, entry) in entries.iter().enumerate() {
        let mut file = archive.by_index(index)?;
        ensure!(file.name_raw() == entry.name.as_bytes(), "ZIP index name mismatch");
        loop {
            budget.check()?;
            if file.read(&mut chunk)? == 0 {
                break;
            }
        }
        contents.insert(entry.name, entry.data);
    }
    let manifest: BackupManifest =
        serde_json::from_slice(contents.get("manifest.json").context("missing backup manifest")?)?;
    manifest.validate()?;
    ensure!(
        contents.len() == manifest.entries.len() + 1,
        "backup manifest entry count mismatch"
    );
    for entry in &manifest.entries {
        budget.check()?;
        let data = contents.get(entry.path.as_str()).context("missing manifest entry")?;
        ensure!(
            data.len() as u64 == entry.bytes && super::hash(data) == entry.sha256,
            "backup manifest length/digest mismatch"
        );
    }
    let profile_count = configurations(&contents, &manifest, &budget)?;
    let archive_sha256 = super::hash(bytes);
    budget.check()?;
    let report = BackupInspection {
        schema_version: manifest.schema_version,
        source: manifest.source.clone(),
        service_version: manifest.service_version.clone(),
        created_at: manifest.created_at,
        archive_bytes: bytes.len() as u64,
        archive_sha256,
        entry_count: entries.len(),
        content_bytes: manifest.entries.iter().map(|entry| entry.bytes).sum(),
        profile_count,
        active_profile_present: manifest.active_profile.is_some(),
        runtime_revision_present: manifest.runtime_revision.is_some(),
    };
    Ok(VerifiedArchive {
        report,
        manifest,
        contents,
    })
}

pub(crate) fn run(
    bytes: &[u8],
    shutdown: watch::Receiver<bool>,
    closing: watch::Receiver<bool>,
) -> Result<BackupInspection> {
    verify(bytes, shutdown, closing).map(|archive| archive.report)
}

#[cfg(test)]
#[path = "inspect_tests.rs"]
mod tests;
