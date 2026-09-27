use super::*;
use headless_core::backup::BackupEntry;
use std::io::Write as _;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn contents() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            "profiles.yaml".into(),
            b"current: main\nitems: [{uid: main, type: local, file: source.yaml}]\n".to_vec(),
        ),
        (
            "settings.yaml".into(),
            serde_yaml_ng::to_string(&ServiceSettings::default())
                .unwrap()
                .into_bytes(),
        ),
        (
            "runtime.yaml".into(),
            b"mode: direct\nrules: ['MATCH,DIRECT']\n".to_vec(),
        ),
        (
            "profiles/source.yaml".into(),
            b"# retained\nproxies: []\nexternal-controller: source.example:9090\n".to_vec(),
        ),
    ])
}
fn manifest(contents: &BTreeMap<String, Vec<u8>>) -> BackupManifest {
    BackupManifest {
        schema_version: 1,
        source: "mihomo-server".into(),
        service_version: "0.1.0".into(),
        created_at: 1,
        active_profile: Some("main".into()),
        runtime_revision: Some("revision.yaml".into()),
        entries: contents
            .iter()
            .map(|(path, bytes)| BackupEntry {
                path: path.clone(),
                bytes: bytes.len() as u64,
                sha256: crate::backup::hash(bytes),
            })
            .collect(),
    }
}
fn zip(mut contents: BTreeMap<String, Vec<u8>>, manifest: BackupManifest) -> Vec<u8> {
    contents.insert("manifest.json".into(), serde_json::to_vec(&manifest).unwrap());
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in contents {
        writer
            .start_file(
                name,
                SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Stored)
                    .unix_permissions(0o600),
            )
            .unwrap();
        writer.write_all(&data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
fn inspect(bytes: &[u8]) -> Result<BackupInspection> {
    run(bytes, watch::channel(false).1, watch::channel(false).1)
}
fn central(bytes: &[u8]) -> usize {
    u32_at(bytes, bytes.len() - 6).unwrap()
}
fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn valid_zip_reports_counts_without_returning_source_credentials_and_accepts_unicode_and_linked_types() -> Result<()> {
    let mut data = contents();
    data.insert("profiles.yaml".into(), b"current: main\nitems:\n- {uid: main, type: remote, url: 'https://example.invalid/private-token', file: source.yaml, option: {merge: m, script: s, rules: r, proxies: p, groups: g}}\n- {uid: m, type: merge, file: merge.yaml}\n- {uid: s, type: script, file: script.js}\n- {uid: r, type: rules, file: rules.yaml}\n- {uid: p, type: proxies, file: proxies.yaml}\n- {uid: g, type: groups, file: groups.yaml}\n".to_vec());
    data.insert(
        "profiles/merge.yaml".into(),
        "# Unicode content\nmode: rule\n".as_bytes().to_vec(),
    );
    data.insert(
        "profiles/script.js".into(),
        b"while(true) {} // inspection must never execute this\n".to_vec(),
    );
    for file in ["rules", "proxies", "groups"] {
        data.insert(
            format!("profiles/{file}.yaml"),
            b"prepend: []\nappend: []\ndelete: []\n".to_vec(),
        );
    }
    // Exercise UTF-8 filenames without changing catalog reference coherence.
    let data_source = data.remove("profiles/merge.yaml").unwrap();
    data.insert("profiles/合并.yaml".into(), data_source);
    let catalog = String::from_utf8(data["profiles.yaml"].clone())?.replace("merge.yaml", "合并.yaml");
    data.insert("profiles.yaml".into(), catalog.into_bytes());
    let bytes = zip(data.clone(), manifest(&data));
    let report = inspect(&bytes)?;
    assert_eq!(report.profile_count, 1);
    assert_eq!(report.entry_count, 10);
    assert_eq!(report.archive_bytes, bytes.len() as u64);
    assert_eq!(report.archive_sha256, crate::backup::hash(&bytes));
    assert_eq!(report.content_bytes, data.values().map(|b| b.len() as u64).sum::<u64>());
    assert!(report.active_profile_present && report.runtime_revision_present);
    let json = serde_json::to_string(&report)?;
    assert!(!json.contains("private-token") && !json.contains("source.yaml") && !json.contains("main"));
    Ok(())
}

#[test]
fn ambiguous_unsupported_and_corrupt_zip_records_are_rejected_before_content_parsing() {
    let data = contents();
    let good = zip(data.clone(), manifest(&data));
    inspect(&good).unwrap();
    for case in [
        "truncated",
        "trailer",
        "prefix",
        "directory-size",
        "multi-disk",
        "comment",
        "count",
        "zip64",
        "overlap",
        "encrypted",
        "descriptor",
        "deflate",
        "mode",
        "symlink",
        "extra",
        "local-name",
        "local-crc",
        "crc",
        "oversize",
        "duplicate",
    ] {
        let mut bytes = good.clone();
        let c = central(&bytes);
        let end = bytes.len() - 22;
        match case {
            "truncated" => {
                bytes.pop();
            }
            "trailer" => bytes.push(0),
            "prefix" => bytes.insert(0, 0),
            "directory-size" => put32(&mut bytes, end + 12, 0),
            "multi-disk" => put16(&mut bytes, end + 4, 1),
            "comment" => put16(&mut bytes, end + 20, 1),
            "count" => put16(&mut bytes, end + 10, MAX_ENTRIES as u16 + 1),
            "zip64" => put32(&mut bytes, end + 16, u32::MAX),
            "overlap" => put32(&mut bytes, c + 42, 1),
            "encrypted" => put16(&mut bytes, c + 8, 1),
            "descriptor" => put16(&mut bytes, c + 8, 8),
            "deflate" => put16(&mut bytes, c + 10, 8),
            "mode" => put32(&mut bytes, c + 38, 0o100666 << 16),
            "symlink" => put32(&mut bytes, c + 38, 0o120600 << 16),
            "extra" => put16(&mut bytes, c + 30, 1),
            "local-name" => bytes[30] = b'x',
            "local-crc" => put32(&mut bytes, 14, 0),
            "crc" => {
                put32(&mut bytes, 14, 0);
                put32(&mut bytes, c + 16, 0);
            }
            "oversize" => {
                put32(&mut bytes, c + 20, MAX_FILE_BYTES as u32 + 1);
                put32(&mut bytes, c + 24, MAX_FILE_BYTES as u32 + 1);
            }
            "duplicate" => {
                let mut cursor = c;
                while cursor < end {
                    let n = u16_at(&bytes, cursor + 28).unwrap();
                    if &bytes[cursor + 46..cursor + 46 + n] == b"settings.yaml" {
                        let l = u32_at(&bytes, cursor + 42).unwrap();
                        bytes[cursor + 46..cursor + 46 + n].copy_from_slice(b"profiles.yaml");
                        bytes[l + 30..l + 30 + n].copy_from_slice(b"profiles.yaml");
                        break;
                    }
                    cursor += 46 + n;
                }
            }
            _ => unreachable!(),
        }
        assert!(inspect(&bytes).is_err(), "{case}");
    }
}

#[test]
fn manifest_and_path_integrity_fail_even_when_zip_crc_is_valid() {
    for case in [
        "schema",
        "digest",
        "length",
        "missing",
        "extra",
        "duplicate",
        "traversal",
        "backslash",
        "absolute",
        "private",
        "unknown-field",
        "missing-manifest",
    ] {
        let mut data = contents();
        let mut m = manifest(&data);
        match case {
            "schema" => m.schema_version = 2,
            "digest" => m.entries[0].sha256 = "a".repeat(64),
            "length" => m.entries[0].bytes += 1,
            "missing" => {
                data.remove("profiles/source.yaml");
            }
            "extra" => {
                data.insert("profiles/unlisted.yaml".into(), b"{}".to_vec());
            }
            "duplicate" => m.entries.push(m.entries[0].clone()),
            "traversal" | "backslash" | "absolute" | "private" => {
                let path = match case {
                    "traversal" => "profiles/../secret",
                    "backslash" => "profiles/a\\b",
                    "absolute" => "/profiles/source.yaml",
                    _ => "management-token",
                };
                let old = data.remove("profiles/source.yaml").unwrap();
                data.insert(path.into(), old);
                m = manifest(&data);
            }
            "unknown-field" | "missing-manifest" => {}
            _ => unreachable!(),
        }
        let mut bytes = zip(data, m);
        if matches!(case, "unknown-field" | "missing-manifest") {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let mut data = BTreeMap::new();
            for index in 0..archive.len() {
                let mut file = archive.by_index(index).unwrap();
                let mut source = Vec::new();
                file.read_to_end(&mut source).unwrap();
                data.insert(file.name().to_string(), source);
            }
            let source = data.remove("manifest.json").unwrap();
            let mut value: serde_json::Value = serde_json::from_slice(&source).unwrap();
            value["destination"] = serde_json::json!("/tmp/secret");
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            if case == "unknown-field" {
                data.insert("manifest.json".into(), serde_json::to_vec(&value).unwrap());
            }
            for (name, source) in data {
                writer
                    .start_file(
                        name,
                        SimpleFileOptions::default()
                            .compression_method(CompressionMethod::Stored)
                            .unix_permissions(0o600),
                    )
                    .unwrap();
                writer.write_all(&source).unwrap();
            }
            bytes = writer.finish().unwrap().into_inner();
        }
        assert!(inspect(&bytes).is_err(), "{case}");
    }
}

#[test]
fn self_consistent_digests_cannot_hide_invalid_catalog_references_or_configuration() {
    for case in [
        "uid",
        "duplicate-uid",
        "unknown-catalog",
        "unknown-option",
        "missing-file",
        "orphan",
        "active",
        "link",
        "link-type",
        "reserved",
        "runtime",
        "source",
        "merge",
        "sequence",
        "script",
        "settings",
        "settings-profile",
        "url",
    ] {
        let mut data = contents();
        match case {
            "uid" => {
                data.insert(
                    "profiles.yaml".into(),
                    b"items: [{type: local, file: source.yaml}]".to_vec(),
                );
            }
            "duplicate-uid" => {
                data.insert("profiles.yaml".into(), b"current: main\nitems: [{uid: main, type: local, file: source.yaml}, {uid: main, type: local, file: source.yaml}]".to_vec());
            }
            "unknown-catalog" => {
                data.insert(
                    "profiles.yaml".into(),
                    b"current: main\nitems: [{uid: main, type: local, file: source.yaml, file_data: secret}]".to_vec(),
                );
            }
            "unknown-option" => {
                data.insert("profiles.yaml".into(), b"current: main\nitems: [{uid: main, type: local, file: source.yaml, option: {destination: secret}}]".to_vec());
            }
            "missing-file" => {
                data.remove("profiles/source.yaml");
            }
            "orphan" => {
                data.insert("profiles/orphan.yaml".into(), b"{}".to_vec());
            }
            "active" => {
                data.insert(
                    "profiles.yaml".into(),
                    b"items: [{uid: main, type: local, file: source.yaml}]".to_vec(),
                );
            }
            "link" | "link-type" => {
                data.insert(
                    "profiles.yaml".into(),
                    format!(
                        "current: main\nitems: [{{uid: main, type: local, file: source.yaml, option: {{merge: {}}}}}]",
                        if case == "link" { "missing" } else { "main" }
                    )
                    .into_bytes(),
                );
            }
            "reserved" => {
                data.insert(
                    "profiles.yaml".into(),
                    b"items: [{uid: Merge, type: script, file: source.yaml}]".to_vec(),
                );
            }
            "runtime" => {
                data.insert("runtime.yaml".into(), b"external-controller: 127.0.0.1:9090".to_vec());
            }
            "source" => {
                data.insert("profiles/source.yaml".into(), b"[not, a, mapping]".to_vec());
            }
            "merge" | "sequence" | "script" => {
                let kind = if case == "sequence" { "rules" } else { case };
                data.insert("profiles.yaml".into(), format!("current: main\nitems: [{{uid: main, type: local, file: source.yaml, option: {{{kind}: aux}}}}, {{uid: aux, type: {kind}, file: aux.yaml}}]").into_bytes());
                data.insert(
                    "profiles/aux.yaml".into(),
                    if case == "script" { Vec::new() } else { b"[]".to_vec() },
                );
            }
            "settings" => {
                data.insert("settings.yaml".into(), b"schema_version: 2".to_vec());
            }
            "settings-profile" => {
                data.insert(
                    "settings.yaml".into(),
                    b"schema_version: 1\nprofile_dns: {missing: {enabled: false}}".to_vec(),
                );
            }
            "url" => {
                data.insert(
                    "profiles.yaml".into(),
                    b"current: main\nitems: [{uid: main, type: remote, file: source.yaml, url: 'file:///private'}]"
                        .to_vec(),
                );
            }
            _ => unreachable!(),
        }
        let bytes = zip(data.clone(), manifest(&data));
        assert!(inspect(&bytes).is_err(), "{case}");
    }
}

#[test]
fn cancellation_deadlines_and_manifest_settings_bounds_fail_closed() {
    let data = contents();
    let bytes = zip(data.clone(), manifest(&data));
    assert!(run(&bytes, watch::channel(true).1, watch::channel(false).1).is_err());
    assert!(run(&bytes, watch::channel(false).1, watch::channel(true).1).is_err());
    let expired = Budget {
        deadline: Instant::now(),
        shutdown: watch::channel(false).1,
        closing: watch::channel(false).1,
    };
    assert!(expired.check().is_err());
    let mut data = contents();
    data.insert("settings.yaml".into(), vec![b' '; MAX_SETTINGS_BYTES + 1]);
    assert!(inspect(&zip(data.clone(), manifest(&data))).is_err());
    let mut data = contents();
    data.insert("profiles/extra.yaml".into(), vec![b' '; MAX_FILE_BYTES as usize + 1]);
    assert!(inspect(&zip(data.clone(), manifest(&data))).is_err());
}
