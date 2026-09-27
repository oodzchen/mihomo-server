//! Transport-independent credentials and same-origin request policy.

use anyhow::{Context as _, Result, ensure};
use std::{
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    net::SocketAddr,
    path::Path,
};
use url::Url;

/// Never implements Debug or Serialize: credentials must not enter state/logs.
pub struct Authentication {
    token: String,
    origin: String,
    authority: String,
}

impl Authentication {
    /// Must run while the service owns its data directory. The token is published
    /// using a non-overwriting hard link so interrupted writes cannot publish it.
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
            let mut entropy = [0u8; 32];
            getrandom::fill(&mut entropy).context("generate management token")?;
            let token: String = entropy.iter().map(|byte| format!("{byte:02x}")).collect();
            let temp = path.with_extension(format!("{}.tmp", &token[..16]));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut file = options.open(&temp).context("create private token file")?;
            let result = (|| -> Result<()> {
                writeln!(file, "{token}")?;
                file.sync_all()?;
                match fs::hard_link(&temp, path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
                #[cfg(unix)]
                fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
                Ok(())
            })();
            drop(file);
            let _ = fs::remove_file(&temp);
            result.context("publish management token")?;
        }
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
        Ok(Self {
            token,
            origin,
            authority,
        })
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// HTTP adapters must reject duplicate Host/Origin/Authorization headers
    /// before calling this method. No query token, cookies or CORS bypass.
    pub fn authorize(&self, host: &str, origin: Option<&str>, authorization: Option<&str>) -> Result<()> {
        self.authorize_origin(host, origin)?;
        self.authorize_token(
            authorization
                .and_then(|value| value.strip_prefix("Bearer "))
                .unwrap_or(""),
        )
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
        // Always compare all stored bytes; never return on the first mismatch.
        let mut difference = supplied.len() ^ self.token.len();
        for (index, byte) in self.token.bytes().enumerate() {
            difference |= usize::from(byte ^ supplied.as_bytes().get(index).copied().unwrap_or(0));
        }
        ensure!(difference == 0, "authentication required");
        Ok(())
    }
}
