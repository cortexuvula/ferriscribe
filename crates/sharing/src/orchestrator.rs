//! Orchestrator -- the public face of the sharing layer.
//!
//! Owns the auth proxy (Ollama route), auth proxy (whisper route), mDNS
//! advertiser, pairing service, and whisper-cpp supervisor. `start()` boots
//! all enabled subsystems with synchronous port binding (so conflicts surface
//! immediately); `stop()` tears them down cleanly.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::SharingError;
use crate::auth_proxy::{ProxyConfig, spawn_auth_proxy};
use crate::mdns::{MdnsAdvertiser, ServerPorts};
use crate::pairing::PairingState;
use crate::token_store::TokenStore;
use crate::upstream::UpstreamKind;
use crate::whisper_supervisor::WhisperSupervisor;

/// Top-level configuration for the sharing subsystem.
///
/// Created by `src-tauri` from persisted user settings and passed to
/// [`SharingService::new`]. Sensitive fields (`token_store_key`,
/// `whisper_internal_api_key`) are redacted in [`Debug`] output.
#[derive(Clone)]
pub struct SharingConfig {
    /// Whether sharing is enabled. When `false`, `start()` is a no-op.
    pub enabled: bool,
    /// Human-readable server name broadcast via mDNS and embedded in the QR URL.
    pub friendly_name: String,
    /// Public listener port for the Ollama auth proxy (default 11435).
    pub ollama_proxy_port: u16,
    /// Public listener port for the whisper auth proxy (default 8081).
    pub whisper_proxy_port: u16,
    /// Listener port for the pairing HTTP service (default 11436).
    pub pairing_port: u16,
    /// Loopback-only port where whisper-server listens (default 8080). Not exposed to the LAN.
    pub whisper_internal_port: u16,
    /// Local LM Studio listener port (typically 1234). `Some` when LM Studio
    /// is detected at config time; `None` skips wiring an LM Studio proxy.
    pub lmstudio_internal_port: Option<u16>,
    /// Public auth-proxy listener for LM Studio (typically 1235). Advertised
    /// to clients via mDNS / QR. Always paired with `lmstudio_internal_port`.
    pub lmstudio_proxy_port: Option<u16>,
    /// Vocabulary CRUD HTTP API port (typically 11437). The HTTP server
    /// itself lives in the Tauri layer because it needs the SQLCipher pool;
    /// the sharing crate just records the port so it gets advertised via
    /// mDNS / QR alongside the rest.
    pub vocab_port: u16,
    /// Filesystem path for the SQLCipher-encrypted token store database.
    pub token_store_path: PathBuf,
    /// 32-byte SQLCipher encryption key. Derived by `medical-security` from the OS keychain.
    pub token_store_key: [u8; 32],
    /// Directory for whisper-server binary downloads.
    pub binary_dir: PathBuf,
    /// Path to the whisper.cpp model file (`.bin`).
    pub whisper_model_path: PathBuf,
    /// Static API key injected into whisper-server requests as `Authorization: Bearer <key>`.
    pub whisper_internal_api_key: String,
    /// Crate version string, broadcast in mDNS TXT records and `/info` responses.
    pub version: String,
}

impl std::fmt::Debug for SharingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharingConfig")
            .field("enabled", &self.enabled)
            .field("friendly_name", &self.friendly_name)
            .field("ollama_proxy_port", &self.ollama_proxy_port)
            .field("whisper_proxy_port", &self.whisper_proxy_port)
            .field("pairing_port", &self.pairing_port)
            .field("whisper_internal_port", &self.whisper_internal_port)
            .field("lmstudio_internal_port", &self.lmstudio_internal_port)
            .field("lmstudio_proxy_port", &self.lmstudio_proxy_port)
            .field("vocab_port", &self.vocab_port)
            .field("token_store_path", &self.token_store_path)
            .field("token_store_key", &"<redacted: 32 bytes>")
            .field("binary_dir", &self.binary_dir)
            .field("whisper_model_path", &self.whisper_model_path)
            .field("whisper_internal_api_key", &"<redacted>")
            .field("version", &self.version)
            .finish()
    }
}

