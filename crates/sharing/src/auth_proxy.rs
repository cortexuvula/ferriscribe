//! Auth proxy -- bearer-validated reverse proxy.
//!
//! One instance fronts Ollama (port 11435 -> 127.0.0.1:11434), a second
//! fronts whisper.cpp (port 8081 -> 127.0.0.1:8080), and an optional third
//! fronts LM Studio (port 1235 -> 127.0.0.1:1234).
//!
//! ## Request flow
//!
//! 1. Extract `Authorization: Bearer <token>` from the inbound request.
//! 2. Hash the token and look it up in the [`TokenStore`]. Reject with
//!    401 + `x-auth-reason` header on missing/invalid/revoked tokens.
//! 3. Strip the client's `Authorization` header. If `inject_api_key` is
//!    configured, replace it with a static backend key.
//! 4. Forward the full request body (up to 256 MiB) to the backend.
//! 5. Stream the response back to the client.
//!
//! ## `x-auth-reason` contract
//!
//! Downstream crates (`stt-providers`) inspect the `x-auth-reason` response
//! header on 401 to distinguish failure modes:
//!
//! | Value | Meaning |
//! |---|---|
//! | `missing-bearer` | No `Authorization` header at all |
//! | `unknown-token` | Token hash not found or already revoked |

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use reqwest::Client;
use tracing::{debug, info, warn};

use crate::token_store::TokenStore;

/// Cheap-clone byte buffer type returned by `axum::body::to_bytes`. We need to
/// be able to clone the buffered request body so the proxy can retry the
/// upstream send after a transient connection failure.
type BodyBytes = axum::body::Bytes;

/// Configuration for a single auth proxy instance.
///
/// Each proxy listens on one public port and forwards validated requests to
/// one loopback-only backend (Ollama, whisper-server, or LM Studio).
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Public listener port (e.g. 11435 for Ollama, 8081 for whisper).
    pub listen_port: u16,
    /// Backend URL to forward requests to (e.g. `http://127.0.0.1:11434`).
    pub backend_url: String,
    /// Path prefix prepended to the forwarded request path. Currently always `"/"`.
    pub path_prefix: String,
    /// If `Some`, the proxy strips the client bearer and replaces it with
    /// this static `Authorization: Bearer ...` header. Used to inject
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
/// `Err`) then spawn the serving task.
///
/// Returns the `JoinHandle` of the background serve task. The proxy runs
/// until the handle is aborted (typically by [`SharingService::stop`](crate::SharingService::stop)).
///
/// # Errors
///
/// Returns [`SharingError::AuthProxy`](crate::SharingError::AuthProxy) if the TCP bind fails or the reqwest
/// client cannot be constructed.
pub async fn spawn_auth_proxy(
    config: ProxyConfig,
    store: Arc<TokenStore>,
) -> crate::Result<tokio::task::JoinHandle<()>> {
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.listen_port))
        .await
        .map_err(|e| {
            crate::SharingError::AuthProxy(format!("bind 0.0.0.0:{}: {e}", config.listen_port))
        })?;
    let client = Client::builder()
        .pool_max_idle_per_host(8)
        .connect_timeout(std::time::Duration::from_secs(10))
        // Bounded overall timeout: a hung upstream (Ollama crash, whisper
        // hang) would otherwise tie up the proxy connection indefinitely.
        // 30 min covers the longest realistic encounter (a 60-min audio at
        // ~3x realtime transcription = ~20 min, plus generation margin).
        .timeout(std::time::Duration::from_secs(30 * 60))
        .build()
        .map_err(|e| crate::SharingError::AuthProxy(e.to_string()))?;
    let state = AppState {
        config: config.clone(),
        client,
        store,
    };
    let app = Router::new().fallback(handler).with_state(state);
    Ok(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!("auth_proxy serve exited: {e}");
        }
    }))
}

/// Bind a TCP listener on `0.0.0.0:port` without spawning the proxy. The
/// readiness gate uses this to claim the port early so a conflict surfaces
/// before the upstream probe, then hands the listener to
/// [`spawn_auth_proxy_on_listener`] once the upstream is ready.
pub async fn bind_proxy_listener(port: u16) -> crate::Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| crate::SharingError::AuthProxy(format!("bind 0.0.0.0:{port}: {e}")))
}

