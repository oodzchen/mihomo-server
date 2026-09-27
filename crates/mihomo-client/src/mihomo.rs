use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Duration,
};

use crate::WsMessage;
use arc_swap::{ArcSwap, Guard};
use clashmap::ClashMap;
use futures_util::{Stream, StreamExt};
use http::{
    HeaderMap, HeaderValue,
    header::{AUTHORIZATION, CONTENT_TYPE, HOST},
};
use log::log_enabled;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{Method, RequestBuilder};
use serde_json::json;
use tokio_tungstenite::{
    client_async, connect_async,
    tungstenite::{Message, client::IntoClientRequest, protocol::CloseFrame as ProtocolCloseFrame},
};

use crate::{
    DEFAULT_REQUEST_TIMEOUT, DOWNLOAD_FILE_TIMEOUT, Error, PROVIDER_UPDATE_TIMEOUT, Result,
    models::{
        BaseConfig, ConnectionManager, Connections, CoreUpdaterChannel, ErrorResponse, Groups, LogLevel, MihomoVersion,
        Protocol, Proxies, Proxy, ProxyDelay, ProxyProvider, ProxyProviders, RuleProviders, Rules, WsConnectionId,
    },
    ret_failed_resp,
    stream::WsStream,
};

type WsReaderKey = (usize, WsConnectionId);

static WS_READER_CANCELLATIONS: LazyLock<ClashMap<WsReaderKey, tokio::sync::oneshot::Sender<()>>> =
    LazyLock::new(ClashMap::new);

fn ws_reader_key(manager: &ConnectionManager, id: WsConnectionId) -> WsReaderKey {
    (Arc::as_ptr(&manager.0) as usize, id)
}

fn text_channel_body(text: &str) -> WsMessage {
    // Preserve JSON payloads; encode plain error text as a JSON string for consumers.
    if serde_json::from_str::<serde::de::IgnoredAny>(text).is_ok() {
        WsMessage::Json(text.to_string())
    } else {
        WsMessage::Json(serde_json::json!(text).to_string())
    }
}

fn websocket_message_to_channel_body(
    message: std::result::Result<Message, tokio_tungstenite::tungstenite::Error>,
) -> (Option<WsMessage>, bool) {
    match message {
        Ok(Message::Text(text)) => (Some(text_channel_body(&text)), false),
        Ok(Message::Close(_)) => (None, true),
        Ok(Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => (None, false),
        Err(err) => {
            log::warn!("websocket error: {err}");
            let error_message = Error::from(err).to_string();
            (Some(text_channel_body(&error_message)), true)
        }
    }
}

fn channel_body_to_text_bytes(body: WsMessage) -> Vec<u8> {
    match body {
        WsMessage::Raw(bytes) => bytes,
        WsMessage::Json(text) => text.into_bytes(),
    }
}

fn forward_channel_text<F>(on_message: F) -> impl Fn(WsMessage) -> bool + Send + 'static
where
    F: Fn(Vec<u8>) + Send + 'static,
{
    move |data| {
        on_message(channel_body_to_text_bytes(data));
        true
    }
}

#[derive(serde::Deserialize)]
struct ConnectionsCount {
    #[serde(default)]
    connections: Option<Vec<serde::de::IgnoredAny>>,
}

#[derive(serde::Deserialize)]
struct ConnectionCount {
    #[serde(default)]
    count: usize,
}

fn connections_count_body(count: usize) -> WsMessage {
    WsMessage::Json(serde_json::json!({ "count": count }).to_string())
}

// Error frames must be forwarded unchanged: consumers rely on the error text
// to trigger reconnection.
fn connections_count_forward_body(text: String) -> WsMessage {
    serde_json::from_str::<ConnectionsCount>(&text).map_or(WsMessage::Json(text), |count| {
        connections_count_body(count.connections.map_or(0, |connections| connections.len()))
    })
}

fn track_ws_reader(key: WsReaderKey, cancel_reader: tokio::sync::oneshot::Sender<()>) {
    WS_READER_CANCELLATIONS.insert(key, cancel_reader);
}

fn cancel_ws_reader(key: WsReaderKey) {
    if let Some((_, cancel_reader)) = WS_READER_CANCELLATIONS.remove(&key) {
        let _ = cancel_reader.send(());
    }
}

fn untrack_ws_reader(key: WsReaderKey) {
    WS_READER_CANCELLATIONS.remove(&key);
}

fn spawn_ws_reader<R, F>(
    manager: ConnectionManager,
    id: WsConnectionId,
    mut reader: R,
    mut cancel_reader_rx: tokio::sync::oneshot::Receiver<()>,
    reader_key: WsReaderKey,
    on_message: F,
) where
    R: Stream<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin + Send + 'static,
    F: Fn(WsMessage) -> bool + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = &mut cancel_reader_rx => {
                    log::debug!("connection [{id}] reader cancelled");
                    manager.0.remove(&id);
                    break;
                }
                message = reader.next() => {
                    match message {
                        Some(message) => {
                            let (response, should_close) = websocket_message_to_channel_body(message);
                            if should_close {
                                log::debug!("connection [{id}] closed");
                            }
                            let keep_reader = response.is_none_or(&on_message);
                            if should_close || !keep_reader {
                                if !keep_reader {
                                    log::debug!("message receiver dropped, closing websocket connection [{id}]");
                                }
                                manager.0.remove(&id);
                                untrack_ws_reader(reader_key);
                                break;
                            }
                        }
                        None => {
                            log::debug!("connection [{id}] stream ended");
                            manager.0.remove(&id);
                            untrack_ws_reader(reader_key);
                            break;
                        }
                    }
                }
            }
        }
    });
}

