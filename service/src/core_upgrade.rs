//! Durable Linux managed-core replacement. Paths are fixed under the private core directory.
use crate::{core_release::StagedCore, resources::TARGET};
use anyhow::{Context as _, Result, ensure};
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::Path,
};
const MAX_CORE: u64 = 128 * 1024 * 1024;
const MAX_RECORD: u64 = 16 * 1024;
const TRANSACTION: &str = ".core-upgrade";
const RECEIPT: &str = ".core-installation.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoreInstallation {
    pub stage_id: String,
    pub version: String,
    pub target: String,
    pub executable_bytes: u64,
    pub executable_sha256: String,
    pub config_sha256: String,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Pending,
    Committed,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    phase: Phase,
    was_running: bool,
    previous_sha256: String,
    previous_mode: u32,
    previous_installation: Option<CoreInstallation>,
    installation: CoreInstallation,
}
pub(crate) fn config_hash(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&SHA256, bytes).as_ref())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn metadata(path: &Path, limit: u64, private: bool) -> Result<fs::Metadata> {
    let m = fs::symlink_metadata(path)?;
    ensure!(m.is_file() && m.len() <= limit, "unsafe core upgrade file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        ensure!(
            m.permissions().mode() & if private { 0o077 } else { 0o022 } == 0,
            "unsafe core upgrade file permissions"
        );
    }
    Ok(m)
}
fn directory(path: &Path) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    ensure!(m.is_dir(), "core upgrade directory must be real");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        ensure!(
            m.permissions().mode() & 0o077 == 0,
            "core upgrade directory must be private"
        );
    }
    Ok(())
}
fn create_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut b = fs::DirBuilder::new();
        b.mode(0o700);
        b.create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(path)?;
    directory(path)
}
fn sync(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}
fn file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn remove_owned(path: &Path, limit: u64) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            metadata(path, limit, true)?;
            fs::remove_file(path)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    };
    Ok(())
}
fn digest(path: &Path, private: bool) -> Result<(u64, String)> {
    let m = metadata(path, MAX_CORE, private)?;
    ensure!(m.len() > 0, "empty managed core");
    let mut input = File::open(path)?;
    let mut buffer = [0; 65536];
    let mut count = 0;
    let mut hash = Context::new(&SHA256);
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        ensure!(n as u64 <= MAX_CORE - count, "managed core exceeds 128 MiB");
        count += n as u64;
        hash.update(&buffer[..n]);
    }
    ensure!(count == m.len(), "managed core changed while reading");
    Ok((count, hex(hash.finish().as_ref())))
}
fn copy(source: &Path, destination: &Path, expected: &str, mode: u32, private: bool) -> Result<()> {
    metadata(source, MAX_CORE, private)?;
    let mut input = File::open(source)?.take(MAX_CORE + 1);
    let mut output = file(destination)?;
    let mut count = 0;
    let mut hash = Context::new(&SHA256);
    let mut buffer = [0; 65536];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        ensure!(n as u64 <= MAX_CORE - count, "core backup exceeds size bounds");
        count += n as u64;
        hash.update(&buffer[..n]);
        output.write_all(&buffer[..n])?;
    }
    ensure!(
        count > 0 && hex(hash.finish().as_ref()) == expected,
        "core changed while making upgrade backup"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        output.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    output.sync_all()?;
    Ok(())
}
fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    metadata(path, MAX_RECORD, true)?;
    let mut bytes = Vec::new();
    File::open(path)?.take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_RECORD, "core upgrade record exceeds bounds");
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid core upgrade record"))
}
fn validate_installation(r: &CoreInstallation) -> Result<()> {
    ensure!(
        r.target == TARGET
            && r.executable_bytes > 0
            && r.executable_bytes <= MAX_CORE
            && hash_valid(&r.executable_sha256)
            && hash_valid(&r.config_sha256),
        "invalid core installation record"
    );
    let (package, config) = r.stage_id.rsplit_once('-').context("invalid installation stage ID")?;
    let (version, package_hash) = package.split_once('-').context("invalid installation stage ID")?;
    let valid_version = version.strip_prefix('v').is_some_and(|s| {
        let parts: Vec<_> = s.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    });
    ensure!(
        version.len() <= 64
            && valid_version
            && version == r.version
            && hash_valid(package_hash)
            && config == r.config_sha256,
        "invalid installation stage ID"
    );
    Ok(())
}
fn journal(root: &Path) -> Result<Journal> {
    directory(root)?;
    let j: Journal = read_record(&root.join("journal.json"))?;
    ensure!(
        j.schema_version == 1
            && hash_valid(&j.previous_sha256)
            && j.previous_mode & 0o100 != 0
            && j.previous_mode & !0o755 == 0,
        "invalid core upgrade journal"
    );
    validate_installation(&j.installation)?;
    if let Some(previous) = &j.previous_installation {
        validate_installation(previous)?;
        ensure!(
            previous.executable_sha256 == j.previous_sha256,
            "previous core receipt/hash conflict"
        );
    }
    Ok(j)
}
fn write_journal(root: &Path, j: &Journal) -> Result<()> {
    let next = root.join("journal.next");
    remove_owned(&next, MAX_RECORD)?;
    let mut output = file(&next)?;
    output.write_all(&serde_json::to_vec(j)?)?;
    output.sync_all()?;
    if fs::symlink_metadata(root.join("journal.json")).is_ok() {
        metadata(&root.join("journal.json"), MAX_RECORD, true)?;
    }
    fs::rename(next, root.join("journal.json"))?;
    sync(root)
}
fn write_receipt(core: &Path, root: &Path, receipt: Option<&CoreInstallation>) -> Result<()> {
    let destination = core.join(RECEIPT);
    if fs::symlink_metadata(&destination).is_ok() {
        metadata(&destination, MAX_RECORD, true)?;
    }
    match receipt {
        Some(receipt) => {
            let next = root.join("receipt.next");
            remove_owned(&next, MAX_RECORD)?;
            let mut output = file(&next)?;
            output.write_all(&serde_json::to_vec(receipt)?)?;
            output.sync_all()?;
            fs::rename(next, destination)?;
            sync(core)?;
        }
        None => {
            remove_owned(&destination, MAX_RECORD)?;
            sync(core)?;
        }
    }
    Ok(())
}
fn owned_files(root: &Path) -> Result<Vec<std::path::PathBuf>> {
    directory(root)?;
    // Validate the entire set before deleting anything. Unknown files/links fail closed.
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        let e = entry?;
        let name = e.file_name();
        let name = name.to_str().context("unknown core upgrade file")?;
        let limit = match name {
            "previous" | "candidate" | "rollback" => MAX_CORE,
            "journal.json" | "journal.next" | "receipt.next" => MAX_RECORD,
            _ => anyhow::bail!("unknown core upgrade file"),
        };
        metadata(&e.path(), limit, true)?;
        paths.push(e.path());
    }
    Ok(paths)
}
fn clean(core: &Path, root: &Path) -> Result<()> {
    for path in owned_files(root)? {
        fs::remove_file(path)?;
    }
    fs::remove_dir(root)?;
    sync(core)
}
/// None means clean or committed; Some(was_running) means an uncommitted switch was rolled back.
pub(crate) fn recover(core: &Path) -> Result<Option<bool>> {
    match fs::symlink_metadata(core) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(_) => directory(core)?,
    };
    let root = core.join(TRANSACTION);
    match fs::symlink_metadata(&root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(_) => directory(&root)?,
    };
    owned_files(&root)?;
    if !root.join("journal.json").try_exists()? {
        clean(core, &root)?;
        return Ok(None);
    }
    let j = journal(&root)?;
    let live = core.join("verge-mihomo");
    let current = match fs::symlink_metadata(&live) {
        Ok(_) => Some(digest(&live, false)?.1),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if j.phase == Phase::Committed {
        ensure!(
            current.as_deref() == Some(&j.installation.executable_sha256)
                && metadata(&live, MAX_CORE, false)?.len() == j.installation.executable_bytes,
            "committed core hash/size conflict; recovery required"
        );
        write_receipt(core, &root, Some(&j.installation))?;
        clean(core, &root)?;
        return Ok(None);
    }
    ensure!(
        current
            .as_deref()
            .is_none_or(|hash| hash == j.previous_sha256 || hash == j.installation.executable_sha256),
        "unexpected live core hash; recovery required"
    );
    if current.as_deref() != Some(&j.previous_sha256) {
        let restore = root.join("rollback");
        remove_owned(&restore, MAX_CORE)?;
        copy(&root.join("previous"), &restore, &j.previous_sha256, 0o700, true)?;
        fs::rename(restore, &live)?;
        sync(core)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&live, fs::Permissions::from_mode(j.previous_mode))?;
        File::open(&live)?.sync_all()?;
    }
    write_receipt(core, &root, j.previous_installation.as_ref())?;
    clean(core, &root)?;
    Ok(Some(j.was_running))
}
pub(crate) fn pending(core: &Path) -> Result<bool> {
    let root = core.join(TRANSACTION);
    if fs::symlink_metadata(&root).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(false);
    }
    directory(&root)?;
    if !root.join("journal.json").try_exists()? {
        return Ok(false);
    }
    Ok(journal(&root)?.phase == Phase::Pending)
}
pub(crate) fn installation(core: &Path) -> Result<Option<CoreInstallation>> {
    let path = core.join(RECEIPT);
    if fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(None);
    }
    let r: CoreInstallation = read_record(&path)?;
    validate_installation(&r)?;
    let (bytes, hash) = digest(&core.join("verge-mihomo"), false)?;
    ensure!(
        bytes == r.executable_bytes && hash == r.executable_sha256,
        "installed core receipt/hash mismatch"
    );
    Ok(Some(r))
}
pub(crate) fn prepare(core: &Path, source: &Path, staged: &StagedCore, was_running: bool) -> Result<CoreInstallation> {
    ensure!(
        matches!(TARGET, "x86_64-unknown-linux-gnu" | "x86_64-unknown-linux-musl"),
        "core activation currently requires Linux x86_64"
    );
    directory(core)?;
    ensure!(!core.join(TRANSACTION).try_exists()?, "core upgrade recovery required");
    let live = core.join("verge-mihomo");
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let name = std::ffi::CString::new(live.as_os_str().as_bytes())?;
        // Private copies intentionally do not transfer privilege-bearing xattrs.
        let size = unsafe { libc::getxattr(name.as_ptr(), c"security.capability".as_ptr(), std::ptr::null_mut(), 0) };
        ensure!(
            size <= 0,
            "capability-bearing cores require native privilege integration before upgrading"
        );
        if size < 0 {
            let error = std::io::Error::last_os_error();
            ensure!(
                matches!(error.raw_os_error(), Some(libc::ENODATA | libc::ENOTSUP)),
                "cannot inspect managed core capabilities"
            );
        }
    }
    let (_, previous_sha256) = digest(&live, false)?;
    #[cfg(unix)]
    let previous_mode = {
        use std::os::unix::fs::PermissionsExt as _;
        fs::metadata(&live)?.permissions().mode() & 0o7777
    };
    #[cfg(not(unix))]
    let previous_mode = 0o700;
    ensure!(
        previous_mode & !0o755 == 0 && previous_mode & 0o100 != 0,
        "unsupported previous core permissions"
    );
    let previous_installation = installation(core)?;
    let r = CoreInstallation {
        stage_id: staged.stage_id.clone(),
        version: staged.prepared.release.version.clone(),
        target: staged.prepared.release.target.clone(),
        executable_bytes: staged.executable_bytes,
        executable_sha256: staged.executable_sha256.clone(),
        config_sha256: staged.config_sha256.clone(),
    };
    validate_installation(&r)?;
    let root = core.join(TRANSACTION);
    create_directory(&root)?;
    copy(&live, &root.join("previous"), &previous_sha256, 0o700, false)?;
    copy(source, &root.join("candidate"), &r.executable_sha256, 0o700, true)?;
    write_journal(
        &root,
        &Journal {
            schema_version: 1,
            phase: Phase::Pending,
            was_running,
            previous_sha256,
            previous_mode,
            previous_installation,
            installation: r.clone(),
        },
    )?;
    sync(core)?;
    Ok(r)
}
pub(crate) fn publish(core: &Path) -> Result<()> {
    let root = core.join(TRANSACTION);
    let j = journal(&root)?;
    ensure!(j.phase == Phase::Pending, "core upgrade already committed");
    ensure!(
        digest(&core.join("verge-mihomo"), false)?.1 == j.previous_sha256,
        "live core changed before replacement"
    );
    ensure!(
        digest(&root.join("previous"), true)?.1 == j.previous_sha256
            && digest(&root.join("candidate"), true)?
                == (
                    j.installation.executable_bytes,
                    j.installation.executable_sha256.clone()
                ),
        "core upgrade backup/candidate integrity failure"
    );
    fs::rename(root.join("candidate"), core.join("verge-mihomo"))?;
    sync(core)
}
pub(crate) fn commit(core: &Path) -> Result<()> {
    let root = core.join(TRANSACTION);
    let mut j = journal(&root)?;
    ensure!(
        digest(&core.join("verge-mihomo"), false)?
            == (
                j.installation.executable_bytes,
                j.installation.executable_sha256.clone()
            ),
        "activated core hash mismatch"
    );
    j.phase = Phase::Committed;
    write_journal(&root, &j)
}

#[cfg(test)]
#[path = "core_upgrade_tests.rs"]
mod tests;