impl Default for SharingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            friendly_name: "FerriScribe Server".to_string(),
            ollama_proxy_port: 11435,
            whisper_proxy_port: 8081,
            pairing_port: 11436,
            whisper_internal_port: 8080,
            lmstudio_internal_port: None,
            lmstudio_proxy_port: None,
            vocab_port: 11437,
            token_store_path: PathBuf::new(),
            token_store_key: [0u8; 32],
            binary_dir: PathBuf::new(),
            whisper_model_path: PathBuf::new(),
            whisper_internal_api_key: String::new(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Snapshot of the sharing subsystem's health.
///
/// Returned by [`SharingService::status`] and serialized to the Svelte
/// frontend via Tauri commands. All `*_ok` booleans are `true` only while the
/// orchestrator is running.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SharingStatus {
    /// `true` when `start()` has been called and `stop()` has not.
    pub enabled: bool,
    /// `true` when the Ollama auth proxy is running.
    pub ollama_ok: bool,
    /// `true` when the whisper auth proxy and supervisor are running.
    pub whisper_ok: bool,
    /// True only when LM Studio's local server was detected at Start sharing
    /// time and the auth proxy was wired up. False when LM Studio wasn't
    /// running at config time -- clients won't see LM Studio models in that
    /// case until the user Stops + Starts sharing with LM Studio running.
    pub lmstudio_ok: bool,
    /// `true` when the mDNS advertiser is active.
    pub mdns_ok: bool,
    /// `true` when the pairing HTTP service is running.
    pub pairing_ok: bool,
    /// Number of non-revoked paired clients in the token store.
    pub paired_clients: u32,
}

/// Per-upstream readiness snapshot, read by `status()` and updated by the
/// ReadinessWatcher and the start gate.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProbeState {
    /// Upstream is part of this server's config (e.g. LM Studio is always a
    /// candidate in office mode). When false the upstream is ignored entirely.
    pub configured: bool,
    /// Auth proxy is currently listening for this upstream.
    pub proxy_bound: bool,
    /// Most recent probe succeeded.
    pub last_probe_ok: bool,
    /// When the most recent probe ran.
    pub last_probe_at: Option<Instant>,
}

/// Readiness cache keyed by upstream kind.
pub type ReadinessState = HashMap<UpstreamKind, ProbeState>;

/// Top-level orchestrator for all sharing subsystems.
///
/// Owns the token store, pairing state, whisper supervisor, mDNS advertiser,
/// and auth proxy join handles. `start()` boots everything in dependency
/// order (bind listeners first so port conflicts surface immediately);
/// `stop()` tears down in reverse order.
///
/// Designed to be held behind an `Arc` and shared across Tauri command
/// handlers. All methods are `&self` (interior mutability via `Mutex`).
pub struct SharingService {
    config: SharingConfig,
    store: Arc<TokenStore>,
    pairing: Arc<PairingState>,
    whisper: Arc<WhisperSupervisor>,
    mdns: Mutex<Option<MdnsAdvertiser>>,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    running: Mutex<bool>,
    /// Per-upstream readiness cache. Drives honest `status()`.
    readiness: tokio::sync::RwLock<ReadinessState>,
    /// Live snapshot served by `GET :11436/info`. Behind `Arc` so it can be
    /// cloned into the spawned pairing task (Tailscale discovery path).
    info: Arc<tokio::sync::RwLock<InfoSnapshot>>,
    /// Watch channel: a new `ReadinessState` is sent whenever the ready set
    /// changes. The Tauri layer forwards changes to a frontend event.
    readiness_tx: tokio::sync::watch::Sender<ReadinessState>,
}

