//! Tauri-free extraction of the pinned upstream Mihomo client.

use std::time::Duration;

pub use error::{Error, Result};
pub use mihomo::{Mihomo, MihomoContext};
use models::Protocol;

mod error;
mod mihomo;
pub mod models;
mod stream;

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const DOWNLOAD_FILE_TIMEOUT: Duration = Duration::from_secs(90);
// Must outlast Mihomo's 20-second provider fetch to preserve its error response.
const PROVIDER_UPDATE_TIMEOUT: Duration = Duration::from_secs(30);

/// Callback payload preserving upstream JSON and plain-error framing.
/// Return false from a checked subscription callback when its receiver is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    Json(String),
    Raw(Vec<u8>),
}

impl WsMessage {
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Json(text) => text.into_bytes(),
            Self::Raw(bytes) => bytes,
        }
    }
}

#[derive(Debug)]
pub struct Builder {
    protocol: Protocol,
    external_host: Option<String>,
    external_port: Option<u16>,
    secret: Option<String>,
    socket_path: Option<String>,
    request_timeout: Option<Duration>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            protocol: Protocol::Http,
            external_host: Some(String::from("127.0.0.1")),
            external_port: Some(9090),
            secret: None,
            socket_path: None,
            request_timeout: Some(DEFAULT_REQUEST_TIMEOUT),
        }
    }
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn protocol(mut self, protocol: Protocol) -> Self {
        self.protocol = protocol;
        self
    }

    pub fn external_host<S: Into<String>>(mut self, external_host: S) -> Self {
        self.external_host = Some(external_host.into());
        self
    }

    pub fn external_port(mut self, external_port: u16) -> Self {
        self.external_port = Some(external_port);
        self
    }

    pub fn secret<S: Into<String>>(mut self, secret: S) -> Self {
        self.secret = Some(secret.into());
        self
    }

    pub fn socket_path<S: Into<String>>(mut self, socket_path: S) -> Self {
        self.socket_path = Some(socket_path.into());
        self
    }

    /// Overrides the ordinary request timeout; download operations retain their own timeouts.
    pub fn request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = Some(request_timeout);
        self
    }

    pub fn build(self) -> Result<Mihomo> {
        let client = MihomoContext::build_client(&self.protocol, self.socket_path.as_deref())?;
        let ctx = MihomoContext::new(
            self.protocol,
            self.external_host,
            self.external_port,
            self.secret,
            self.socket_path,
            self.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT),
            client,
        );
        Ok(Mihomo::new(ctx))
    }
}
