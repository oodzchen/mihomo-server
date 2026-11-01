//! Transport-independent credentials, same-origin request policy and
//! management token rotation.

use super::preferences::{read_json, write_json};
use crate::secure_fs::{create_private, hex, sha256_hex, sync_directory};
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{PoisonError, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, Notify, watch};
use url::Url;

/// Rotation period of a new instance, and of one that has no recorded choice.
pub const DEFAULT_ROTATION_DAYS: u32 = 30;
pub const MAX_ROTATION_DAYS: u32 = 3650;
const DAY: u64 = 86_400;
/// Clients are reminded this long before a rotation, or a quarter of a shorter period.
const MAX_REMINDER: u64 = 7 * DAY;
/// HTTP requests already sent with the previous token still pass for this
/// long, so a client that read the token file just before a rotation is not
/// refused. WebSocket sessions never use the previous token.
const PREVIOUS_TOKEN_GRACE: Duration = Duration::from_secs(10);
/// The rotation task checks the wall clock at least this often (suspend does
/// not advance tokio timers).
const CHECK_INTERVAL: Duration = Duration::from_secs(600);
const RETRY_INTERVAL: Duration = Duration::from_secs(60);

/// Persisted beside the token file. The digest ties the record to the token
/// it describes: a token replaced outside the service counts as issued when
/// the service first sees it, keeping the chosen period.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RotationRecord {
    /// `None` (JSON null): the token never rotates automatically.
    #[serde(default = "default_period")]
    period_days: Option<u32>,
    /// Unix seconds.
    issued_at: u64,
    token_sha256: String,
}

fn default_period() -> Option<u32> {
    Some(DEFAULT_ROTATION_DAYS)
}

fn valid_period(period: Option<u32>) -> bool {
    period.is_none_or(|days| (1..=MAX_ROTATION_DAYS).contains(&days))
}

/// The rotation policy and schedule of the current token (Unix seconds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenRotation {
    pub period_days: Option<u32>,
    pub default_period_days: u32,
    pub issued_at: u64,
    pub expires_at: Option<u64>,
    pub remind_at: Option<u64>,
}

impl TokenRotation {
    fn of(record: &RotationRecord) -> Self {
        let period = record.period_days.map(|days| u64::from(days) * DAY);
        let expires_at = period.map(|period| record.issued_at.saturating_add(period));
        Self {
            period_days: record.period_days,
            default_period_days: DEFAULT_ROTATION_DAYS,
            issued_at: record.issued_at,
            expires_at,
            remind_at: period
                .zip(expires_at)
                .map(|(period, expires)| expires.saturating_sub(MAX_REMINDER.min(period / 4))),
        }
    }

    fn due(&self, now: u64) -> bool {
        self.expires_at.is_some_and(|expires| now >= expires)
    }
}