impl SharingService {
    /// Create a new sharing service from the given config.
    ///
    /// Opens (or creates) the token store database on disk. Does not start
    /// any listeners -- call [`start`](Self::start) for that.
    pub fn new(config: SharingConfig) -> Result<Self, SharingError> {
        let store = Arc::new(
            TokenStore::open(&config.token_store_path, &config.token_store_key)
                .map_err(|e| SharingError::TokenStore(e.to_string()))?,
        );
        let pairing = Arc::new(PairingState::new(store.clone()));
        let whisper = Arc::new(WhisperSupervisor::new(
            config.binary_dir.clone(),
            config.whisper_model_path.clone(),
            config.whisper_internal_port,
        ));
        let mut readiness: ReadinessState = HashMap::new();
        readiness.insert(UpstreamKind::Ollama, ProbeState {
            configured: true, proxy_bound: false, last_probe_ok: false, last_probe_at: None,
        });
        readiness.insert(UpstreamKind::Whisper, ProbeState {
            configured: true, proxy_bound: false, last_probe_ok: false, last_probe_at: None,
        });
        readiness.insert(UpstreamKind::LmStudio, ProbeState {
            configured: config.lmstudio_proxy_port.is_some(),
            proxy_bound: false, last_probe_ok: false, last_probe_at: None,
        });
        let info = InfoSnapshot {
            host: config.friendly_name.clone(),
            version: config.version.clone(),
            ports: ServerPorts {
                ollama: Some(config.ollama_proxy_port),
                whisper: Some(config.whisper_proxy_port),
                // LM Studio advertised only once ready (see watcher).
                lmstudio: None,
                pairing: Some(config.pairing_port),
                vocab: Some(config.vocab_port),
            },
        };
        let (readiness_tx, _rx) = tokio::sync::watch::channel(readiness.clone());
        Ok(Self {
            config,
            store,
            pairing,
            whisper,
            mdns: Mutex::new(None),
            handles: Mutex::new(Vec::new()),
            running: Mutex::new(false),
            readiness: tokio::sync::RwLock::new(readiness),
            info: Arc::new(tokio::sync::RwLock::new(info)),
            readiness_tx,
        })
    }

    /// Clone the [`PairingState`] handle (for issuing/validating codes from
    /// Tauri commands).
    pub fn pairing_state(&self) -> Arc<PairingState> { self.pairing.clone() }

    /// Clone the [`TokenStore`] handle (for listing/revoking clients from
    /// Tauri commands).
    pub fn token_store(&self) -> Arc<TokenStore> { self.store.clone() }

    /// Borrow the active config.
    pub fn config(&self) -> &SharingConfig { &self.config }

    /// Subscribe to readiness changes. A new `ReadinessState` is sent whenever
    /// the ready set of upstreams changes (an upstream binds or goes down).
    /// The Tauri layer forwards these to a frontend event.
    pub fn readiness_changes(&self) -> tokio::sync::watch::Receiver<ReadinessState> {
        self.readiness_tx.subscribe()
    }

