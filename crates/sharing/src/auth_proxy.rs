//! Auth proxy — bearer-validated reverse proxy. One instance fronts Ollama,
//! a second fronts whisper.cpp.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use reqwest::Client;
use tracing::{debug, warn};

use crate::token_store::TokenStore;

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub listen_port: u16,
    pub backend_url: String,
    pub path_prefix: String,
    /// If `Some`, the proxy strips the client bearer and replaces it with
    /// this static `Authorization: Bearer …` header. Used to inject
    /// whisper.cpp's shared `--api-key` value.
    pub inject_api_key: Option<String>,
}

#[derive(Clone)]
struct AppState {
    config: ProxyConfig,
    client: Client,
    store: Arc<TokenStore>,
}

/// Bind the listener synchronously (so port conflicts surface immediately as
/// `Err`) then spawn the serving task. Returns the `JoinHandle` on success.
pub async fn spawn_auth_proxy(
    config: ProxyConfig,
    store: Arc<TokenStore>,
) -> crate::Result<tokio::task::JoinHandle<()>> {
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.listen_port))
        .await
        .map_err(|e| crate::SharingError::AuthProxy(format!(
            "bind 0.0.0.0:{}: {e}", config.listen_port
        )))?;
    let client = Client::builder()
        .pool_max_idle_per_host(8)
        .connect_timeout(std::time::Duration::from_secs(10))
        // No overall timeout — Ollama generations and whisper transcriptions
        // can be arbitrarily long; only the connection phase is bounded.
        .build()
        .map_err(|e| crate::SharingError::AuthProxy(e.to_string()))?;
    let state = AppState { config: config.clone(), client, store };
    let app = Router::new()
        .fallback(handler)
        .with_state(state);
    Ok(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!("auth_proxy serve exited: {e}");
        }
    }))
}

async fn handler(State(state): State<AppState>, req: Request) -> Response {
    match handle_inner(state, req).await {
        Ok(resp) => resp,
        Err(resp) => resp,
    }
}

fn unauthorized_with_reason(reason: &'static str) -> Response {
    let mut resp = (StatusCode::UNAUTHORIZED, "").into_response();
    // HeaderValue from a 'static &str is infallible for short ASCII tags.
    resp.headers_mut()
        .insert("x-auth-reason", HeaderValue::from_static(reason));
    resp
}

async fn handle_inner(state: AppState, req: Request) -> Result<Response, Response> {
    let token = match extract_bearer(req.headers()) {
        Some(t) => t,
        None => {
            warn!("proxy: 401 missing-bearer (no Authorization header)");
            return Err(unauthorized_with_reason("missing-bearer"));
        }
    };

    let row = match state.store.validate(&token) {
        Ok(Some(row)) => row,
        Ok(None) => {
            warn!("proxy: 401 unknown-token (no matching non-revoked row)");
            return Err(unauthorized_with_reason("unknown-token"));
        }
        Err(e) => {
            warn!(error = %e, "proxy: 500 token store validation error");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "").into_response());
        }
    };
    let client_id = row.id;
    debug!(client_id, "proxy: validated bearer");
    let _ = state.store.touch(client_id);

    let (parts, body) = req.into_parts();

    const MAX_BODY_BYTES: usize = 256 * 1024 * 1024;
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|_| (StatusCode::PAYLOAD_TOO_LARGE, "").into_response())?;

    let path_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let upstream_url = format!("{}{}", state.config.backend_url.trim_end_matches('/'), path_query);

    let mut req_builder = state
        .client
        .request(parts.method.clone(), &upstream_url)
        .body(body_bytes.clone());

    for (k, v) in parts.headers.iter() {
        if k == reqwest::header::HOST || k == reqwest::header::AUTHORIZATION {
            continue;
        }
        req_builder = req_builder.header(k.clone(), v.clone());
    }
    if let Some(api_key) = &state.config.inject_api_key {
        req_builder = req_builder.bearer_auth(api_key);
    }

    let upstream = req_builder.send().await.map_err(|e| {
        warn!("proxy upstream error: {e}");
        (StatusCode::BAD_GATEWAY, "").into_response()
    })?;

    let status = upstream.status();
    let mut resp_headers = HeaderMap::new();
    for (k, v) in upstream.headers() {
        if let Ok(hv) = HeaderValue::from_bytes(v.as_bytes()) {
            resp_headers.insert(k.clone(), hv);
        }
    }

    use futures_util::TryStreamExt;
    let stream = upstream
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    *resp.headers_mut() = resp_headers;
    Ok(resp)
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(reqwest::header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(|s| s.to_string())
}
