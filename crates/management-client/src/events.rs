//! Authenticated subscriptions to the service's WebSocket feeds. The token is
//! sent in the first frame, never in the URL.
use crate::Endpoint;
use anyhow::{Context as _, Result, bail, ensure};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest as _},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The service pings every 15 s; silence beyond this means a dead link.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Feed {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl Feed {
    /// Subscribe to `/api/streams/<name>` (or `/api/events` for `None`).
    pub async fn connect(endpoint: &Endpoint, token: &str, name: Option<&str>) -> Result<Self> {
        let Some(authority) = endpoint.base.strip_prefix("http://") else {
            bail!(
                "event feeds need a plain http:// management address, not {}",
                endpoint.base
            );
        };
        let path = name.map_or_else(|| "/api/events".to_owned(), |name| format!("/api/streams/{name}"));
        let mut request = format!("ws://{authority}{path}").into_client_request()?;
        let headers = request.headers_mut();
        headers.insert("Host", endpoint.host.parse()?);
        headers.insert("Origin", endpoint.management_url.parse()?);
        let (mut socket, _) = timeout(CONNECT_TIMEOUT, connect_async(request))
            .await
            .context("the event feed did not answer in time")?
            .with_context(|| format!("cannot open the event feed {path}"))?;
        socket
            .send(Message::Text(
                json!({"type": "authenticate", "token": token}).to_string().into(),
            ))
            .await?;
        let mut feed = Self { socket };
        let ready = feed
            .next()
            .await?
            .context("the event feed closed before it was ready")?;
        ensure!(ready["type"] == "ready", "event feed refused: {}", ready["message"]);
        Ok(feed)
    }

    /// The next event; `None` once the service closes the feed.
    pub async fn next(&mut self) -> Result<Option<Value>> {
        loop {
            let message = timeout(IDLE_TIMEOUT, self.socket.next())
                .await
                .context("the event feed went silent")?;
            match message {
                None | Some(Ok(Message::Close(_))) => return Ok(None),
                Some(Ok(Message::Text(text))) => return Ok(Some(serde_json::from_str(&text)?)),
                // Pings are answered on the next read or write; flush the pong now.
                Some(Ok(Message::Ping(_))) => self.socket.flush().await?,
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error.into()),
            }
        }
    }
}
