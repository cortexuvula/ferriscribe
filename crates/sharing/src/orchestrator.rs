//! Orchestrator — the public face of the sharing layer.
//!
//! Owns the auth proxy (Ollama route), auth proxy (whisper route), mDNS
//! advertiser, pairing service, whisper-cpp supervisor. start() boots all
//! enabled subsystems; stop() tears them down cleanly.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::SharingError;
use crate::auth_proxy::{ProxyConfig, spawn_auth_proxy};
use crate::mdns::{MdnsAdvertiser, ServerPorts};
use crate::pairing::PairingState;
use crate::token_store::TokenStore;
use crate::whisper_supervisor::WhisperSupervisor;

#[derive(Clone)]
pub struct SharingConfig {
    pub enabled: bool,
    pub friendly_name: String,
    pub ollama_proxy_port: u16,
    pub whisper_proxy_port: u16,
    pub pairing_port: u16,
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
    pub token_store_path: PathBuf,
    pub token_store_key: [u8; 32],
    pub binary_dir: PathBuf,
    pub whisper_model_path: PathBuf,
    pub whisper_internal_api_key: String,
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

#[derive(Debug, Clone, Default, Serialize)]
pub struct SharingStatus {
    pub enabled: bool,
    pub ollama_ok: bool,
    pub whisper_ok: bool,
    /// True only when LM Studio's local server was detected at Start sharing
    /// time and the auth proxy was wired up. False when LM Studio wasn't
    /// running at config time — clients won't see LM Studio models in that
    /// case until the user Stops + Starts sharing with LM Studio running.
    pub lmstudio_ok: bool,
    pub mdns_ok: bool,
    pub pairing_ok: bool,
    pub paired_clients: u32,
}

pub struct SharingService {
    config: SharingConfig,
    store: Arc<TokenStore>,
    pairing: Arc<PairingState>,
    whisper: Arc<WhisperSupervisor>,
    mdns: Mutex<Option<MdnsAdvertiser>>,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    running: Mutex<bool>,
}

impl SharingService {
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
        Ok(Self {
            config,
            store,
            pairing,
            whisper,
            mdns: Mutex::new(None),
            handles: Mutex::new(Vec::new()),
            running: Mutex::new(false),
        })
    }

    pub fn pairing_state(&self) -> Arc<PairingState> { self.pairing.clone() }
    pub fn token_store(&self) -> Arc<TokenStore> { self.store.clone() }
    pub fn config(&self) -> &SharingConfig { &self.config }

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

    pub async fn status(&self) -> SharingStatus {
        let running = *self.running.lock().await;
        let n = self
            .store
            .list()
            .map(|v| v.len() as u32)
            .unwrap_or(0);
        SharingStatus {
            enabled: running,
            ollama_ok: running,
            whisper_ok: running,
            // Reflect the wiring decision made at start_sharing time.
            // `lmstudio_proxy_port` is Some iff lmstudio_running_port()
            // detected a local LM Studio listener.
            lmstudio_ok: running && self.config.lmstudio_proxy_port.is_some(),
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

async fn spawn_pairing_service(
    port: u16,
    pairing: Arc<PairingState>,
    store: Arc<TokenStore>,
    info: InfoSnapshot,
) -> crate::Result<tokio::task::JoinHandle<()>> {
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
    let app = Router::new()
        .route("/pair/enroll", post(enroll))
        .route("/pair/clients", get(list_clients))
        .route("/pair/revoke/:id", post(revoke))
        .route("/info", get(info_handler))
        .with_state(st);

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
