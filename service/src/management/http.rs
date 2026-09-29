//! A small, explicit HTTP surface; mutations reuse the serialized manager.
use super::assets::WebAssets;
use super::websocket;
use super::{Management, ManagementCommand, RequestCredentials};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Request, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Includes the JSON envelope. Domain YAML limits still apply independently.
pub const MAX_REQUEST_BYTES: usize = 9 * 1024 * 1024;
pub const MAX_WEBSOCKETS: u32 = 32;

#[derive(Clone)]
pub struct HttpState {
    pub(super) management: Arc<Management>,
    accepting: Arc<AtomicBool>,
    pub(super) closing: tokio::sync::watch::Sender<bool>,
    pub(super) sessions: Arc<tokio::sync::Semaphore>,
    pub(super) assets: Option<WebAssets>,
}

impl HttpState {
    pub fn new(management: Management) -> Self {
        Self {
            management: Arc::new(management),
            accepting: Arc::new(AtomicBool::new(true)),
            closing: tokio::sync::watch::channel(false).0,
            sessions: Arc::new(tokio::sync::Semaphore::new(MAX_WEBSOCKETS as usize)),
            assets: None,
        }
    }

    pub fn with_web_assets(mut self, directory: &std::path::Path) -> anyhow::Result<Self> {
        self.assets = Some(WebAssets::open(directory)?);
        Ok(self)
    }

    /// Close command admission before requesting core shutdown.
    pub fn close(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        self.closing.send_replace(true);
    }

    /// Axum's HTTP graceful shutdown does not await upgraded socket tasks.
    pub async fn drain_websockets(&self) {
        if let Ok(permits) = Arc::clone(&self.sessions).acquire_many_owned(MAX_WEBSOCKETS).await {
            drop(permits);
        }
    }
}

pub fn router(state: HttpState) -> Router {
    Router::new()
        .route("/api/commands", post(command))
        .route("/api/backup", post(backup))
        .route("/api/backups", get(list_retained_backups).post(create_retained_backup))
        .route(
            "/api/backups/{id}",
            get(download_retained_backup).delete(delete_retained_backup),
        )
        .route("/api/backup/inspect", post(inspect_backup))
        .route("/api/backup/validate", post(validate_backup_restore))
        .route("/api/backup/restore", post(restore_backup))
        .route("/api/status", get(status))
        .route("/api/logs", get(logs))
        .route("/api/profiles", get(profiles))
        .route("/api/config", get(config))
        .route("/api/proxies", get(proxies))
        .route("/api/events", get(websocket::events))
        .route("/api/streams/{feed}", get(websocket::stream))
        .fallback(super::assets::serve)
        .method_not_allowed_fallback(|headers: HeaderMap| async move {
            error_with_headers(
                &headers,
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "method is not allowed",
            )
        })
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

fn credentials(headers: &HeaderMap) -> Result<RequestCredentials<'_>, &'static str> {
    fn single(headers: &HeaderMap, name: header::HeaderName) -> Result<Option<&str>, &'static str> {
        let mut values = headers.get_all(name).iter();
        let first = values.next();
        if values.next().is_some() {
            return Err("duplicate credential header");
        }
        first
            .map(|value| value.to_str().map_err(|_| "invalid credential header"))
            .transpose()
    }
    Ok(RequestCredentials {
        host: single(headers, header::HOST)?.unwrap_or(""),
        origin: single(headers, header::ORIGIN)?,
        authorization: single(headers, header::AUTHORIZATION)?,
    })
}

async fn authenticate(State(state): State<HttpState>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let language = language_from_headers(headers);
    let mut response = if !state.accepting.load(Ordering::SeqCst) {
        error_with_language(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutting_down",
            "service is shutting down",
            language.as_deref(),
        )
    } else {
        match credentials(headers) {
            Err(message) => {
                error_with_language(StatusCode::BAD_REQUEST, "invalid_headers", message, language.as_deref())
            }
            Ok(credentials)
                if if websocket::is_route(request.uri().path()) || !super::assets::is_api(request.uri().path()) {
                    state
                        .management
                        .authentication
                        .authorize_origin(credentials.host, credentials.origin)
                        .is_err()
                        || credentials
                            .authorization
                            .is_some_and(|_| state.management.authorize(credentials).is_err())
                } else {
                    state.management.authorize(credentials).is_err()
                } =>
            {
                let mut response = error_with_language(
                    StatusCode::UNAUTHORIZED,
                    "unauthorized",
                    "authentication required",
                    language.as_deref(),
                );
                response
                    .headers_mut()
                    .insert(header::WWW_AUTHENTICATE, "Bearer".parse().unwrap());
                response
            }
            Ok(_) if super::assets::is_api(request.uri().path()) && request.uri().query().is_some() => {
                error_with_language(
                    StatusCode::BAD_REQUEST,
                    "invalid_query",
                    "query parameters are not supported",
                    language.as_deref(),
                )
            }
            Ok(_) => next.run(request).await,
        }
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
}

pub fn language_from_headers(headers: &HeaderMap) -> Option<std::borrow::Cow<'static, str>> {
    headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .and_then(clash_verge_i18n::resolve_accept_language)
}