/// A rotation answer: the new token only for the client that caused it.
/// Never implements Debug: it can carry a credential.
#[derive(Serialize)]
pub struct RotationReply {
    #[serde(flatten)]
    pub rotation: TokenRotation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

struct TokenState {
    token: String,
    previous: Option<(String, Instant)>,
    /// Increases with every rotation; WebSocket sessions record theirs.
    generation: u64,
    record: RotationRecord,
}

/// Never implements Debug or Serialize: credentials must not enter state/logs.
pub struct Authentication {
    origin: String,
    authority: String,
    token_path: PathBuf,
    record_path: PathBuf,
    state: RwLock<TokenState>,
    rotations: watch::Sender<u64>,
    changed: Notify,
    write: Mutex<()>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn generate_token() -> Result<String> {
    let mut entropy = [0u8; 32];
    getrandom::fill(&mut entropy).context("generate management token")?;
    Ok(hex(&entropy))
}

/// `management-token` → `management-token.json`.
fn record_path(token_path: &Path) -> PathBuf {
    let mut name = token_path.file_name().unwrap_or_default().to_owned();
    name.push(".json");
    token_path.with_file_name(name)
}

/// Always compare all stored bytes; never return on the first mismatch.
fn same(stored: &str, supplied: &str) -> bool {
    let mut difference = supplied.len() ^ stored.len();
    for (index, byte) in stored.bytes().enumerate() {
        difference |= usize::from(byte ^ supplied.as_bytes().get(index).copied().unwrap_or(0));
    }
    difference == 0
}

/// First creation never overwrites: the token is published using a
/// non-overwriting hard link so interrupted writes cannot publish it.
fn create_token(path: &Path) -> Result<()> {
    let token = generate_token()?;
    let temp = path.with_extension(format!("{}.tmp", &token[..16]));
    let mut file = create_private(&temp).context("create private token file")?;
    let result = (|| -> Result<()> {
        writeln!(file, "{token}")?;
        file.sync_all()?;
        match fs::hard_link(&temp, path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        sync_directory(path.parent().unwrap_or(Path::new(".")))?;
        Ok(())
    })();
    drop(file);
    let _ = fs::remove_file(&temp);
    result.context("publish management token")
}

/// Rotation replaces the file atomically: readers see the old or the new token.
fn replace_token(path: &Path, token: &str) -> Result<()> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(format!(".{nanos}.tmp"));
    let temp = path.with_file_name(name);
    let result = (|| -> Result<()> {
        let mut file = create_private(&temp).context("create private token file")?;
        writeln!(file, "{token}")?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        sync_directory(path.parent().unwrap_or(Path::new(".")))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.context("replace management token")
}

fn read_token(path: &Path) -> Result<String> {
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "token file must not be a symlink"
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).context("read management token")?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file() && metadata.len() <= 128, "invalid token file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "token file must be private (chmod 600)"
        );
    }
    let mut token = String::new();
    file.take(129).read_to_string(&mut token)?;
    let token = token.trim().to_owned();
    ensure!(
        token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "token must contain exactly 64 hexadecimal characters"
    );
    Ok(token)
}

impl Authentication {
    /// Must run while the service owns its data directory.
    pub fn load_or_create(path: &Path, listen: SocketAddr, public_origin: Option<&str>) -> Result<Self> {
        ensure!(listen.port() != 0, "management port must be explicit and nonzero");
        let origin = public_origin
            .map(str::to_owned)
            .unwrap_or_else(|| format!("http://{listen}"));
        ensure!(
            !listen.ip().is_unspecified() || public_origin.is_some(),
            "wildcard listener requires an explicit public origin"
        );
        let mut parsed = Url::parse(&origin).context("invalid management origin")?;
        ensure!(
            matches!(parsed.scheme(), "http" | "https")
                && parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.query().is_none()
                && parsed.fragment().is_none()
                && parsed.path() == "/",
            "management origin must contain only scheme and authority"
        );
        parsed.set_path("");
        let origin = parsed.origin().ascii_serialization();
        let authority = origin
            .split_once("://")
            .context("invalid management origin")?
            .1
            .to_owned();
        if !path.try_exists()? {
            create_token(path)?;
        }
        let token = read_token(path)?;

        let record_path = record_path(path);
        let digest = sha256_hex(token.as_bytes());
        let stored = read_json::<Option<RotationRecord>>(&record_path, "token rotation").unwrap_or_else(|error| {
            eprintln!("ignoring token rotation record {}: {error:#}", record_path.display());
            None
        });
        let stored = stored.filter(|record| valid_period(record.period_days));
        let record = match stored {
            Some(record) if record.token_sha256 == digest => record,
            stored => {
                // A new token (or an existing installation's first start with
                // rotation): its period starts now.
                let record = RotationRecord {
                    period_days: stored.map_or(default_period(), |record| record.period_days),
                    issued_at: unix_now(),
                    token_sha256: digest,
                };
                if let Err(error) = write_json(&record_path, &record, "token rotation") {
                    eprintln!("cannot record token rotation: {error:#}");
                }
                record
            }
        };
        Ok(Self {
            origin,
            authority,
            token_path: path.to_owned(),
            record_path,
            state: RwLock::new(TokenState {
                token,
                previous: None,
                generation: 0,
                record,
            }),
            rotations: watch::channel(0).0,
            changed: Notify::new(),
            write: Mutex::new(()),
        })
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// HTTP adapters must reject duplicate Host/Origin/Authorization headers
    /// before calling this method. No query token, cookies or CORS bypass.
    pub fn authorize(&self, host: &str, origin: Option<&str>, authorization: Option<&str>) -> Result<()> {
        self.authorize_origin(host, origin)?;
        let supplied = authorization
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or("");
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let current = same(&state.token, supplied);
        let previous = state
            .previous
            .as_ref()
            .is_some_and(|(token, rotated)| rotated.elapsed() < PREVIOUS_TOKEN_GRACE && same(token, supplied));
        ensure!(current || previous, "authentication required");
        Ok(())
    }

    /// WebSocket upgrades check origin before waiting for a browser's first frame.
    pub fn authorize_origin(&self, host: &str, origin: Option<&str>) -> Result<()> {
        ensure!(host == self.authority, "management host is not allowed");
        ensure!(
            origin.is_none_or(|value| value == self.origin),
            "browser origin is not allowed"
        );
        Ok(())
    }

    pub fn authorize_token(&self, supplied: &str) -> Result<()> {
        self.authorize_session(supplied).map(drop)
    }

    /// The current token only; returns the token generation the session ends with.
    pub fn authorize_session(&self, supplied: &str) -> Result<u64> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        ensure!(same(&state.token, supplied), "authentication required");
        Ok(state.generation)
    }

    /// Changes whenever the token rotates.
    pub fn subscribe_rotations(&self) -> watch::Receiver<u64> {
        self.rotations.subscribe()
    }

    pub fn rotation(&self) -> TokenRotation {
        TokenRotation::of(&self.state.read().unwrap_or_else(PoisonError::into_inner).record)
    }

    /// Replace the token now; the caller receives the new one.
    pub async fn rotate(&self) -> Result<RotationReply> {
        let _write = self.write.lock().await;
        let period = self.rotation().period_days;
        self.replace(period).await
    }