#[derive(Clone, Debug)]
pub struct MihomoContext {
    protocol: Protocol,
    external_host: Option<String>,
    external_port: Option<u16>,
    secret: Option<String>,
    socket_path: Option<String>,
    request_timeout: Duration,
    client: reqwest::Client,
}

impl MihomoContext {
    pub fn new(
        protocol: Protocol,
        external_host: Option<String>,
        external_port: Option<u16>,
        secret: Option<String>,
        socket_path: Option<String>,
        request_timeout: Duration,
        client: reqwest::Client,
    ) -> Self {
        Self {
            protocol,
            external_host,
            external_port,
            secret,
            socket_path,
            request_timeout,
            client,
        }
    }

    pub fn build_client(protocol: &Protocol, socket_path: Option<&str>) -> Result<reqwest::Client> {
        let mut builder = reqwest::ClientBuilder::new().no_proxy();
        match protocol {
            Protocol::Http => Ok(builder.build()?),
            Protocol::LocalSocket => {
                let Some(socket_path) = socket_path else {
                    return Err(Error::MissingPathParameter("socket_path".into()));
                };
                #[cfg(windows)]
                {
                    builder = builder.windows_named_pipe(socket_path);
                }
                #[cfg(unix)]
                {
                    builder = builder.unix_socket(socket_path);
                }
                Ok(builder.build()?)
            }
        }
    }

    #[inline]
    fn socket_path(&self) -> Result<&str> {
        self.socket_path.as_deref().ok_or_else(|| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing socket path".to_string(),
            ))
        })
    }

    fn generate_request_url(&self, suffix_url: &str) -> Result<String> {
        let suffix_url = suffix_url.trim_start_matches("/");
        match self.protocol {
            Protocol::Http => {
                if let Some(host) = self.external_host.as_ref() {
                    let port = self.external_port.unwrap_or(9090);
                    Ok(format!("http://{host}:{port}/{suffix_url}"))
                } else {
                    Err(Error::MissingPathParameter("external_host".into()))
                }
            }
            Protocol::LocalSocket => Ok(format!("http://localhost/{suffix_url}")),
        }
    }

    fn generate_req_headers(&self) -> Result<HeaderMap<HeaderValue>> {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_str("localhost")?);
        headers.insert(CONTENT_TYPE, HeaderValue::from_str("application/json")?);
        if matches!(self.protocol, Protocol::Http)
            && let Some(secret) = &self.secret
        {
            let auth_value = HeaderValue::from_str(&format!("Bearer {secret}"))?;
            headers.insert(AUTHORIZATION, auth_value);
        }
        Ok(headers)
    }

    pub fn build_request(&self, method: Method, suffix_url: &str) -> Result<RequestBuilder> {
        let url = self.generate_request_url(suffix_url)?;
        let headers = self.generate_req_headers()?;
        let request = match method {
            Method::POST => self.client.post(url),
            Method::GET => self.client.get(url),
            Method::PUT => self.client.put(url),
            Method::PATCH => self.client.patch(url),
            Method::DELETE => self.client.delete(url),
            _ => {
                let method_str = method.as_str().to_string();
                log::error!("method not supported: {method_str}");
                return Err(Error::MethodNotSupported(method_str));
            }
        };
        Ok(request.headers(headers).timeout(self.request_timeout))
    }

    fn get_websocket_url(&self, suffix_url: &str, queries: Option<Vec<(&str, &str)>>) -> Result<String> {
        let suffix_url = suffix_url.trim_start_matches("/");
        match self.protocol {
            Protocol::Http => {
                if let Some(host) = self.external_host.as_ref() {
                    let port = self.external_port.unwrap_or(9090);
                    let secret = self.secret.as_deref().unwrap_or_default();
                    let secret = utf8_percent_encode(secret, NON_ALPHANUMERIC);
                    let mut ws_url = format!("ws://{host}:{port}/{suffix_url}?token={secret}");
                    if let Some(queries) = queries {
                        queries.iter().for_each(|(k, v)| {
                            ws_url.push_str(format!("&{k}={v}").as_str());
                        });
                    }
                    Ok(ws_url)
                } else {
                    Err(Error::MissingPathParameter("external_host".into()))
                }
            }
            Protocol::LocalSocket => {
                let mut ws_url = format!("ws://localhost/{suffix_url}");
                if let Some(queries) = queries {
                    queries.iter().enumerate().for_each(|(index, (k, v))| {
                        if index == 0 {
                            ws_url.push_str(format!("?{k}={v}").as_str());
                        } else {
                            ws_url.push_str(format!("&{k}={v}").as_str());
                        }
                    });
                }
                Ok(ws_url)
            }
        }
    }
}

