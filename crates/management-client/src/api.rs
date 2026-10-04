//! The authenticated command API.
use crate::Endpoint;
use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};
use std::time::Duration;

/// A response the service rejected; `Display` is its (localized) message.
#[derive(Debug, Clone)]
pub struct CommandError {
    pub status: u16,
    pub code: Option<String>,
    pub message: String,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

/// Never implements Debug: it holds the management token.
pub struct Api {
    client: reqwest::Client,
    pub endpoint: Endpoint,
    token: String,
    timeout: Duration,
    language: String,
}

impl Api {
    /// Read the token from the endpoint's token file.
    pub fn connect(endpoint: Endpoint) -> Result<Self> {
        let token = std::fs::read_to_string(&endpoint.token_file)
            .with_context(|| format!("cannot read management token {}", endpoint.token_file.display()))?
            .trim()
            .to_owned();
        Self::with_token(endpoint, token)
    }

    pub fn with_token(endpoint: Endpoint, token: String) -> Result<Self> {
        anyhow::ensure!(!token.is_empty(), "management token file is empty");
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            client,
            endpoint,
            token,
            // Slow operations (delay tests, downloads, core upgrades) are
            // bounded by the service itself.
            timeout: Duration::from_secs(900),
            language: "en".into(),
        })
    }

    /// Bound every request; interactive clients should not wait 15 minutes.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// `Accept-Language` for service messages (`en`, `zh`, `zhtw`, ...).
    pub fn with_language(mut self, language: &str) -> Self {
        self.language = language.to_owned();
        self
    }

    /// Run one management command.
    pub async fn command(&self, name: &str, fields: Value) -> Result<Value> {
        let mut body = match fields {
            Value::Object(map) => map,
            Value::Null => Map::new(),
            _ => bail!("command fields must be an object"),
        };
        body.insert("command".into(), Value::String(name.into()));
        let response = self
            .client
            .post(format!("{}/api/commands", self.endpoint.base))
            .header(reqwest::header::HOST, &self.endpoint.host)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT_LANGUAGE, &self.language)
            .timeout(self.timeout)
            .json(&body)
            .send()
            .await
            .with_context(|| {
                format!(
                    "cannot reach the management API at {}; is the service running? (mihomo-server start)",
                    self.endpoint.base
                )
            })?;
        let status = response.status();
        let value: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(value);
        }
        Err(CommandError {
            status: status.as_u16(),
            code: value.pointer("/error/code").and_then(Value::as_str).map(str::to_owned),
            message: value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| status.to_string()),
        }
        .into())
    }
}