/// Like [`spawn_auth_proxy`] but accepts a pre-bound listener. Used by the
/// readiness gate, which binds the proxy only after the upstream becomes
/// ready (long after `start()`).
pub async fn spawn_auth_proxy_on_listener(
    listener: tokio::net::TcpListener,
    config: ProxyConfig,
    store: Arc<TokenStore>,
) -> crate::Result<tokio::task::JoinHandle<()>> {
    let client = Client::builder()
        .pool_max_idle_per_host(8)
        .connect_timeout(std::time::Duration::from_secs(10))
        // Bounded overall timeout: a hung upstream (Ollama crash, whisper
        // hang) would otherwise tie up the proxy connection indefinitely.
        // 30 min covers the longest realistic encounter (a 60-min audio at
        // ~3x realtime transcription = ~20 min, plus generation margin).
        .timeout(std::time::Duration::from_secs(30 * 60))
        .build()
        .map_err(|e| crate::SharingError::AuthProxy(e.to_string()))?;
    let state = AppState {
        config: config.clone(),
        client,
        store,
    };
    let app = Router::new().fallback(handler).with_state(state);
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
    let upstream_url = format!(
        "{}{}",
        state.config.backend_url.trim_end_matches('/'),
        path_query
    );

    let build_upstream = |body_bytes: BodyBytes| {
        let mut req_builder = state
            .client
            .request(parts.method.clone(), &upstream_url)
            .body(body_bytes);
        for (k, v) in parts.headers.iter() {
            if k == reqwest::header::HOST || k == reqwest::header::AUTHORIZATION {
                continue;
            }
            req_builder = req_builder.header(k.clone(), v.clone());
        }
        if let Some(api_key) = &state.config.inject_api_key {
            req_builder = req_builder.bearer_auth(api_key);
        }
        req_builder
    };

    // First attempt.
    let upstream = match build_upstream(body_bytes.clone()).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "proxy upstream error on first attempt; will retry once");
            // The backend (whisper-server / ollama / lmstudio) is likely down.
            // Give the supervisor a moment to restart it (its first backoff is
            // 1s) and try once more before surfacing the failure. This closes
            // the "bound proxy keeps 502-ing forever after a transient crash"
            // gap left by the gate-then-self-heal design.
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            match build_upstream(body_bytes.clone()).send().await {
                Ok(r) => {
                    info!("proxy upstream recovered on retry");
                    r
                }
                Err(e2) => {
                    warn!(error = %e2, "proxy upstream still down after retry; returning 502");
                    return Err(gateway_down_response());
                }
            }
        }
    };

    let status = upstream.status();
    let mut resp_headers = HeaderMap::new();
    for (k, v) in upstream.headers() {
        if let Ok(hv) = HeaderValue::from_bytes(v.as_bytes()) {
            resp_headers.insert(k.clone(), hv);
        }
    }

    use futures_util::TryStreamExt;
    let stream = upstream.bytes_stream().map_err(std::io::Error::other);
    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    *resp.headers_mut() = resp_headers;
    Ok(resp)
}