pub struct Mihomo {
    ctx: ArcSwap<MihomoContext>,
    pub connection_manager: ConnectionManager,
}

impl Mihomo {
    pub fn new(ctx: MihomoContext) -> Self {
        Self {
            ctx: ArcSwap::from_pointee(ctx),
            connection_manager: ConnectionManager::default(),
        }
    }

    pub fn load_ctx(&self) -> Guard<Arc<MihomoContext>> {
        self.ctx.load()
    }

    /// Atomically update the context snapshot via read-copy-update.
    ///
    /// Retries when a concurrent update lands in between, so no update is lost.
    fn update_ctx(&self, mut f: impl FnMut(&mut MihomoContext)) {
        self.ctx.rcu(|current| {
            let mut new_ctx = (**current).clone();
            f(&mut new_ctx);
            Arc::new(new_ctx)
        });
    }

    /// Atomically update the context snapshot via read-copy-update (fallible variant).
    ///
    /// The callback error is propagated and only successful mutations are published.
    fn try_update_ctx(&self, mut f: impl FnMut(&mut MihomoContext) -> Result<()>) -> Result<()> {
        loop {
            let current = self.ctx.load_full();
            let mut new_ctx = (*current).clone();
            f(&mut new_ctx)?;
            let prev = self.ctx.compare_and_swap(&current, Arc::new(new_ctx));
            if Arc::ptr_eq(&prev, &current) {
                return Ok(());
            }
        }
    }
}

impl Mihomo {
    pub fn update_protocol(&self, protocol: Protocol) -> Result<()> {
        self.try_update_ctx(|ctx| {
            ctx.protocol = protocol;
            ctx.client = MihomoContext::build_client(&ctx.protocol, ctx.socket_path.as_deref())?;
            Ok(())
        })
    }

    pub fn update_external_host(&self, host: Option<&str>) {
        self.update_ctx(|ctx| ctx.external_host = host.map(Into::into));
    }

    pub fn update_external_port(&self, port: Option<u16>) {
        self.update_ctx(|ctx| ctx.external_port = port);
    }

    pub fn update_secret(&self, secret: Option<&str>) {
        self.update_ctx(|ctx| ctx.secret = secret.map(Into::into));
    }

    pub fn update_socket_path<S: Into<String>>(&self, socket_path: S) -> Result<()> {
        let path = socket_path.into();
        self.try_update_ctx(|ctx| {
            ctx.socket_path = Some(path.clone());
            ctx.client = MihomoContext::build_client(&ctx.protocol, ctx.socket_path.as_deref())?;
            Ok(())
        })
    }

    /// 连接 WebSocket
    pub async fn connect<F>(
        &self,
        suffix_url: &str,
        queries: Option<Vec<(&str, &str)>>,
        on_message: F,
    ) -> Result<WsConnectionId>
    where
        F: Fn(WsMessage) -> bool + Send + 'static,
    {
        let ctx = self.load_ctx();
        let id = uuid::Uuid::new_v4();
        let url = ctx.get_websocket_url(suffix_url, queries)?;
        log::debug!("connecting to websocket, id: {id}");
        let manager = self.connection_manager.clone();

        match ctx.protocol {
            Protocol::Http => {
                log::debug!("starting connect to websocket by using http");
                let request = url.into_client_request()?;
                let (ws_stream, _) = connect_async(request).await?;
                let (writer, reader) = WsStream::from(ws_stream).split();
                let (cancel_reader, cancel_reader_rx) = tokio::sync::oneshot::channel();
                let reader_key = ws_reader_key(&manager, id);

                manager.0.insert(id, writer);
                track_ws_reader(reader_key, cancel_reader);

                spawn_ws_reader(manager, id, reader, cancel_reader_rx, reader_key, on_message);

                Ok(id)
            }
            Protocol::LocalSocket => {
                let socket_path = ctx.socket_path()?;
                log::trace!("starting connect to websocket by using local socket: {socket_path}");
                let stream = crate::stream::connect_to_socket(socket_path).await?;

                let request = url.into_client_request()?;
                let (ws_stream, _) = client_async(request, stream).await?;
                let (writer, reader) = WsStream::from(ws_stream).split();
                let (cancel_reader, cancel_reader_rx) = tokio::sync::oneshot::channel();
                let reader_key = ws_reader_key(&manager, id);

                manager.0.insert(id, writer);
                track_ws_reader(reader_key, cancel_reader);

                spawn_ws_reader(manager, id, reader, cancel_reader_rx, reader_key, on_message);
                Ok(id)
            }
        }
    }