    /// Start all sharing subsystems.
    ///
    /// Binds listeners synchronously so port conflicts return as `Err`
    /// immediately. On partial failure, already-spawned tasks are aborted
    /// before the error is returned.
    ///
    /// Idempotent: calling `start()` when already running is a no-op.
    pub async fn start(&self) -> Result<(), SharingError> {
        let mut running = self.running.lock().await;
        if *running { return Ok(()); }

        // Ollama auth proxy — bind first so port conflicts surface as errors.
        let h1 = spawn_auth_proxy(
            ProxyConfig {
                listen_port: self.config.ollama_proxy_port,
                backend_url: "http://127.0.0.1:11434".to_string(),
                path_prefix: "/".to_string(),
                inject_api_key: None,
            },
            self.store.clone(),
        ).await?;

        // Push h1 immediately so that if anything below fails, stop() can abort it.
        self.handles.lock().await.push(h1);

        // Whisper auth proxy — bind first.
        let h2 = match spawn_auth_proxy(
            ProxyConfig {
                listen_port: self.config.whisper_proxy_port,
                backend_url: format!("http://127.0.0.1:{}", self.config.whisper_internal_port),
                path_prefix: "/".to_string(),
                inject_api_key: Some(self.config.whisper_internal_api_key.clone()),
            },
            self.store.clone(),
        ).await {
            Ok(h) => h,
            Err(e) => {
                // h1 is already in handles; drain and abort it.
                for h in self.handles.lock().await.drain(..) { h.abort(); }
                return Err(e);
            }
        };

        // Push h2 immediately so that if anything below fails, stop() can abort it.
        self.handles.lock().await.push(h2);

        // LM Studio auth proxy — only when LM Studio is detected locally.
        // Symmetric with the Ollama route: bearer-validated, strips the
        // inbound Authorization, no upstream auth (LM Studio doesn't validate
        // bearers). Skipped when either port is None — that includes the
        // common case where the user hasn't started LM Studio's local server.
        if let (Some(internal), Some(proxy)) = (
            self.config.lmstudio_internal_port,
            self.config.lmstudio_proxy_port,
        ) {
            tracing::info!(
                "LM Studio detected on 127.0.0.1:{internal}; spawning auth proxy on {proxy}"
            );
            let h_lm = match spawn_auth_proxy(
                ProxyConfig {
                    listen_port: proxy,
                    backend_url: format!("http://127.0.0.1:{internal}"),
                    path_prefix: "/".to_string(),
                    inject_api_key: None,
                },
                self.store.clone(),
            ).await {
                Ok(h) => h,
                Err(e) => {
                    for h in self.handles.lock().await.drain(..) { h.abort(); }
                    return Err(e);
                }
            };
            self.handles.lock().await.push(h_lm);
        } else {
            tracing::warn!(
                "LM Studio not detected on 127.0.0.1:1234 at Start sharing; LM Studio models will not be available to paired clients. Stop and Start sharing again with LM Studio running to enable."
            );
        }

        // Whisper child — if this fails, roll back the proxy tasks above.
        if let Err(e) = self.whisper.start().await {
            for h in self.handles.lock().await.drain(..) { h.abort(); }
            return Err(SharingError::WhisperSupervisor(e.to_string()));
        }

        // mDNS — roll back on failure.
        let mdns = match MdnsAdvertiser::start(
            &self.config.friendly_name,
            &ServerPorts {
                ollama: Some(self.config.ollama_proxy_port),
                whisper: Some(self.config.whisper_proxy_port),
                lmstudio: self.config.lmstudio_proxy_port,
                pairing: Some(self.config.pairing_port),
                vocab: Some(self.config.vocab_port),
            },
            &self.config.version,
        ) {
            Ok(m) => m,
            Err(e) => {
                self.whisper.stop().await;
                for h in self.handles.lock().await.drain(..) { h.abort(); }
                return Err(e);
            }
        };
        *self.mdns.lock().await = Some(mdns);

        // Pairing HTTP service — bind first so port conflicts surface as errors.
        let info_snapshot = InfoSnapshot {
            host: self.config.friendly_name.clone(),
            version: self.config.version.clone(),
            ports: ServerPorts {
                ollama: Some(self.config.ollama_proxy_port),
                whisper: Some(self.config.whisper_proxy_port),
                lmstudio: self.config.lmstudio_proxy_port,
                pairing: Some(self.config.pairing_port),
                vocab: Some(self.config.vocab_port),
            },
        };
        let h3 = match spawn_pairing_service(
            self.config.pairing_port,
            self.pairing.clone(),
            self.store.clone(),
            info_snapshot,
        ).await {
            Ok(h) => h,
            Err(e) => {
                if let Some(m) = self.mdns.lock().await.take() { m.stop(); }
                self.whisper.stop().await;
                for h in self.handles.lock().await.drain(..) { h.abort(); }
                return Err(e);
            }
        };

        self.handles.lock().await.push(h3);
        *running = true;
        Ok(())
    }