/// 502 response with a plain-text body that tells the operator exactly what to
/// do. Previously the proxy returned an empty 502 on a dead upstream, which
/// the client surfaced as a generic "502 Bad Gateway" — leaving the user with
/// no clue that the office-side backend (not their machine) was the problem.
fn gateway_down_response() -> Response {
    let body = "Backend service is down. Restart Sharing on the office machine \
        (Settings \u{2192} Sharing \u{2192} Stop, then Start) so it relaunches \
        the local whisper-server.";
    let mut resp = (StatusCode::BAD_GATEWAY, body).into_response();
    resp.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    // Tag the failure mode so the client side can craft a tailored message if
    // it ever wants to (contract with crates/stt-providers/src/client.rs).
    resp.headers_mut().insert(
        "x-proxy-reason",
        HeaderValue::from_static("backend-unreachable"),
    );
    resp
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(reqwest::header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_store::TokenStore;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use wiremock::matchers::{header, method as http_method, path as http_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Open a TokenStore in a tempdir and return both (store, tempdir-guard).
    fn fresh_store() -> (Arc<TokenStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.db");
        let store = Arc::new(TokenStore::open(&path, &[7u8; 32]).expect("open token store"));
        (store, dir)
    }

    /// Bind on 127.0.0.1:0, capture the kernel-assigned port, drop the
    /// listener so spawn_auth_proxy can re-bind. Rare race with other
    /// processes for the same port; acceptable for tests.
    async fn ephemeral_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);
        port
    }

    /// Start the proxy and return (port, JoinHandle). The TempDir from
    /// fresh_store() must outlive the test — keep it bound.
    async fn spawn_test_proxy(
        store: Arc<TokenStore>,
        backend_url: String,
        inject_api_key: Option<String>,
    ) -> (u16, tokio::task::JoinHandle<()>) {
        let port = ephemeral_port().await;
        let cfg = ProxyConfig {
            listen_port: port,
            backend_url,
            path_prefix: "/".into(),
            inject_api_key,
        };
        let handle = spawn_auth_proxy(cfg, store).await.expect("spawn");
        // Give the spawned task a moment to begin serving. axum::serve
        // is ready immediately after spawn but on slow CI a 50ms warmup
        // avoids flakes.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (port, handle)
    }

    #[tokio::test]
    async fn proxy_401_missing_bearer() {
        let (store, _dir) = fresh_store();
        let upstream = MockServer::start().await;
        let (port, handle) = spawn_test_proxy(store, upstream.uri(), None).await;

        let resp = reqwest::get(&format!("http://127.0.0.1:{port}/anything"))
            .await
            .expect("send");
        assert_eq!(resp.status(), 401);
        assert_eq!(
            resp.headers()
                .get("x-auth-reason")
                .and_then(|v| v.to_str().ok()),
            Some("missing-bearer"),
        );
        handle.abort();
    }

    #[tokio::test]
    async fn proxy_401_unknown_token() {
        let (store, _dir) = fresh_store();
        let _issued = store.issue("good-client").expect("issue");
        let upstream = MockServer::start().await;
        let (port, handle) = spawn_test_proxy(store, upstream.uri(), None).await;

        let resp = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth("not-a-real-token")
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 401);
        assert_eq!(
            resp.headers()
                .get("x-auth-reason")
                .and_then(|v| v.to_str().ok()),
            Some("unknown-token"),
        );
        handle.abort();
    }

    #[tokio::test]
    async fn proxy_401_revoked_token() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("doomed").expect("issue");
        store.revoke(issued.id).expect("revoke");
        let upstream = MockServer::start().await;
        let (port, handle) = spawn_test_proxy(store, upstream.uri(), None).await;

        let resp = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth(&issued.token)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 401);
        assert_eq!(
            resp.headers()
                .get("x-auth-reason")
                .and_then(|v| v.to_str().ok()),
            Some("unknown-token"),
            "revoked tokens classify as unknown-token because validate() filters revoked rows"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn proxy_200_proxies_to_backend_on_valid_bearer() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("clinic-laptop").expect("issue");
        let store_for_check = Arc::clone(&store);

        let upstream = MockServer::start().await;
        Mock::given(http_method("GET"))
            .and(http_path("/anything"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok-body"))
            .mount(&upstream)
            .await;

        let (port, handle) = spawn_test_proxy(store, upstream.uri(), None).await;

        let resp = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth(&issued.token)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 200);
        let body = resp.text().await.expect("text");
        assert_eq!(body, "ok-body");

        // Validate that touch() fired (last_seen_at is now Some).
        let rows = store_for_check.list().expect("list");
        let row = rows.iter().find(|r| r.id == issued.id).expect("issued row");
        assert!(
            row.last_seen_at.is_some(),
            "successful proxy call should have fired touch()"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn proxy_strips_client_bearer_and_injects_api_key() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("a").expect("issue");

        let upstream = MockServer::start().await;
        // Only match requests where the upstream sees the INJECTED key —
        // wiremock returns 404 (default) for any other auth header. So a
        // 200 response proves the swap happened.
        Mock::given(http_method("GET"))
            .and(http_path("/anything"))
            .and(header("authorization", "Bearer server-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_string("authed"))
            .mount(&upstream)
            .await;

        let (port, handle) =
            spawn_test_proxy(store, upstream.uri(), Some("server-secret".to_string())).await;

        let resp = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth(&issued.token) // client uses its own token
            .send()
            .await
            .expect("send");
        assert_eq!(
            resp.status(),
            200,
            "200 proves wiremock saw 'Bearer server-secret', not the client token"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn proxy_502_when_backend_unreachable() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("a").expect("issue");
        // Port 1 is privileged on Unix and almost certainly not listening.
        // Even if it were, the connect_timeout would fire after 10s; tests
        // run in <1s for the typical "connection refused" case.
        let (port, handle) = spawn_test_proxy(store, "http://127.0.0.1:1".to_string(), None).await;

        let resp = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth(&issued.token)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 502);
        handle.abort();
    }

    /// A persistently-down backend must surface the new actionable 502 body
    /// and the `x-proxy-reason` tag so the client can tailor its message.
    #[tokio::test]
    async fn proxy_502_carries_clear_body_and_reason_header() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("a").expect("issue");
        // Port 1 refuses connections → both attempts fail → gateway_down_response().
        let (port, handle) = spawn_test_proxy(store, "http://127.0.0.1:1".to_string(), None).await;

        let resp = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth(&issued.token)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 502);
        assert_eq!(
            resp.headers()
                .get("x-proxy-reason")
                .and_then(|v| v.to_str().ok()),
            Some("backend-unreachable"),
        );
        let body = resp.text().await.expect("text");
        assert!(
            body.contains("Restart Sharing on the office machine"),
            "502 body should tell the operator what to do; got: {body}",
        );
        handle.abort();
    }

    /// When the backend refuses the first attempt but comes up within the
    /// 3s backoff window, the proxy's retry must succeed and stream a 200 back
    /// to the client. This is the core self-healing behavior.
    #[tokio::test]
    async fn proxy_retries_and_succeeds_when_backend_comes_up() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("a").expect("issue");

        // Start a wiremock that answers 200 to the upstream path.
        let upstream = MockServer::start().await;
        Mock::given(http_method("GET"))
            .and(http_path("/anything"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok-body"))
            .mount(&upstream)
            .await;

        // Point the proxy at a closed port first, then flip it to the live
        // upstream before the retry fires. We can't reconfigure a running
        // proxy's backend_url, so instead we use a tiny TCP relay we control:
        // it refuses (port closed) for the first connection, then accepts and
        // forwards to the wiremock thereafter.
        let upstream_uri = upstream.uri();
        let relay_port = ephemeral_port().await;
        let gate = std::sync::Arc::new(tokio::sync::Notify::new());
        let gate_clone = gate.clone();
        let relay_handle = tokio::spawn(async move {
            // First bind is deliberately delayed until notified, so the first
            // incoming connection from the proxy gets refused (nothing
            // listening yet).
            gate_clone.notified().await;
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", relay_port))
                .await
                .expect("relay bind");
            // Accept and blindly forward all subsequent connections to the
            // wiremock upstream. Simplest correct forwarder: act as a dumb
            // byte pipe (we don't parse HTTP here).
            loop {
                let (mut client, _) = match listener.accept().await {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let upstream_uri = upstream_uri.clone();
                tokio::spawn(async move {
                    let (host, port) = upstream_uri
                        .trim_start_matches("http://")
                        .split_once(':')
                        .map(|(h, p)| (h.to_string(), p.parse::<u16>().unwrap()))
                        .unwrap();
                    let mut up = tokio::net::TcpStream::connect((host, port))
                        .await
                        .expect("connect upstream");
                    tokio::io::copy_bidirectional(&mut client, &mut up)
                        .await
                        .ok();
                });
            }
        });

        let (port, handle) =
            spawn_test_proxy(store, format!("http://127.0.0.1:{relay_port}"), None).await;

        // Fire the request from a task so we can open the relay gate while
        // it's in flight (during the 3s backoff).
        let url = format!("http://127.0.0.1:{port}/anything");
        let token = issued.token.clone();
        let req_task = tokio::spawn(async move {
            reqwest::Client::new()
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .expect("send")
        });

        // Let the proxy's first attempt fail (relay not listening yet), then
        // open the gate so the bind completes before the retry after the 3s
        // backoff. The retry will then connect through the relay to wiremock.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        gate.notify_waiters();

        let resp = req_task.await.expect("task");
        assert_eq!(
            resp.status(),
            200,
            "retry should succeed after backend comes up",
        );
        let body = resp.text().await.expect("text");
        assert_eq!(body, "ok-body");

        handle.abort();
        relay_handle.abort();
    }
}
