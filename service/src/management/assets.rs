//! Public login assets and a navigation-only SPA fallback, separate from APIs.
use super::http::{HttpState, error_with_headers};
use anyhow::{Context as _, Result, ensure};
use axum::{
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

#[derive(Clone)]
pub(super) struct WebAssets {
    root: Arc<PathBuf>,
}
impl WebAssets {
    pub fn open(directory: &Path) -> Result<Self> {
        let root = directory.canonicalize().context("cannot open Web asset directory")?;
        let index = root
            .join("index.html")
            .canonicalize()
            .context("Web assets need a built index.html")?;
        ensure!(index.starts_with(&root) && index.is_file(), "invalid Web index file");
        Ok(Self { root: Arc::new(root) })
    }
}

pub(super) fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

pub(super) async fn serve(State(state): State<HttpState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let path = request.uri().path();
    if is_api(path) {
        return error_with_headers(&headers, StatusCode::NOT_FOUND, "not_found", "API route does not exist");
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return error_with_headers(
            &headers,
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "method is not allowed",
        );
    }
    let Some(assets) = state.assets else {
        return error_with_headers(
            &headers,
            StatusCode::NOT_FOUND,
            "not_found",
            "Web assets are not configured",
        );
    };
    let Ok(decoded) = percent_encoding::percent_decode_str(path).decode_utf8() else {
        return error_with_headers(&headers, StatusCode::BAD_REQUEST, "invalid_path", "invalid asset path");
    };
    if is_api(&decoded) {
        return error_with_headers(&headers, StatusCode::NOT_FOUND, "not_found", "API route does not exist");
    }
    let navigation = matches!(
        decoded.as_ref(),
        "/" | "/profiles"
            | "/config"
            | "/proxies"
            | "/unlock"
            | "/rules"
            | "/logs"
            | "/settings"
            | "/core"
            | "/service"
    );
    let accepts_html = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|part| part.trim().starts_with("text/html")));
    let relative = if decoded == "/" || (navigation && accepts_html) {
        "index.html"
    } else {
        decoded.trim_start_matches('/')
    };
    if !Path::new(relative)
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return error_with_headers(&headers, StatusCode::NOT_FOUND, "not_found", "asset does not exist");
    }
    let file = match tokio::fs::canonicalize(assets.root.join(relative)).await {
        Ok(file) if file.starts_with(assets.root.as_ref()) && file.is_file() => file,
        _ => return error_with_headers(&headers, StatusCode::NOT_FOUND, "not_found", "asset does not exist"),
    };
    match ServeFile::new(file).oneshot(request).await {
        Ok(response) => {
            let mut response = response.into_response();
            response.headers_mut().insert(header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'; form-action 'self'".parse().unwrap());
            response
                .headers_mut()
                .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
            response
        }
        Err(_) => error_with_headers(
            &headers,
            StatusCode::INTERNAL_SERVER_ERROR,
            "asset_error",
            "cannot serve asset",
        ),
    }
}