    /// Stop all sharing subsystems.
    ///
    /// Unregisters mDNS, kills the whisper-server child, and aborts all
    /// proxy/pairing join handles. Idempotent: calling `stop()` when already
    /// stopped is a no-op.
    pub async fn stop(&self) -> Result<(), SharingError> {
        let mut running = self.running.lock().await;
        if !*running { return Ok(()); }
        if let Some(m) = self.mdns.lock().await.take() {
            m.stop();
        }
        self.whisper.stop().await;
        for h in self.handles.lock().await.drain(..) {
            h.abort();
        }
        *running = false;
        Ok(())
    }

    /// Snapshot the current health of all subsystems.
    ///
    /// Returns immediately without blocking on any subsystem. The
    /// `paired_clients` count is read from the token store synchronously.
    pub async fn status(&self) -> SharingStatus {
        let running = *self.running.lock().await;
        let n = self
            .store
            .list()
            .map(|v| v.len() as u32)
            .unwrap_or(0);
        let r = self.readiness.read().await;
        let get = |k: UpstreamKind| -> ProbeState {
            *r.get(&k).unwrap_or(&ProbeState::default())
        };
        let ollama = get(UpstreamKind::Ollama);
        let whisper = get(UpstreamKind::Whisper);
        let lmstudio = get(UpstreamKind::LmStudio);
        SharingStatus {
            enabled: running,
            ollama_ok: running && ollama.last_probe_ok,
            whisper_ok: running && whisper.last_probe_ok,
            lmstudio_ok: running && lmstudio.configured && lmstudio.last_probe_ok,
            mdns_ok: running,
            pairing_ok: running,
            paired_clients: n,
        }
    }
}

/// Public, unauthenticated snapshot of an office server's identity and
/// service ports. Returned by GET /info on the pairing port so clients
/// that can't see the server's mDNS broadcasts (e.g. across Tailscale)
/// can probe for FerriScribe servers without exchanging secrets.
#[derive(Clone, Serialize)]
pub struct InfoSnapshot {
    pub host: String,
    pub version: String,
    pub ports: crate::mdns::ServerPorts,
}

pub(crate) fn build_pairing_router(
    pairing: Arc<PairingState>,
    store: Arc<TokenStore>,
    info: InfoSnapshot,
) -> axum::Router {
    use std::net::SocketAddr;
    use axum::{Json, Router, extract::{ConnectInfo, State}, routing::{get, post}};
    use serde::{Deserialize, Serialize};

    #[derive(Clone)]
    struct St { pairing: Arc<PairingState>, store: Arc<TokenStore>, info: InfoSnapshot }

    #[derive(Deserialize)]
    struct EnrollReq { code: String, label: String }
    #[derive(Serialize)]
    struct EnrollResp { token: String }

    async fn enroll(
        State(st): State<St>,
        Json(req): Json<EnrollReq>,
    ) -> Result<Json<EnrollResp>, axum::http::StatusCode> {
        let token = st
            .pairing
            .enroll(&req.code, &req.label)
            .await
            .map_err(|_| axum::http::StatusCode::UNAUTHORIZED)?;
        Ok(Json(EnrollResp { token }))
    }

    #[derive(Serialize)]
    struct ClientView { id: i64, label: String }

    /// Admin endpoint: list paired clients. Loopback-only.
    async fn list_clients(
        State(st): State<St>,
        ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ) -> Result<Json<Vec<ClientView>>, axum::http::StatusCode> {
        if !addr.ip().is_loopback() {
            return Err(axum::http::StatusCode::FORBIDDEN);
        }
        let v = st
            .store
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|r| ClientView { id: r.id, label: r.label })
            .collect();
        Ok(Json(v))
    }

    /// Admin endpoint: revoke a client. Loopback-only.
    async fn revoke(
        State(st): State<St>,
        ConnectInfo(addr): ConnectInfo<SocketAddr>,
        axum::extract::Path(id): axum::extract::Path<i64>,
    ) -> axum::http::StatusCode {
        if !addr.ip().is_loopback() {
            return axum::http::StatusCode::FORBIDDEN;
        }
        match st.store.revoke(id) {
            Ok(_) => axum::http::StatusCode::NO_CONTENT,
            Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Public discovery: same shape mDNS broadcasts (friendly name,
    /// version, public ports). No secrets, no codes — reaching it tells
    /// you no more than seeing the office server on the LAN would.
    async fn info_handler(State(st): State<St>) -> Json<InfoSnapshot> {
        Json(st.info.clone())
    }

    let st = St { pairing, store, info };
    Router::new()
        .route("/pair/enroll", post(enroll))
        .route("/pair/clients", get(list_clients))
        .route("/pair/revoke/{id}", post(revoke))
        .route("/info", get(info_handler))
        .with_state(st)
}

async fn spawn_pairing_service(
    port: u16,
    pairing: Arc<PairingState>,
    store: Arc<TokenStore>,
    info: InfoSnapshot,
) -> crate::Result<tokio::task::JoinHandle<()>> {
    use std::net::SocketAddr;

    let app = build_pairing_router(pairing, store, info);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| crate::SharingError::Pairing(format!("bind 0.0.0.0:{port}: {e}")))?;

    Ok(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        ).await;
    }))
}

