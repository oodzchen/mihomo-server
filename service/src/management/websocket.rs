//! Browser events and owned, bounded forwarding through the extracted client.
use super::http::{HttpState, error_with_headers};
use crate::core_manager::CorePhase;
use anyhow::{Result, bail, ensure};
use axum::{
    extract::{
        Path, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection},
    },
    http::StatusCode,
    response::{IntoResponse as _, Response},
};
use mihomo_client::{
    Mihomo, WsMessage,
    models::{LogLevel, WsConnectionId},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    time::{Instant, sleep_until, timeout},
};

const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
const MAX_SAMPLE_BYTES: usize = 1024 * 1024;
const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;
const SAMPLE_CAPACITY: usize = 8;

#[derive(Clone, Copy)]
enum Feed {
    Traffic,
    Memory,
    Connections,
    ConnectionsCount,
    Logs,
}
impl Feed {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "traffic" => Self::Traffic,
            "memory" => Self::Memory,
            "connections" => Self::Connections,
            "connections_count" => Self::ConnectionsCount,
            "logs" => Self::Logs,
            _ => return None,
        })
    }
}

pub(super) fn is_route(path: &str) -> bool {
    path == "/api/events" || path.starts_with("/api/streams/")
}

pub(super) async fn events(
    State(state): State<HttpState>,
    headers: axum::http::HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    match upgrade {
        Ok(upgrade) => upgrade_session(state, &headers, upgrade, None),
        Err(rejection) => error_with_headers(&headers, rejection.status(), "invalid_upgrade", &rejection.body_text()),
    }
}

pub(super) async fn stream(
    State(state): State<HttpState>,
    Path(feed): Path<String>,
    headers: axum::http::HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Some(feed) = Feed::parse(&feed) else {
        return error_with_headers(&headers, StatusCode::NOT_FOUND, "not_found", "stream does not exist");
    };
    match upgrade {
        Ok(upgrade) => upgrade_session(state, &headers, upgrade, Some(feed)),
        Err(rejection) => error_with_headers(&headers, rejection.status(), "invalid_upgrade", &rejection.body_text()),
    }
}

fn upgrade_session(state: HttpState, headers: &axum::http::HeaderMap, upgrade: WebSocketUpgrade, feed: Option<Feed>) -> Response {
    let Ok(permit) = Arc::clone(&state.sessions).try_acquire_owned() else {
        return error_with_headers(
            headers,
            StatusCode::SERVICE_UNAVAILABLE,
            "session_limit",
            "WebSocket session limit reached",
        );
    };
    if *state.closing.borrow() {
        return error_with_headers(
            headers,
            StatusCode::SERVICE_UNAVAILABLE,
            "shutting_down",
            "service is shutting down",
        );
    }
    upgrade
        .max_message_size(4096)
        .max_frame_size(4096)
        .read_buffer_size(4096)
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_EVENT_BYTES + 65536)
        .on_upgrade(move |mut socket| async move {
            let _permit = permit;
            let mut closing = state.closing.subscribe();
            let authenticated = authenticate(&mut socket, &state, &mut closing).await;
            let code = if authenticated {
                if run(&mut socket, &state, feed, &mut closing).await.is_ok() {
                    1000
                } else {
                    1011
                }
            } else {
                1008
            };
            let _ = timeout(
                WRITE_TIMEOUT,
                socket.send(Message::Close(Some(CloseFrame {
                    code,
                    reason: "session ended".into(),
                }))),
            )
            .await;
        })
        .into_response()
}

// Credentials deliberately have no Debug implementation and never enter events.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum AuthenticationFrame {
    Authenticate { token: String },
}

async fn authenticate(socket: &mut WebSocket, state: &HttpState, closing: &mut watch::Receiver<bool>) -> bool {
    let accepted = tokio::select! {
        biased;
        _ = closed(closing) => return false,
        message = timeout(AUTH_TIMEOUT, socket.recv()) => {
            match message {
                Ok(Some(Ok(Message::Text(text)))) => {
                    match serde_json::from_str::<AuthenticationFrame>(&text) {
                        Ok(AuthenticationFrame::Authenticate { token }) => state.management.authentication.authorize_token(&token).is_ok(),
                        Err(_) => false,
                    }
                }
                _ => false,
            }
        }
    };
    if !accepted {
        let _ = emit(
            socket,
            json!({"type":"error","code":"unauthorized","message":"authentication required"}),
        )
        .await;
    }
    accepted
}