    /// 取消 WebSocket 连接
    /// Synchronous, per-subscription cancellation for owned service guards.
    /// Unlike clear_all_ws_connections, this does not affect other consumers.
    pub fn cancel_ws_connection(&self, id: WsConnectionId) {
        cancel_ws_reader(ws_reader_key(&self.connection_manager, id));
        self.connection_manager.0.remove(&id);
    }

    /// 取消 WebSocket 连接
    pub async fn disconnect(&self, id: WsConnectionId, force_timeout: Option<u64>) -> Result<()> {
        log::debug!("disconnecting connection: {id}");
        // 先通过 websocket 发送关闭信息, 再发送取消读取信息的关闭信号
        {
            let Some(mut conn) = self.connection_manager.0.get_mut(&id) else {
                log::debug!("connection not found: {id}");
                return Err(Error::ConnectionNotFound(id));
            };

            let close_message = Message::Close(Some(ProtocolCloseFrame {
                code: 1000.into(),
                reason: "Disconnected by client".into(),
            }));

            log::debug!("send close message");
            let writer = conn.value_mut();
            let _ = writer.send(close_message).await;
        }

        if let Some(timeout) = force_timeout {
            log::trace!("force close after wait {timeout}ms");
            tokio::time::sleep(Duration::from_millis(timeout)).await;
            if self.connection_manager.0.contains_key(&id) {
                log::debug!("ws not received close message, force close");
                cancel_ws_reader(ws_reader_key(&self.connection_manager, id));
            }
        }
        log::debug!("close ws connection: {id} finished");

        Ok(())
    }