pub fn error(status: StatusCode, code: &str, message: &str) -> Response {
    error_with_language(status, code, message, None)
}

pub fn error_with_headers(headers: &HeaderMap, status: StatusCode, code: &str, message: &str) -> Response {
    let language = language_from_headers(headers);
    error_with_language(status, code, message, language.as_deref())
}

pub fn error_with_language(status: StatusCode, code: &str, message: &str, language: Option<&str>) -> Response {
    let localized = clash_verge_i18n::translate_service_error(code, message, language);
    (status, Json(json!({"error": {"code": code, "message": localized}}))).into_response()
}

async fn backup(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
    if !body.is_empty() {
        return error_with_headers(
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_backup_request",
            "backup request must be empty",
        );
    }
    let download = match state.management.manager.export_backup().await {
        Ok(download) => download,
        Err(_) => {
            return error_with_headers(
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                "backup_export_failed",
                "backup export unavailable; check source files and retry",
            );
        }
    };
    backup_response(download)
}

fn backup_response(download: crate::backup::BackupDownload) -> Response {
    let metadata = download.metadata;
    let stream = futures_util::stream::unfold(
        (Bytes::from(download.bytes), download.permit),
        |(mut bytes, permit)| async move {
            if bytes.is_empty() {
                return None;
            }
            let chunk = bytes.split_to(bytes.len().min(64 * 1024));
            Some((Ok::<_, std::convert::Infallible>(chunk), (bytes, permit)))
        },
    );
    let mut response = Body::from_stream(stream).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, "application/zip".parse().unwrap());
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", metadata.filename)
            .parse()
            .unwrap(),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        metadata.content_length.to_string().parse().unwrap(),
    );
    headers.insert("x-backup-sha256", metadata.sha256.parse().unwrap());
    response
}