async fn closed(receiver: &mut watch::Receiver<bool>) {
    if !*receiver.borrow_and_update() {
        let _ = receiver.changed().await;
    }
}

async fn emit(socket: &mut WebSocket, value: Value) -> Result<()> {
    let text = serde_json::to_string(&value)?;
    ensure!(text.len() <= MAX_EVENT_BYTES, "event exceeds size limit");
    timeout(WRITE_TIMEOUT, socket.send(Message::Text(text.into()))).await??;
    Ok(())
}

struct Subscription {
    client: Arc<Mihomo>,
    id: WsConnectionId,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.client.cancel_ws_connection(self.id);
    }
}

fn sample_sender(
    sender: mpsc::Sender<Vec<u8>>,
    overflow: Arc<AtomicBool>,
) -> impl Fn(WsMessage) -> bool + Send + 'static {
    move |message| {
        let bytes = message.into_bytes();
        if bytes.len() > MAX_SAMPLE_BYTES {
            overflow.store(true, Ordering::SeqCst);
            return false;
        }
        match sender.try_send(bytes) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                overflow.store(true, Ordering::SeqCst);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

async fn subscribe(
    client: &Mihomo,
    feed: Feed,
    sender: mpsc::Sender<Vec<u8>>,
    overflow: Arc<AtomicBool>,
) -> mihomo_client::Result<WsConnectionId> {
    let callback = sample_sender(sender, overflow);
    match feed {
        Feed::Traffic => client.ws_traffic_checked(callback).await,
        Feed::Memory => client.ws_memory_checked(callback).await,
        Feed::Connections => client.ws_connections_checked(callback).await,
        Feed::ConnectionsCount => client.ws_connections_count_checked(callback).await,
        Feed::Logs => client.ws_logs_checked(LogLevel::DEBUG, callback).await,
    }
}

async fn incoming(message: Option<std::result::Result<Message, axum::Error>>, last_pong: &mut Instant) -> Result<bool> {
    match message {
        Some(Ok(Message::Close(_))) | None => Ok(false),
        Some(Ok(Message::Pong(_))) => {
            *last_pong = Instant::now();
            Ok(true)
        }
        // Axum automatically replies to pings; the next write flushes the pong.
        Some(Ok(Message::Ping(_))) => Ok(true),
        Some(Err(error)) => Err(error.into()),
        _ => bail!("only WebSocket control frames are accepted after authentication"),
    }
}

async fn run(
    socket: &mut WebSocket,
    state: &HttpState,
    feed: Option<Feed>,
    closing: &mut watch::Receiver<bool>,
) -> Result<()> {
    let manager = &state.management.manager;
    let client = manager.client();
    let mut status = manager.subscribe_status();
    let mut profiles = manager.subscribe_profiles();
    let mut logs = manager.subscribe_logs();
    let initial_status = status.borrow_and_update().clone();
    let initial_profiles = profiles.borrow_and_update().clone();
    emit(socket, json!({"type":"ready"})).await?;
    if feed.is_none() {
        emit(
            socket,
            json!({"type":"snapshot", "status":initial_status, "profiles":initial_profiles, "logs":manager.logs()}),
        )
        .await?;
    } else {
        emit(socket, json!({"type":"core_state", "data":initial_status})).await?;
    }
    let mut generation = initial_status.generation;
    let mut subscription = None::<Subscription>;
    let mut samples = None::<mpsc::Receiver<Vec<u8>>>;
    let mut overflow = Arc::new(AtomicBool::new(false));
    let mut retry_at = Instant::now();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    heartbeat.tick().await;
    let mut last_pong = Instant::now();
    loop {
        if *closing.borrow() || manager.status().phase == CorePhase::Shutdown {
            break;
        }
        tokio::select! {
            biased;
            _ = closed(closing) => break,
            message = socket.recv() => if !incoming(message, &mut last_pong).await? { break; },
            changed = status.changed() => {
                if changed.is_err() { break; }
                let current = status.borrow_and_update().clone();
                if current.phase != CorePhase::Running || current.generation != generation {
                    subscription = None;
                    samples = None;
                    retry_at = Instant::now();
                }
                generation = current.generation;
                emit(socket, json!({"type":if feed.is_some() {"core_state"} else {"status"},"data":current})).await?;
            }
            changed = profiles.changed(), if feed.is_none() => {
                if changed.is_err() { break; }
                let current = profiles.borrow_and_update().clone();
                emit(socket, json!({"type":"profiles","data":current})).await?;
            }
            log = logs.recv(), if feed.is_none() => {
                match log {
                    Ok(log) => emit(socket, json!({"type":"log","data":log})).await?,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        logs = manager.subscribe_logs();
                        emit(socket, json!({"type":"logs_reset","data":manager.logs()})).await?;
                    }
                    Err(_) => break,
                }
            }
            _ = heartbeat.tick() => {
                ensure!(last_pong.elapsed() < Duration::from_secs(45), "WebSocket heartbeat expired");
                timeout(WRITE_TIMEOUT, socket.send(Message::Ping(Vec::new().into()))).await??;
            }
            sample = async { samples.as_mut().expect("sample receiver enabled").recv().await }, if samples.is_some() => {
                if overflow.load(Ordering::SeqCst) {
                    emit(socket, json!({"type":"error","code":"stream_overflow","message":"stream consumer is too slow or sample exceeds limit"})).await?;
                    bail!("bounded stream overflow");
                }
                match sample {
                    Some(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                        Ok(value) if value.is_object() => emit(socket, json!({"type":"data","data":value})).await?,
                        _ => {
                            subscription = None; samples = None; retry_at = Instant::now() + RETRY_INTERVAL;
                            emit(socket, json!({"type":"stream_error","message":"core stream reported an error; reconnecting"})).await?;
                        }
                    },
                    None => {
                        subscription = None; samples = None; retry_at = Instant::now() + RETRY_INTERVAL;
                        emit(socket, json!({"type":"stream_error","message":"core stream closed; reconnecting"})).await?;
                    }
                }
            }
            _ = sleep_until(retry_at), if feed.is_some() && subscription.is_none() && manager.status().phase == CorePhase::Running => {
                let (sender, receiver) = mpsc::channel(SAMPLE_CAPACITY);
                overflow = Arc::new(AtomicBool::new(false));
                let mut connecting_state = status.clone();
                let result = tokio::select! {
                    biased;
                    _ = closed(closing) => break,
                    message = socket.recv() => { if !incoming(message, &mut last_pong).await? { break; } continue; },
                    _ = connecting_state.changed() => continue,
                    result = timeout(Duration::from_secs(5), subscribe(&client, feed.expect("feed enabled"), sender, Arc::clone(&overflow))) => result,
                };
                match result {
                    Ok(Ok(id)) => {
                        subscription = Some(Subscription { client: Arc::clone(&client), id });
                        samples = Some(receiver);
                    }
                    _ => {
                        retry_at = Instant::now() + RETRY_INTERVAL;
                        emit(socket, json!({"type":"stream_error","message":"core stream unavailable; reconnecting"})).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_bounds_samples_and_cancels_when_consumer_is_gone() {
        let (sender, receiver) = mpsc::channel(SAMPLE_CAPACITY);
        let overflow = Arc::new(AtomicBool::new(false));
        let callback = sample_sender(sender, Arc::clone(&overflow));
        for _ in 0..SAMPLE_CAPACITY {
            assert!(callback(WsMessage::Json("{}".into())));
        }
        assert!(!callback(WsMessage::Json("{}".into())));
        assert!(overflow.load(Ordering::SeqCst));
        drop(receiver);
        let (sender, receiver) = mpsc::channel(1);
        let overflow = Arc::new(AtomicBool::new(false));
        let callback = sample_sender(sender, Arc::clone(&overflow));
        assert!(!callback(WsMessage::Raw(vec![0; MAX_SAMPLE_BYTES + 1])));
        assert!(overflow.load(Ordering::SeqCst));
        overflow.store(false, Ordering::SeqCst);
        drop(receiver);
        assert!(!callback(WsMessage::Json("{}".into())));
        assert!(!overflow.load(Ordering::SeqCst));
    }
}