#[cfg(test)]
mod pairing_router_tests {
    use super::*;
    use crate::mdns::ServerPorts;
    use crate::pairing::PairingState;
    use crate::token_store::TokenStore;
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::{Method, Request, StatusCode};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tower::ServiceExt;

    fn fresh_store_and_pairing() -> (tempfile::TempDir, Arc<TokenStore>, Arc<PairingState>) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.db");
        let key = [0u8; 32];
        let store = Arc::new(TokenStore::open(&path, &key).expect("open store"));
        let pairing = Arc::new(PairingState::new(store.clone()));
        (dir, store, pairing)
    }

    fn sample_info() -> InfoSnapshot {
        InfoSnapshot {
            host: "test-host".into(),
            version: "9.9.9".into(),
            ports: ServerPorts {
                ollama: Some(11435),
                whisper: Some(8081),
                lmstudio: None,
                pairing: Some(11436),
                vocab: Some(11437),
            },
        }
    }

    fn loopback_connect_info() -> ConnectInfo<SocketAddr> {
        ConnectInfo("127.0.0.1:50000".parse().unwrap())
    }

    fn lan_connect_info() -> ConnectInfo<SocketAddr> {
        ConnectInfo("192.168.1.50:50000".parse().unwrap())
    }

    fn json_body<T: serde::Serialize>(v: &T) -> Body {
        Body::from(serde_json::to_vec(v).unwrap())
    }

    #[tokio::test]
    async fn enroll_succeeds_with_valid_code() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let app = build_pairing_router(pairing.clone(), store.clone(), sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri("/pair/enroll")
            .header("content-type", "application/json")
            .body(json_body(&serde_json::json!({ "code": code, "label": "iPad" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed["token"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[tokio::test]
    async fn enroll_returns_401_on_invalid_code() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let app = build_pairing_router(pairing, store.clone(), sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri("/pair/enroll")
            .header("content-type", "application/json")
            .body(json_body(&serde_json::json!({ "code": "000000", "label": "iPad" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(store.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn enroll_persists_token_in_store() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let app = build_pairing_router(pairing.clone(), store.clone(), sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri("/pair/enroll")
            .header("content-type", "application/json")
            .body(json_body(&serde_json::json!({ "code": code, "label": "phone-1" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "phone-1");
    }

    #[tokio::test]
    async fn list_clients_from_loopback_returns_paired_clients() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let _ = pairing.enroll(&code, "loopback-client").await.unwrap();
        let app = build_pairing_router(pairing, store, sample_info());

        let req = Request::builder()
            .method(Method::GET)
            .uri("/pair/clients")
            .extension(loopback_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["label"], "loopback-client");
    }

    #[tokio::test]
    async fn list_clients_from_non_loopback_returns_403() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let _ = pairing.enroll(&code, "client").await.unwrap();
        let app = build_pairing_router(pairing, store, sample_info());

        let req = Request::builder()
            .method(Method::GET)
            .uri("/pair/clients")
            .extension(lan_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn revoke_from_loopback_removes_token() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let _ = pairing.enroll(&code, "to-revoke").await.unwrap();
        let id = store.list().unwrap()[0].id;
        let app = build_pairing_router(pairing, store.clone(), sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("/pair/revoke/{id}"))
            .extension(loopback_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(store.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn revoke_from_non_loopback_returns_403() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let _ = pairing.enroll(&code, "to-keep").await.unwrap();
        let id = store.list().unwrap()[0].id;
        let app = build_pairing_router(pairing, store.clone(), sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("/pair/revoke/{id}"))
            .extension(lan_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn revoke_returns_204_even_for_unknown_id() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let app = build_pairing_router(pairing, store, sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri("/pair/revoke/99999")
            .extension(loopback_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn info_returns_snapshot_with_configured_ports() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let app = build_pairing_router(pairing, store, sample_info());

        let req = Request::builder()
            .method(Method::GET)
            .uri("/info")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["host"], "test-host");
        assert_eq!(parsed["version"], "9.9.9");
        assert_eq!(parsed["ports"]["ollama"], 11435);
        assert_eq!(parsed["ports"]["pairing"], 11436);
        assert_eq!(parsed["ports"]["vocab"], 11437);
        assert!(parsed["ports"]["lmstudio"].is_null());
    }

    #[tokio::test]
    async fn info_requires_no_auth_or_loopback() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let app = build_pairing_router(pairing, store, sample_info());

        let req = Request::builder()
            .method(Method::GET)
            .uri("/info")
            .extension(lan_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn cfg_with_tokens_at(path: PathBuf, key: [u8; 32], api_key: &str) -> SharingConfig {
        SharingConfig {
            enabled: true,
            friendly_name: "test-server".into(),
            ollama_proxy_port: 11435,
            whisper_proxy_port: 8081,
            pairing_port: 11436,
            whisper_internal_port: 8080,
            lmstudio_internal_port: None,
            lmstudio_proxy_port: None,
            vocab_port: 11437,
            token_store_path: path,
            token_store_key: key,
            binary_dir: PathBuf::from("/tmp"),
            whisper_model_path: PathBuf::from("/tmp/model.bin"),
            whisper_internal_api_key: api_key.to_string(),
            version: "9.9.9".into(),
        }
    }

    #[test]
    fn sharing_config_default_has_expected_ports() {
        let c = SharingConfig::default();
        assert_eq!(c.ollama_proxy_port, 11435);
        assert_eq!(c.whisper_proxy_port, 8081);
        assert_eq!(c.pairing_port, 11436);
        assert_eq!(c.whisper_internal_port, 8080);
        assert_eq!(c.vocab_port, 11437);
    }

    #[test]
    fn sharing_config_default_is_disabled() {
        let c = SharingConfig::default();
        assert!(!c.enabled);
        assert!(c.lmstudio_internal_port.is_none());
        assert!(c.lmstudio_proxy_port.is_none());
    }

    #[test]
    fn sharing_config_debug_redacts_token_store_key() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7).wrapping_add(3);
        }
        let c = cfg_with_tokens_at(PathBuf::from("/tmp/x"), key, "irrelevant");
        let dbg = format!("{:?}", c);
        assert!(dbg.contains("<redacted: 32 bytes>"), "Debug must redact key marker; got: {dbg}");
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert!(
            !dbg.to_lowercase().contains(&hex),
            "Debug must not contain key bytes as hex"
        );
    }

    #[test]
    fn sharing_config_debug_redacts_whisper_internal_api_key() {
        let api_key = "secret-key-DO-NOT-LEAK-12345";
        let c = cfg_with_tokens_at(PathBuf::from("/tmp/x"), [0u8; 32], api_key);
        let dbg = format!("{:?}", c);
        assert!(dbg.contains("<redacted>"), "Debug must contain redacted marker for api key; got: {dbg}");
        assert!(
            !dbg.contains(api_key),
            "Debug must not contain literal api key"
        );
    }

    #[test]
    fn sharing_service_new_creates_token_store_on_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.db");
        let c = cfg_with_tokens_at(path.clone(), [0u8; 32], "k");
        let _svc = SharingService::new(c).expect("new() should succeed");
        assert!(path.exists(), "token store db should be created on disk");
    }

    #[cfg(unix)]
    #[test]
    fn sharing_service_new_returns_token_store_error_on_unwritable_path() {
        // A path under /dev/null/... can't be created because /dev/null isn't a directory.
        // Unix-specific because /dev/null doesn't resolve the same way on Windows.
        let c = cfg_with_tokens_at(
            PathBuf::from("/dev/null/cannot-create/tokens.db"),
            [0u8; 32],
            "k",
        );
        match SharingService::new(c) {
            Ok(_) => panic!("expected TokenStore error, but new() succeeded"),
            Err(e) => assert!(
                matches!(e, SharingError::TokenStore(_)),
                "expected TokenStore variant, got {e:?}"
            ),
        }
    }

    #[tokio::test]
    async fn sharing_service_status_when_not_running_reports_disabled() {
        let dir = tempdir().unwrap();
        let c = cfg_with_tokens_at(dir.path().join("tokens.db"), [0u8; 32], "k");
        let svc = SharingService::new(c).unwrap();
        let s = svc.status().await;
        assert!(!s.enabled);
        assert!(!s.ollama_ok);
        assert!(!s.whisper_ok);
        assert!(!s.lmstudio_ok);
        assert!(!s.mdns_ok);
        assert!(!s.pairing_ok);
        assert_eq!(s.paired_clients, 0);
    }

    #[tokio::test]
    async fn sharing_service_status_counts_paired_clients_when_stopped() {
        let dir = tempdir().unwrap();
        let c = cfg_with_tokens_at(dir.path().join("tokens.db"), [0u8; 32], "k");
        let svc = SharingService::new(c).unwrap();
        let pairing = svc.pairing_state();
        let code = pairing.issue_code().await;
        let _token = pairing.enroll(&code, "client-a").await.unwrap();
        let s = svc.status().await;
        assert_eq!(s.paired_clients, 1);
        assert!(!s.enabled);
    }

    #[tokio::test]
    async fn sharing_service_stop_is_idempotent_when_never_started() {
        let dir = tempdir().unwrap();
        let c = cfg_with_tokens_at(dir.path().join("tokens.db"), [0u8; 32], "k");
        let svc = SharingService::new(c).unwrap();
        svc.stop().await.expect("first stop should be Ok");
        svc.stop().await.expect("second stop should also be Ok");
    }

    #[tokio::test]
    async fn status_reflects_readiness_cache_not_running_flag() {
        let dir = tempdir().unwrap();
        let mut c = cfg_with_tokens_at(dir.path().join("tokens.db"), [0u8; 32], "k");
        // Mark LM Studio as a configured candidate (office mode default).
        c.lmstudio_internal_port = Some(1234);
        c.lmstudio_proxy_port = Some(1235);
        let svc = SharingService::new(c).unwrap();

        // Force the cache into a mixed state without calling start() (which
        // needs real upstreams). Ollama ok, whisper ok, lmstudio not.
        {
            let mut r = svc.readiness.write().await;
            r.get_mut(&UpstreamKind::Ollama).unwrap().last_probe_ok = true;
            r.get_mut(&UpstreamKind::Whisper).unwrap().last_probe_ok = true;
            r.get_mut(&UpstreamKind::LmStudio).unwrap().last_probe_ok = false;
            // Simulate start() having run: running=true.
            *svc.running.lock().await = true;
        }

        let s = svc.status().await;
        assert!(s.enabled);
        assert!(s.ollama_ok, "ollama reflects probe cache");
        assert!(s.whisper_ok, "whisper reflects probe cache");
        assert!(!s.lmstudio_ok, "lmstudio reflects probe cache (not running flag)");
    }
}