async fn create_retained_backup(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
    retained_backup(state, &headers, crate::backup::RetainedOperation::Create, body).await
}
async fn list_retained_backups(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
    retained_backup(state, &headers, crate::backup::RetainedOperation::List, body).await
}
async fn download_retained_backup(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    retained_backup(state, &headers, crate::backup::RetainedOperation::Download(id), body).await
}
async fn delete_retained_backup(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    retained_backup(state, &headers, crate::backup::RetainedOperation::Delete(id), body).await
}
async fn retained_backup(
    state: HttpState,
    headers: &HeaderMap,
    operation: crate::backup::RetainedOperation,
    body: Bytes,
) -> Response {
    use crate::backup::{RetainedOperation, RetainedOutcome};
    if !body.is_empty() {
        return error_with_headers(
            headers,
            StatusCode::BAD_REQUEST,
            "invalid_backup_request",
            "local backup requests must be empty",
        );
    }
    if let RetainedOperation::Download(id) | RetainedOperation::Delete(id) = &operation
        && (id.len() != 24 || !id.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
    {
        return error_with_headers(
            headers,
            StatusCode::BAD_REQUEST,
            "invalid_backup_id",
            "invalid local backup ID",
        );
    }
    let (permit, shutdown) = match state.management.manager.admit_backup_upload() {
        Ok(admission) => admission,
        Err(_) if state.management.manager.is_shutting_down() => {
            return error_with_headers(
                headers,
                StatusCode::SERVICE_UNAVAILABLE,
                "shutting_down",
                "service is shutting down",
            );
        }
        Err(_) => {
            return error_with_headers(
                headers,
                StatusCode::CONFLICT,
                "backup_busy",
                "finish the current backup operation and retry",
            );
        }
    };
    let closing = state.closing.subscribe();
    match state
        .management
        .manager
        .retained_backup(operation, permit, closing.clone())
        .await
    {
        Ok(RetainedOutcome::Created(receipt)) => (StatusCode::CREATED, Json(receipt)).into_response(),
        Ok(RetainedOutcome::Listed(list)) => Json(list).into_response(),
        Ok(RetainedOutcome::Deleted(receipt)) => Json(receipt).into_response(),
        Ok(RetainedOutcome::Downloaded(download)) => backup_response(download),
        Err(cause) => {
            #[cfg(not(target_os = "linux"))]
            let _ = &cause;
            #[cfg(target_os = "linux")]
            {
                if cause.downcast_ref::<crate::backup::storage::StorageFull>().is_some() {
                    return error_with_headers(
                        headers,
                        StatusCode::INSUFFICIENT_STORAGE,
                        "backup_storage_full",
                        "local backup capacity reached; delete a retained backup and retry",
                    );
                }
                if cause.downcast_ref::<crate::backup::storage::Missing>().is_some() {
                    return error_with_headers(
                        headers,
                        StatusCode::NOT_FOUND,
                        "backup_not_found",
                        "local backup not found",
                    );
                }
            }
            if *shutdown.borrow() || *closing.borrow() {
                return error_with_headers(
                    headers,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "backup_interrupted",
                    "local backup operation interrupted; inspect retained backups before retrying",
                );
            }
            error_with_headers(
                headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                "backup_storage_failed",
                "local backup operation failed; inspect retained backups before retrying",
            )
        }
    }
}

async fn inspect_backup(State(state): State<HttpState>, request: Request) -> Response {
    backup_upload(state, request, BackupUpload::Inspect).await
}

async fn validate_backup_restore(State(state): State<HttpState>, request: Request) -> Response {
    backup_upload(state, request, BackupUpload::ValidateRestore).await
}

async fn restore_backup(State(state): State<HttpState>, request: Request) -> Response {
    use headless_core::backup::BackupRuntimePolicy;
    let values: Vec<_> = request.headers().get_all("x-backup-runtime").iter().collect();
    let policy = match values.as_slice() {
        [value] if *value == "archived" => BackupRuntimePolicy::Archived,
        [value] if *value == "regenerated" => BackupRuntimePolicy::Regenerated,
        _ => {
            return error_with_headers(
                request.headers(),
                StatusCode::BAD_REQUEST,
                "invalid_restore_policy",
                "send one X-Backup-Runtime header: archived or regenerated",
            );
        }
    };
    backup_upload(state, request, BackupUpload::Restore(policy)).await
}

enum BackupUpload {
    Inspect,
    ValidateRestore,
    Restore(headless_core::backup::BackupRuntimePolicy),
}

async fn backup_upload(state: HttpState, request: Request, operation: BackupUpload) -> Response {
    use headless_core::backup::MAX_ARCHIVE_BYTES;
    let headers = request.headers().clone();
    if headers.get_all(header::CONTENT_TYPE).iter().count() != 1
        || headers
            .get(header::CONTENT_TYPE)
            .is_none_or(|value| value != "application/zip")
        || headers.contains_key(header::CONTENT_ENCODING)
    {
        return error_with_headers(
            &headers,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_backup_media",
            "send an unencoded application/zip body",
        );
    }
    let lengths: Vec<_> = headers.get_all(header::CONTENT_LENGTH).iter().collect();
    let length = match lengths.as_slice() {
        [] => None,
        [value] => match value.to_str().ok().and_then(|value| value.parse::<u64>().ok()) {
            Some(length) => Some(length),
            None => {
                return error_with_headers(
                    &headers,
                    StatusCode::BAD_REQUEST,
                    "invalid_backup_length",
                    "invalid upload length",
                );
            }
        },
        _ => {
            return error_with_headers(
                &headers,
                StatusCode::BAD_REQUEST,
                "invalid_backup_length",
                "invalid upload length",
            );
        }
    };
    if length.is_some_and(|length| length > MAX_ARCHIVE_BYTES as u64) {
        return error_with_headers(
            &headers,
            StatusCode::PAYLOAD_TOO_LARGE,
            "backup_too_large",
            "backup archive exceeds 65 MiB",
        );
    }
    let (permit, mut shutdown) = match state.management.manager.admit_backup_upload() {
        Ok(admission) => admission,
        Err(_) => {
            return error_with_headers(
                &headers,
                StatusCode::CONFLICT,
                "backup_busy",
                "backup operation unavailable; finish the current operation and retry",
            );
        }
    };
    let mut closing = state.closing.subscribe();
    if *shutdown.borrow() || *closing.borrow() {
        return error_with_headers(
            &headers,
            StatusCode::SERVICE_UNAVAILABLE,
            "shutting_down",
            "service is shutting down",
        );
    }
    // Manual bounded collection intentionally gives only this binary route 65 MiB;
    // /api/commands continues to use its independent 9 MiB JSON envelope limit.
    let bytes = tokio::select! {
        _ = shutdown.changed() => return error_with_headers(&headers, StatusCode::SERVICE_UNAVAILABLE, "shutting_down", "service is shutting down"),
        _ = closing.changed() => return error_with_headers(&headers, StatusCode::SERVICE_UNAVAILABLE, "shutting_down", "service is shutting down"),
        result = tokio::time::timeout(std::time::Duration::from_secs(15), axum::body::to_bytes(request.into_body(), MAX_ARCHIVE_BYTES)) => match result {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(_)) => return error_with_headers(&headers, StatusCode::PAYLOAD_TOO_LARGE, "invalid_backup_body", "upload is oversized or could not be read"),
            Err(_) => return error_with_headers(&headers, StatusCode::REQUEST_TIMEOUT, "backup_upload_timeout", "backup upload exceeded 15 seconds"),
        }
    };
    if length.is_some_and(|length| length != bytes.len() as u64) {
        return error_with_headers(
            &headers,
            StatusCode::BAD_REQUEST,
            "invalid_backup_length",
            "upload length mismatch",
        );
    }
    if let BackupUpload::Restore(policy) = operation {
        // Do not discard a committed receipt merely because graceful close started.
        // The actor cancels before commit and finishes recovery after commit.
        return match state
            .management
            .manager
            .restore_backup(bytes, policy, permit, closing.clone())
            .await
        {
            Ok(receipt) => Json(receipt).into_response(),
            Err(cause) if cause.downcast_ref::<crate::backup::RestoreNeedsSettled>().is_some() => error_with_headers(
                &headers,
                StatusCode::CONFLICT,
                "restore_requires_settled_core",
                "wait for a settled running or stopped core before restoring a backup",
            ),
            Err(_) if *shutdown.borrow() || *closing.borrow() => error_with_headers(
                &headers,
                StatusCode::SERVICE_UNAVAILABLE,
                "restore_interrupted",
                "service is closing; inspect status before retrying restoration",
            ),
            Err(_) => error_with_headers(
                &headers,
                StatusCode::UNPROCESSABLE_ENTITY,
                "backup_restore_failed",
                "backup restoration failed; inspect service status before retrying",
            ),
        };
    }
    if matches!(operation, BackupUpload::ValidateRestore) {
        let operation = state
            .management
            .manager
            .validate_backup_restore(bytes, permit, closing.clone());
        return tokio::select! {
            _ = shutdown.changed() => error_with_headers(&headers, StatusCode::SERVICE_UNAVAILABLE, "shutting_down", "service is shutting down"),
            _ = closing.changed() => error_with_headers(&headers, StatusCode::SERVICE_UNAVAILABLE, "shutting_down", "service is shutting down"),
            result = operation => match result {
                Ok(report) => Json(report).into_response(),
                Err(_) => error_with_headers(&headers, StatusCode::UNPROCESSABLE_ENTITY, "backup_restore_validation_failed", "backup restore candidate failed validation; no configuration was applied"),
            }
        };
    }
    let worker_shutdown = shutdown.clone();
    let worker_closing = closing.clone();
    let worker = tokio::task::spawn_blocking(move || {
        // A dropped request cannot admit another large upload while this worker runs.
        let _permit = permit;
        crate::backup::inspect::run(&bytes, worker_shutdown, worker_closing)
    });
    tokio::select! {
        _ = shutdown.changed() => error_with_headers(&headers, StatusCode::SERVICE_UNAVAILABLE, "shutting_down", "service is shutting down"),
        _ = closing.changed() => error_with_headers(&headers, StatusCode::SERVICE_UNAVAILABLE, "shutting_down", "service is shutting down"),
        result = worker => match result {
            Ok(Ok(report)) => Json(report).into_response(),
            _ => error_with_headers(&headers, StatusCode::UNPROCESSABLE_ENTITY, "invalid_backup_archive", "backup archive failed format, integrity or configuration checks"),
        }
    }
}