    pub fn start_ws_connections_watcher(&self) -> tokio::task::JoinHandle<()> {
        let connection_manager = self.connection_manager.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                if log_enabled!(log::Level::Trace) {
                    let ids: Vec<WsConnectionId> = connection_manager.0.iter().map(|entry| *entry.key()).collect();
                    log::trace!("mihomo ws ids: {:?}", ids);
                }
                interval.tick().await;
            }
        })
    }

    pub fn clear_all_ws_connections(&self) -> Result<()> {
        log::debug!("start to clear all websocket connections");
        let old_keys = self
            .connection_manager
            .0
            .iter()
            .map(|entry| *entry.key())
            .collect::<Vec<_>>();
        log::trace!("manage_ids: {:?}", old_keys);
        self.connection_manager.0.clear();
        log::trace!(
            "clear all done, manager_ids: {:?}",
            self.connection_manager
                .0
                .iter()
                .map(|entry| *entry.key())
                .collect::<Vec<_>>()
        );
        for id in old_keys {
            cancel_ws_reader(ws_reader_key(&self.connection_manager, id));
        }
        Ok(())
    }

    // ------------------------------------------------------
    // |                     Mihomo API                     |
    // ------------------------------------------------------
    /// WebSocket: Mihomo 流量数据
    pub async fn ws_traffic<F>(&self, on_message: F) -> Result<WsConnectionId>
    where
        F: Fn(Vec<u8>) + Send + 'static,
    {
        self.ws_traffic_checked(forward_channel_text(on_message)).await
    }

    pub async fn ws_traffic_checked<F>(&self, on_message: F) -> Result<WsConnectionId>
    where
        F: Fn(WsMessage) -> bool + Send + 'static,
    {
        self.connect("/traffic", None, on_message).await
    }

    /// WebSocket: Mihomo 内存使用数据
    pub async fn ws_memory<F>(&self, on_message: F) -> Result<WsConnectionId>
    where
        F: Fn(Vec<u8>) + Send + 'static,
    {
        self.ws_memory_checked(forward_channel_text(on_message)).await
    }

    pub async fn ws_memory_checked<F>(&self, on_message: F) -> Result<WsConnectionId>
    where
        F: Fn(WsMessage) -> bool + Send + 'static,
    {
        self.connect("/memory", None, on_message).await
    }

    /// WebSocket: Mihomo 连接信息数据
    pub async fn ws_connections<F>(&self, on_message: F) -> Result<WsConnectionId>
    where
        F: Fn(Vec<u8>) + Send + 'static,
    {
        self.ws_connections_checked(forward_channel_text(on_message)).await
    }

    pub async fn ws_connections_checked<F>(&self, on_message: F) -> Result<WsConnectionId>
    where
        F: Fn(WsMessage) -> bool + Send + 'static,
    {
        self.connect("/connections", None, on_message).await
    }

    /// WebSocket: Mihomo 活跃连接数量(不含具体连接数据)
    pub async fn ws_connections_count<F>(&self, on_message: F) -> Result<WsConnectionId>
    where
        F: Fn(usize) + Send + 'static,
    {
        self.ws_connections_count_checked(move |data| {
            if let Ok(reply) = serde_json::from_slice::<ConnectionCount>(&channel_body_to_text_bytes(data)) {
                on_message(reply.count);
            }
            true
        })
        .await
    }

    pub async fn ws_connections_count_checked<F>(&self, on_message: F) -> Result<WsConnectionId>
    where
        F: Fn(WsMessage) -> bool + Send + 'static,
    {
        self.connect("/connections", None, move |body| {
            match String::from_utf8(channel_body_to_text_bytes(body)) {
                Ok(text) => on_message(connections_count_forward_body(text)),
                Err(_) => true, // unreachable: text frames are valid UTF-8
            }
        })
        .await
    }

    /// WebSocket: Mihomo 日志数据
    pub async fn ws_logs<F>(&self, level: LogLevel, on_message: F) -> Result<WsConnectionId>
    where
        F: Fn(Vec<u8>) + Send + 'static,
    {
        self.ws_logs_checked(level, forward_channel_text(on_message)).await
    }

    pub async fn ws_logs_checked<F>(&self, level: LogLevel, on_message: F) -> Result<WsConnectionId>
    where
        F: Fn(WsMessage) -> bool + Send + 'static,
    {
        let level = level.to_string();
        let queries = Some(vec![("level", level.as_str())]);
        self.connect("/logs", queries, on_message).await
    }

    // clash api
    /// 获取 Mihomo 版本信息
    pub async fn get_version(&self) -> Result<MihomoVersion> {
        let response = self.load_ctx().build_request(Method::GET, "/version")?.send().await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("get mihomo version failed, {}", e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<MihomoVersion>().await?)
    }

    /// 清理 FakeIP 缓存
    pub async fn flush_fakeip(&self) -> Result<()> {
        let response = self
            .load_ctx()
            .build_request(Method::POST, "/cache/fakeip/flush")?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("flush fakeip cache failed, {}", e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 清理 DNS 缓存
    pub async fn flush_dns(&self) -> Result<()> {
        let response = self
            .load_ctx()
            .build_request(Method::POST, "/cache/dns/flush")?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response
                .json::<ErrorResponse>()
                .await
                .map_or_else(|e| format!("flush dns cache failed, {}", e), |err_res| err_res.message);
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 获取全部连接信息
    pub async fn get_connections(&self) -> Result<Connections> {
        let response = self
            .load_ctx()
            .build_request(Method::GET, "/connections")?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("get all connections failed, {}", e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<Connections>().await?)
    }

    /// 关闭全部连接
    pub async fn close_all_connections(&self) -> Result<()> {
        let response = self
            .load_ctx()
            .build_request(Method::DELETE, "/connections")?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("close all connections failed, {}", e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 关闭指定 ID 的连接
    pub async fn close_connection(&self, connection_id: &str) -> Result<()> {
        let response = self
            .load_ctx()
            .build_request(Method::DELETE, &format!("/connections/{connection_id}"))?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response
                .json::<ErrorResponse>()
                .await
                .map_or_else(|e| format!("close connection failed, {}", e), |err_res| err_res.message);
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 获取所有的代理组
    pub async fn get_groups(&self) -> Result<Groups> {
        let response = self.load_ctx().build_request(Method::GET, "/group")?.send().await?;
        if !response.status().is_success() {
            let err_msg = response
                .json::<ErrorResponse>()
                .await
                .map_or_else(|e| format!("get all groups failed, {}", e), |err_res| err_res.message);
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<Groups>().await?)
    }

    /// 获取指定名称的代理组
    pub async fn get_group_by_name(&self, group_name: &str) -> Result<Proxy> {
        let group_name_encode = utf8_percent_encode(group_name, NON_ALPHANUMERIC);
        let response = self
            .load_ctx()
            .build_request(Method::GET, &format!("/group/{group_name_encode}"))?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("get group[{}] failed, {}", group_name, e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<Proxy>().await?)
    }

    /// 对指定代理组进行延迟测试, 同时清理代理组已固定的节点
    pub async fn delay_group(&self, group_name: &str, test_url: &str, timeout: u32) -> Result<HashMap<String, u32>> {
        let group_name_encode = utf8_percent_encode(group_name, NON_ALPHANUMERIC);
        let suffix_url = format!("/group/{group_name_encode}/delay");
        let req_timeout = Duration::from_millis(timeout as u64) + DEFAULT_REQUEST_TIMEOUT;
        let response = self
            .load_ctx()
            .build_request(Method::GET, &suffix_url)?
            .query(&[("url", test_url), ("timeout", &timeout.to_string())])
            .timeout(req_timeout)
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("delay group[{}] failed, {}", group_name, e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<HashMap<String, u32>>().await?)
    }

    /// 获取代理提供者信息
    pub async fn get_proxy_providers(&self) -> Result<ProxyProviders> {
        let response = self
            .load_ctx()
            .build_request(Method::GET, "/providers/proxies")?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("get all proxy providers failed, {}", e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<ProxyProviders>().await?)
    }

    /// 获取指定代理提供者信息
    pub async fn get_proxy_provider_by_name(&self, provider_name: &str) -> Result<ProxyProvider> {
        let provider_name_encode = utf8_percent_encode(provider_name, NON_ALPHANUMERIC);
        let response = self
            .load_ctx()
            .build_request(Method::GET, &format!("/providers/proxies/{provider_name_encode}"))?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("get proxy provider[{}] failed, {}", provider_name, e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<ProxyProvider>().await?)
    }

    /// 更新指定代理提供者信息
    pub async fn update_proxy_provider(&self, provider_name: &str) -> Result<()> {
        let provider_name_encode = utf8_percent_encode(provider_name, NON_ALPHANUMERIC);
        let response = self
            .load_ctx()
            .build_request(Method::PUT, &format!("/providers/proxies/{provider_name_encode}"))?
            .timeout(PROVIDER_UPDATE_TIMEOUT)
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("update proxy provider[{}] failed, {}", provider_name, e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 对指定代理提供者进行健康检查
    pub async fn healthcheck_proxy_provider(&self, provider_name: &str) -> Result<()> {
        let provider_name_encode = utf8_percent_encode(provider_name, NON_ALPHANUMERIC);
        let suffix_url = format!("/providers/proxies/{provider_name_encode}/healthcheck");
        let response = self
            .load_ctx()
            .build_request(Method::GET, &suffix_url)?
            .timeout(Duration::from_secs(60))
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("healthcheck proxy provider[{}] failed, {}", provider_name, e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 对指定代理提供者下的指定节点（非代理组）进行健康检查, 并返回新的延迟信息
    pub async fn healthcheck_node_in_provider(
        &self,
        provider_name: &str,
        proxy_name: &str,
        test_url: &str,
        timeout: u32,
    ) -> Result<ProxyDelay> {
        let provider_name_encode = utf8_percent_encode(provider_name, NON_ALPHANUMERIC);
        let proxy_name_encode = utf8_percent_encode(proxy_name, NON_ALPHANUMERIC);
        let suffix_url = format!("/providers/proxies/{provider_name_encode}/{proxy_name_encode}/healthcheck");
        let req_timeout = Duration::from_millis(timeout as u64) + DEFAULT_REQUEST_TIMEOUT;
        let response = self
            .load_ctx()
            .build_request(Method::GET, &suffix_url)?
            .query(&[("url", test_url), ("timeout", &timeout.to_string())])
            .timeout(req_timeout)
            .send()
            .await?;
        if !response.status().is_success() {
            // maybe proxy delay is timeout response, try parse it.
            match response.json::<ErrorResponse>().await {
                Ok(err_res) => {
                    log::debug!("healthcheck node[{}] error: {}", proxy_name, err_res.message);
                    return Ok(ProxyDelay { delay: 0 });
                }
                Err(e) => {
                    ret_failed_resp!("healthcheck node[{}] failed, {}", proxy_name, e);
                }
            }
        }
        Ok(response.json::<ProxyDelay>().await?)
    }

    /// 获取所有代理信息
    pub async fn get_proxies(&self) -> Result<Proxies> {
        let response = self.load_ctx().build_request(Method::GET, "/proxies")?.send().await?;
        if !response.status().is_success() {
            let err_msg = response
                .json::<ErrorResponse>()
                .await
                .map_or_else(|e| format!("get all proxies failed, {}", e), |err_res| err_res.message);
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<Proxies>().await?)
    }

    /// 获取指定代理信息
    pub async fn get_proxy_by_name(&self, proxy_name: &str) -> Result<Proxy> {
        let proxy_name_encode = utf8_percent_encode(proxy_name, NON_ALPHANUMERIC);
        let response = self
            .load_ctx()
            .build_request(Method::GET, &format!("/proxies/{proxy_name_encode}"))?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("get proxy[{}] failed, {}", proxy_name, e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<Proxy>().await?)
    }

    /// 为指定代理选择节点
    ///
    /// 一般为指定代理组下使用指定的代理节点 【代理组/节点】
    pub async fn select_node_for_group(&self, group_name: &str, node: &str) -> Result<()> {
        let group_name_encode = utf8_percent_encode(group_name, NON_ALPHANUMERIC);
        let body = json!({ "name": node });
        let response = self
            .load_ctx()
            .build_request(Method::PUT, &format!("/proxies/{group_name_encode}"))?
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("select node[{}] for group[{}] failed, {}", node, group_name, e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 指定代理组下不再使用固定的代理节点
    ///
    /// 一般用于自动选择的代理组（例如：URLTest 类型的代理组）下的节点
    pub async fn unfixed_proxy(&self, group_name: &str) -> Result<()> {
        let group_name_encode = utf8_percent_encode(group_name, NON_ALPHANUMERIC);
        let response = self
            .load_ctx()
            .build_request(Method::DELETE, &format!("/proxies/{group_name_encode}"))?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("unfixed group[{}] failed, {}", group_name, e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 对指定代理进行延迟测试
    ///
    /// 一般用于代理节点的延迟测试，也可传代理组名称（只会测试代理组下选中的代理节点）
    pub async fn delay_proxy_by_name(&self, proxy_name: &str, test_url: &str, timeout: u32) -> Result<ProxyDelay> {
        let proxy_name_encode = utf8_percent_encode(proxy_name, NON_ALPHANUMERIC);
        let suffix_url = format!("/proxies/{proxy_name_encode}/delay");
        let req_timeout = Duration::from_millis(timeout as u64) + DEFAULT_REQUEST_TIMEOUT;
        let response = self
            .load_ctx()
            .build_request(Method::GET, &suffix_url)?
            .query(&[("timeout", &timeout.to_string()), ("url", &test_url.to_string())])
            .timeout(req_timeout)
            .send()
            .await?;
        if !response.status().is_success() {
            match response.json::<ErrorResponse>().await {
                Ok(err_res) => {
                    log::debug!(
                        "delay proxy[{}], mark it timeout, response error message: {}",
                        proxy_name,
                        err_res.message
                    );
                    return Ok(ProxyDelay { delay: 0 });
                }
                Err(e) => {
                    ret_failed_resp!("delay proxy[{}] failed, {}", proxy_name, e);
                }
            }
        }
        Ok(response.json::<ProxyDelay>().await?)
    }

    /// 获取所有规则信息
    pub async fn get_rules(&self) -> Result<Rules> {
        let response = self.load_ctx().build_request(Method::GET, "/rules")?.send().await?;
        if !response.status().is_success() {
            let err_msg = response
                .json::<ErrorResponse>()
                .await
                .map_or_else(|e| format!("get all rules failed, {}", e), |err_res| err_res.message);
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<Rules>().await?)
    }

    /// 获取所有规则提供者信息
    pub async fn get_rule_providers(&self) -> Result<RuleProviders> {
        let response = self
            .load_ctx()
            .build_request(Method::GET, "/providers/rules")?
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("get all rule providers failed, {}", e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<RuleProviders>().await?)
    }

    /// 更新规则提供者信息
    pub async fn update_rule_provider(&self, provider_name: &str) -> Result<()> {
        let provider_name_encode = utf8_percent_encode(provider_name, NON_ALPHANUMERIC);
        let response = self
            .load_ctx()
            .build_request(Method::PUT, &format!("/providers/rules/{provider_name_encode}"))?
            .timeout(PROVIDER_UPDATE_TIMEOUT)
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("update rule provider[{}] failed, {}", provider_name, e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 获取基础配置
    pub async fn get_base_config(&self) -> Result<BaseConfig> {
        let response = self.load_ctx().build_request(Method::GET, "/configs")?.send().await?;
        if !response.status().is_success() {
            let err_msg = response
                .json::<ErrorResponse>()
                .await
                .map_or_else(|e| format!("get base config failed, {}", e), |err_res| err_res.message);
            ret_failed_resp!("{}", err_msg);
        }
        Ok(response.json::<BaseConfig>().await?)
    }

    /// GET /configs with field presence retained for connection settings readback.
    pub async fn get_connection_config(&self) -> Result<crate::models::ConnectionConfig> {
        let response = self.load_ctx().build_request(Method::GET, "/configs")?.send().await?;
        if !response.status().is_success() {
            ret_failed_resp!("connection settings read failed");
        }
        Ok(response.json::<crate::models::ConnectionConfig>().await?)
    }

    /// 重新加载配置
    pub async fn reload_config(&self, force: bool, config_path: &str) -> Result<()> {
        let response = self
            .load_ctx()
            .build_request(Method::PUT, "/configs")?
            .timeout(Duration::from_secs(60))
            .query(&[("force", force)])
            .json(&json!({ "path": config_path }))
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("reload base config failed, {}", e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 更新基础配置
    pub async fn patch_base_config<D: serde::Serialize + Clone + Sync>(&self, data: &D) -> Result<()> {
        let response = self
            .load_ctx()
            .build_request(Method::PATCH, "/configs")?
            .json(&data)
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("patch base config failed, {}", e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 更新 Geo, 同 [`upgrade_geo`](crate::mihomo::Mihomo::upgrade_geo)
    pub async fn update_geo(&self) -> Result<()> {
        let response = self
            .load_ctx()
            .build_request(Method::POST, "/configs/geo")?
            .timeout(Duration::from_secs(60))
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("update geo database failed, {}", e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 重启核心
    pub async fn restart(&self) -> Result<()> {
        let response = self.load_ctx().build_request(Method::POST, "/restart")?.send().await?;
        if !response.status().is_success() {
            let err_msg = response
                .json::<ErrorResponse>()
                .await
                .map_or_else(|e| format!("restart core failed, {}", e), |err_res| err_res.message);
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 升级核心
    pub async fn upgrade_core(&self, channel: CoreUpdaterChannel, force: bool) -> Result<()> {
        let response = self
            .load_ctx()
            .build_request(Method::POST, "/upgrade")?
            .timeout(DOWNLOAD_FILE_TIMEOUT)
            .query(&[("channel", &channel.to_string()), ("force", &force.to_string())])
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("upgrade core failed, {}", e),
                |err_res| {
                    let msg = err_res.message;
                    if msg.to_lowercase().contains("already using latest version") {
                        "already using latest version".to_string()
                    } else {
                        msg
                    }
                },
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 更新 UI
    pub async fn upgrade_ui(&self) -> Result<()> {
        let response = self
            .load_ctx()
            .build_request(Method::POST, "/upgrade/ui")?
            .timeout(DOWNLOAD_FILE_TIMEOUT)
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response
                .json::<ErrorResponse>()
                .await
                .map_or_else(|e| format!("upgrade ui failed, {}", e), |err_res| err_res.message);
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }

    /// 更新 Geo
    pub async fn upgrade_geo(&self) -> Result<()> {
        let response = self
            .load_ctx()
            .build_request(Method::POST, "/upgrade/geo")?
            .timeout(DOWNLOAD_FILE_TIMEOUT)
            .send()
            .await?;
        if !response.status().is_success() {
            let err_msg = response.json::<ErrorResponse>().await.map_or_else(
                |e| format!("upgrade geo database failed, {}", e),
                |err_res| err_res.message,
            );
            ret_failed_resp!("{}", err_msg);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_text_channel_body_roundtrips_bytes_exactly() {
        let payload = r#"{"connections":[{"id":"a","metadata":{"host":"example.com"}}]}"#;
        assert_eq!(
            channel_body_to_text_bytes(text_channel_body(payload)),
            payload.as_bytes()
        );
    }

    #[test]
    fn json_text_becomes_json_channel_body() {
        let payload = r#"{"up":1,"down":2}"#;
        assert!(matches!(text_channel_body(payload),
            WsMessage::Json(text) if text == payload));
    }

    #[test]
    fn non_json_text_is_delivered_as_a_json_string() {
        let payload = "websocket error: connection closed";
        assert!(matches!(text_channel_body(payload),
            WsMessage::Json(text)
                if serde_json::from_str::<String>(&text).ok().as_deref() == Some(payload)));
    }

    #[test]
    fn websocket_error_message_is_json_encoded() {
        let err = tokio_tungstenite::tungstenite::Error::ConnectionClosed;
        let (body, should_close) = websocket_message_to_channel_body(Err(err));
        assert!(should_close);
        assert!(matches!(body,
            Some(WsMessage::Json(text))
                if serde_json::from_str::<String>(&text).ok().is_some_and(|message| !message.is_empty())));
    }

    #[test]
    fn every_channel_body_is_valid_json() {
        let frames = [
            r#"{"up":69632,"down":3810304}"#,
            "plain error text",
            "text with \"escapes\" and\nnewlines",
            "",
            "\0 control byte",
        ];
        for frame in frames {
            let body = websocket_message_to_channel_body(Ok(Message::Text(frame.into()))).0;
            assert!(
                matches!(&body, Some(WsMessage::Json(text))
                    if serde_json::from_str::<serde::de::IgnoredAny>(text).is_ok()),
                "text frames must produce a valid-JSON channel body, got {body:?}"
            );
        }
    }

    #[test]
    fn connections_count_forward_body_extracts_count_from_snapshots() {
        let snapshot =
            r#"{"downloadTotal":1,"uploadTotal":2,"connections":[{"id":"a"},{"id":"b"},{"id":"c"}]}"#.to_string();
        assert!(matches!(connections_count_forward_body(snapshot),
            WsMessage::Json(text) if text == r#"{"count":3}"#));
    }

    #[test]
    fn connections_count_forward_body_treats_null_connections_as_empty() {
        let snapshot = r#"{"downloadTotal":0,"uploadTotal":0,"connections":null}"#.to_string();
        assert!(matches!(connections_count_forward_body(snapshot),
            WsMessage::Json(text) if text == r#"{"count":0}"#));
    }

    #[test]
    fn connections_count_forward_body_passes_error_frames_through() {
        let error_frame = serde_json::json!("websocket error: connection closed").to_string();
        let forwarded = connections_count_forward_body(error_frame.clone());
        assert!(matches!(&forwarded,
            WsMessage::Json(text) if *text == error_frame));
    }

    #[test]
    fn connections_count_body_is_valid_json_with_count() {
        let body = connections_count_body(42);
        assert!(matches!(&body,
            WsMessage::Json(text) if text.as_str() == r#"{"count":42}"#));
    }
}