    /// Change the period, counted from the current token's issue. A period
    /// already exceeded rotates now and gives the caller the new token.
    pub async fn set_rotation_period(&self, period: Option<u32>) -> Result<RotationReply> {
        ensure!(
            valid_period(period),
            "rotation period must be 1 to {MAX_ROTATION_DAYS} days, or null for never"
        );
        let _write = self.write.lock().await;
        let mut record = self.state.read().unwrap_or_else(PoisonError::into_inner).record.clone();
        record.period_days = period;
        if TokenRotation::of(&record).due(unix_now()) {
            return self.replace(period).await;
        }
        let (path, saved) = (self.record_path.clone(), record.clone());
        tokio::task::spawn_blocking(move || write_json(&path, &saved, "token rotation")).await??;
        self.state.write().unwrap_or_else(PoisonError::into_inner).record = record;
        self.changed.notify_one();
        Ok(RotationReply {
            rotation: self.rotation(),
            token: None,
        })
    }

    /// Rotate when the period has run out; true when it did.
    pub async fn rotate_if_due(&self) -> Result<bool> {
        let _write = self.write.lock().await;
        let rotation = self.rotation();
        if !rotation.due(unix_now()) {
            return Ok(false);
        }
        self.replace(rotation.period_days).await?;
        Ok(true)
    }

    /// Run while the service runs: rotates the token whenever it is due.
    pub async fn run_rotation(&self, mut closing: watch::Receiver<bool>) {
        loop {
            if *closing.borrow_and_update() {
                return;
            }
            let wait = match self.rotate_if_due().await {
                Ok(rotated) => {
                    if rotated {
                        eprintln!("management token rotated; clients read the new token file");
                    }
                    self.rotation()
                        .expires_at
                        .map_or(CHECK_INTERVAL, |expires| {
                            Duration::from_secs(expires.saturating_sub(unix_now()))
                        })
                        .min(CHECK_INTERVAL)
                }
                Err(error) => {
                    eprintln!("cannot rotate management token: {error:#}");
                    RETRY_INTERVAL
                }
            };
            tokio::select! {
                changed = closing.changed() => if changed.is_err() { return },
                () = tokio::time::sleep(wait) => {}
                () = self.changed.notified() => {}
            }
        }
    }

    /// Callers hold the write lock. Once the new token file is in place the
    /// service accepts only it (apart from the short HTTP grace), even when
    /// its record cannot be saved: clients read the file.
    async fn replace(&self, period: Option<u32>) -> Result<RotationReply> {
        let token = generate_token()?;
        let record = RotationRecord {
            period_days: period,
            issued_at: unix_now(),
            token_sha256: sha256_hex(token.as_bytes()),
        };
        let (path, written) = (self.token_path.clone(), token.clone());
        tokio::task::spawn_blocking(move || replace_token(&path, &written)).await??;
        let generation = {
            let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
            let previous = std::mem::replace(&mut state.token, token.clone());
            state.previous = Some((previous, Instant::now()));
            state.generation += 1;
            state.record = record.clone();
            state.generation
        };
        self.rotations.send_replace(generation);
        self.changed.notify_one();
        let path = self.record_path.clone();
        match tokio::task::spawn_blocking(move || write_json(&path, &record, "token rotation")).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("cannot record token rotation: {error:#}"),
            Err(error) => eprintln!("cannot record token rotation: {error}"),
        }
        Ok(RotationReply {
            rotation: self.rotation(),
            token: Some(token),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders_precede_expiry_by_a_week_or_a_quarter_of_the_period() {
        let record = |period_days| RotationRecord {
            period_days,
            issued_at: 1_000,
            token_sha256: String::new(),
        };
        let monthly = TokenRotation::of(&record(Some(30)));
        assert_eq!(monthly.expires_at, Some(1_000 + 30 * DAY));
        assert_eq!(monthly.remind_at, Some(1_000 + 23 * DAY));
        let daily = TokenRotation::of(&record(Some(1)));
        assert_eq!(daily.remind_at, Some(1_000 + DAY - DAY / 4));
        let never = TokenRotation::of(&record(None));
        assert_eq!((never.expires_at, never.remind_at), (None, None));
        assert!(!never.due(u64::MAX));
        assert!(monthly.due(1_000 + 30 * DAY) && !monthly.due(1_000 + 30 * DAY - 1));
    }

    #[test]
    fn records_distinguish_never_from_the_default() {
        let parse = |text| serde_json::from_str::<RotationRecord>(text).unwrap().period_days;
        assert_eq!(
            parse(r#"{"issued_at":1,"token_sha256":""}"#),
            Some(DEFAULT_ROTATION_DAYS)
        );
        assert_eq!(parse(r#"{"period_days":null,"issued_at":1,"token_sha256":""}"#), None);
        assert_eq!(parse(r#"{"period_days":7,"issued_at":1,"token_sha256":""}"#), Some(7));
        assert!(!valid_period(Some(0)) && !valid_period(Some(MAX_ROTATION_DAYS + 1)));
        assert!(valid_period(None) && valid_period(Some(MAX_ROTATION_DAYS)));
    }
}