async fn execute(state: HttpState, headers: HeaderMap, command: ManagementCommand) -> Response {
    let credentials = match credentials(&headers) {
        Ok(credentials) => credentials,
        Err(message) => return error_with_headers(&headers, StatusCode::BAD_REQUEST, "invalid_headers", message),
    };
    match state.management.execute(credentials, command).await {
        Ok(value) => Json(value).into_response(),
        // Validation/application errors preserve their actionable domain context.
        // A failed operation can already have attempted recovery; inspect status.
        Err(cause) => error_with_headers(
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            "operation_failed",
            &format!("{cause:#}"),
        ),
    }
}

async fn command(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Result<Json<ManagementCommand>, JsonRejection>,
) -> Response {
    match body {
        Ok(Json(command)) => execute(state, headers, command).await,
        Err(rejection) => error_with_headers(&headers, rejection.status(), "invalid_request", &rejection.body_text()),
    }
}

macro_rules! read_route {
    ($function:ident, $command:ident) => {
        async fn $function(State(state): State<HttpState>, headers: HeaderMap) -> Response {
            execute(state, headers, ManagementCommand::$command {}).await
        }
    };
}
read_route!(status, Status);
read_route!(logs, Logs);
read_route!(profiles, Profiles);
read_route!(config, Config);
read_route!(proxies, Proxies);
