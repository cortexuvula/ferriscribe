use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::info;

use medical_ai_providers::ProviderRegistry;
use medical_ai_providers::http_client::RetryConfig;
use medical_ai_providers::lmstudio::LmStudioProvider;
use medical_ai_providers::ollama::OllamaProvider;

use medical_core::types::RemoteEndpoint;
use medical_core::types::settings::AppConfig;

use medical_agents::orchestrator::AgentOrchestrator;
use medical_agents::tools::{RagSearchTool, ToolRegistry};

use medical_audio::capture::CaptureHandle;

use medical_db::Database;
use medical_db::recordings::RecordingsRepo;

use medical_rag::bm25::Bm25Search;
use medical_rag::embeddings::EmbeddingGenerator;
use medical_rag::graph_search::GraphSearch;
use medical_rag::ingestion::IngestionPipeline;
use medical_rag::vector_store::VectorStore;

use medical_security::key_storage::KeyStorage;

use medical_stt_providers::models as stt_models;

use medical_core::traits::SttProvider;

/// Errors that can be returned from `AppState::initialize()` to signal
/// special boot conditions to the caller.
#[derive(Debug)]
pub enum InitError {
    /// Encrypted DB exists but the keychain entry is missing or inaccessible.
    /// The caller should emit a `database-recovery-needed` event and skip the
    /// rest of normal app initialization.
    DatabaseRecoveryNeeded { reason: String },
    /// Any other initialization error (treated as fatal).
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitError::DatabaseRecoveryNeeded { reason } => {
                write!(f, "database recovery needed: {reason}")
            }
            InitError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InitError {}

impl From<std::io::Error> for InitError {
    fn from(e: std::io::Error) -> Self {
        InitError::Other(Box::new(e))
    }
}

impl From<medical_db::DbError> for InitError {
    fn from(e: medical_db::DbError) -> Self {
        InitError::Other(Box::new(e))
    }
}

impl From<medical_security::SecurityError> for InitError {
    fn from(e: medical_security::SecurityError) -> Self {
        InitError::Other(Box::new(e))
    }
}

impl From<medical_core::error::AppError> for InitError {
    fn from(e: medical_core::error::AppError) -> Self {
        InitError::Other(Box::new(e))
    }
}

/// Holds the recovery reason between boot and the frontend's mount.
///
/// Always registered with `app.manage(...)` (regardless of which init branch
/// fires) so the `get_database_recovery_state` command can query it without
/// depending on `AppState`. `Some(reason)` means the frontend should render
/// the recovery dialog; `None` means normal boot.
///
/// This replaces the racy 500ms-delay-and-emit pattern: the frontend invokes
/// the query command on mount instead of subscribing to an event that may
/// fire before the listener is registered.
#[derive(Default)]
pub struct RecoveryState(pub std::sync::Mutex<Option<String>>);

impl RecoveryState {
    pub fn set(&self, reason: String) {
        if let Ok(mut guard) = self.0.lock() {
            *guard = Some(reason);
        }
    }

    pub fn get(&self) -> Option<String> {
        self.0.lock().ok().and_then(|g| g.clone())
    }
}

/// Holds a fatal initialization error between boot and the frontend's mount.
///
/// Always registered with `app.manage(...)` (regardless of which init branch
/// fires) so the `get_fatal_error` command can query it without depending on
/// `AppState`. `Some(message)` means the frontend should render the fatal-error
/// dialog; `None` means normal boot (or recovery mode — see [`RecoveryState`]).
///
/// This exists so a non-recovery init failure (DB corruption, migration error,
/// I/O error) surfaces a usable dialog instead of panicking the process with no
/// UI — which under `panic = "abort"` (release profile) was a silent hard exit.
#[derive(Default)]
pub struct FatalErrorState(pub std::sync::Mutex<Option<String>>);

impl FatalErrorState {
    pub fn set(&self, message: String) {
        if let Ok(mut guard) = self.0.lock() {
            *guard = Some(message);
        }
    }

    pub fn get(&self) -> Option<String> {
        self.0.lock().ok().and_then(|g| g.clone())
    }
}

/// Wrapper to make `CaptureHandle` usable across threads.
///
/// `CaptureHandle` holds a `cpal::Stream`, which cpal marks `!Send` on every
/// platform as a defensive measure — the underlying audio APIs are actually
/// thread-safe on each of the three desktop platforms we target:
///
/// * **macOS (CoreAudio):** `AudioUnit*` APIs are documented as thread-safe
///   since 10.13; audio callbacks already run on a dedicated real-time thread
///   and `AudioOutputUnitStop` / `AudioUnitUninitialize` can be invoked from
///   any thread.
/// * **Windows (WASAPI):** `IAudioClient::Stop` and `Release` can be called
///   from any thread; the stream is stopped before any cross-thread drop.
/// * **Linux (ALSA):** `snd_pcm_drop` / `snd_pcm_close` are documented as safe
///   from any thread so long as the handle is only touched by one caller at a
///   time.
///
/// Our invariants:
/// * All access to `CaptureHandle`'s methods (`pause`/`resume`, `Drop`) is
///   serialised through `AppState::capture_handle` (a `std::sync::Mutex`),
///   so no two threads ever reach the inner FFI simultaneously.
/// * The handle is moved to a `tokio::task::spawn_blocking` worker before
///   being dropped, so the potentially-blocking drain-thread join never
///   happens on the async runtime's worker threads.
///
/// Given those, marking the wrapper `Send + Sync` is sound. If a future
/// refactor removes the mutex guard or introduces parallel access, revisit
/// this.
pub struct SendCaptureHandle(pub Option<CaptureHandle>);

// SAFETY: see the doc comment above. Access is serialised through the
// `AppState::capture_handle` Mutex; the underlying platform audio APIs are
// thread-safe on macOS/Windows/Linux; and the !Send marker on cpal::Stream
// is defensive rather than reflecting a real platform constraint.
unsafe impl Send for SendCaptureHandle {}
unsafe impl Sync for SendCaptureHandle {}

/// Tracks the currently active recording session.
///
/// Stored in `AppState::current_recording` so commands can look up the WAV
/// path and start time for the in-progress recording.
pub struct CurrentRecording {
    /// UUID of the recording row in the database.
    pub id: String,
    /// Path to the WAV file being written.
    pub wav_path: PathBuf,
    /// When the recording started (for elapsed-time display).
    pub started_at: Instant,
}

/// Application state managed by Tauri and injected into every command via
/// `tauri::State<'_, AppState>`.
///
/// Created by [`AppState::initialize()`] during app startup in `lib.rs::run()`.
/// Holds the database, AI/STT providers, audio capture handle, RAG subsystem,
/// and the lazy-initialized sharing service. All fields are `Arc`-wrapped for
/// cheap cloning into async tasks.
///
/// # Lifetime
///
/// `AppState` lives for the duration of the Tauri app. Commands borrow it via
/// `tauri::State<'_, AppState>` which ties the borrow to the command's
/// execution. Don't move it out or store it beyond the command's lifetime.
///
/// # Recovery mode
///
/// When `initialize()` returns `InitError::DatabaseRecoveryNeeded`, `AppState`
/// is **not** registered with Tauri. Commands that depend on it will fail; the
/// frontend renders a recovery dialog instead.
pub struct AppState {
    /// SQLite database (optionally SQLCipher-encrypted). Shared across all
    /// commands and background tasks.
    pub db: Arc<Database>,
    /// OS-keychain-backed API key store. Used for STT remote API keys and
    /// other secrets that shouldn't live in the settings JSON.
    pub keys: Arc<KeyStorage>,
    /// Root data directory (`~/{data}/rust-medical-assistant/`). Recordings,
    /// models, and config subdirectories live under here.
    pub data_dir: PathBuf,
    /// Whether an audio recording is currently in progress. Checked-and-set
    /// atomically under the mutex to prevent concurrent recordings.
    pub recording_active: Arc<Mutex<bool>>,
    /// Registry of AI providers (Ollama, LM Studio). Commands resolve the
    /// active provider by name from this registry.
    pub ai_providers: Arc<Mutex<ProviderRegistry>>,
    /// Active STT provider. `None` if STT initialization failed at startup.
    /// The inner `Arc<dyn SttProvider>` is either a `LocalSttProvider`
    /// (whisper.cpp) or `RemoteSttProvider` depending on user settings.
    pub stt_providers: Arc<Mutex<Option<Arc<dyn SttProvider + Send + Sync>>>>,
    /// Agent orchestrator for chat and agent-driven commands. Holds the
    /// tool registry (RAG search) and all agent definitions.
    pub orchestrator: Arc<AgentOrchestrator>,
    /// Active audio capture stream. Wrapped in `SendCaptureHandle` to satisfy
    /// `Send + Sync` bounds. Access serialized through the `std::sync::Mutex`.
    pub capture_handle: Arc<std::sync::Mutex<SendCaptureHandle>>,
    /// Metadata about the currently active recording session (ID, WAV path,
    /// start time). `None` when no recording is in progress.
    pub current_recording: Arc<std::sync::Mutex<Option<CurrentRecording>>>,
    /// Cancel tokens for in-flight pipelines, keyed by recording id. The
    /// pipeline inserts its own token on entry and removes it on exit;
    /// `cancel_pipeline` calls `.cancel()` to signal in-flight tasks and
    /// poll points to bail out.
    pub pipeline_cancels: Arc<std::sync::Mutex<HashMap<String, CancellationToken>>>,
    // RAG subsystem
    /// Embedding generator for vector-store ingestion and similarity search.
    pub embedding_generator: Arc<EmbeddingGenerator>,
    /// Vector store for semantic search over ingested documents.
    pub vector_store: Arc<VectorStore>,
    /// BM25 index for keyword search over ingested documents.
    pub bm25_search: Arc<Bm25Search>,
    /// Graph search over the knowledge graph. Currently consumed only by
    /// `IngestionPipeline`; held here so future Tauri commands can issue
    /// direct graph queries.
    #[allow(dead_code)]
    pub graph_search: Arc<GraphSearch>,
    /// Document ingestion pipeline. Parses, chunks, embeds, and indexes
    /// documents into the RAG subsystem.
    pub ingestion: Arc<IngestionPipeline>,
    /// Lazy-initialized sharing service. `None` until `start_sharing` is called.
    pub sharing: Arc<RwLock<Option<Arc<medical_sharing::SharingService>>>>,
    /// JoinHandle for the vocab CRUD HTTP API task spawned alongside the
    /// sharing service. Held here (rather than in SharingService) because
    /// the API handlers need DB access that lives in the Tauri layer.
    pub vocab_api: RwLock<Option<tokio::task::JoinHandle<()>>>,
    // ── Typed provider handles for runtime endpoint updates ──────────────────
    /// Concrete Ollama provider reference; allows `set_endpoint` after startup.
    /// Wrapped in `RwLock` so `reinit_providers` / `download_model` can replace
    /// the Arc after a full reinit, keeping registry and typed handle in sync.
    pub ollama_provider: RwLock<Option<Arc<OllamaProvider>>>,
    /// Concrete LM Studio provider reference; allows `set_endpoint` after startup.
    pub lmstudio_provider: RwLock<Option<Arc<LmStudioProvider>>>,
    /// Concrete RemoteSttProvider reference; `None` when STT mode is Local.
    pub remote_stt_provider:
        RwLock<Option<Arc<medical_stt_providers::remote_provider::RemoteSttProvider>>>,
    /// Shared HTTP client for connection-test and pairing commands.
    /// Pooled per-host; reuse this instead of constructing a fresh
    /// `reqwest::Client` per call.
    pub http_client: Arc<reqwest::Client>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Paired-endpoint helpers — read the on-disk pairing metadata and keychain
// ──────────────────────────────────────────────────────────────────────────────

/// Load the persisted `PairedConnection` from disk (non-secret endpoint metadata).
/// Returns `None` if this machine has never paired with an office server or the
/// file can't be read.
pub fn load_paired_connection() -> Option<crate::commands::sharing::PairedConnection> {
    let path = dirs::data_dir()?
        .join("rust-medical-assistant")
        .join("sharing-paired.json");
    if !path.exists() {
        return None;
    }
    let json = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&json) {
        Ok(conn) => Some(conn),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse paired-connection JSON; treating as not paired");
            None
        }
    }
}

/// Load the persisted office-server config from disk. Returns `None` if this
/// machine has never run as an office server (or the file is unreadable /
/// malformed). Used by the app-startup auto-resume hook in `lib.rs::run`.
pub fn load_server_config() -> Option<crate::commands::sharing::ServerConfig> {
    let path = dirs::data_dir()?
        .join("rust-medical-assistant")
        .join("sharing-server.json");
    if !path.exists() {
        return None;
    }
    let json = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&json) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse server-config JSON; using defaults");
            None
        }
    }
}

/// Load the bearer token stored in the OS keychain after pairing.
/// Returns `None` if not paired or the keychain entry is absent.
pub fn load_sharing_bearer() -> Option<String> {
    keyring::Entry::new("rustMedicalAssistant", "sharing-bearer")
        .ok()?
        .get_password()
        .ok()
}

// ──────────────────────────────────────────────────────────────────────────────

/// The return type of `init_ai_providers`, bundling the registry with typed
/// Arc handles so callers can update endpoints after startup.
pub struct AiProviderHandles {
    pub registry: ProviderRegistry,
    pub ollama: Option<Arc<OllamaProvider>>,
    pub lmstudio: Option<Arc<LmStudioProvider>>,
}

/// Register all supported AI providers (LM Studio + Ollama).
///
/// `config` supplies host/port; `ollama_ep` / `lmstudio_ep` override with a
/// `RemoteEndpoint` for LAN/Tailscale resolution when this machine is a paired
/// client. Pass `None` for local-only (default) mode.
pub fn init_ai_providers(
    config: &AppConfig,
    ollama_ep: Option<RemoteEndpoint>,
    lmstudio_ep: Option<RemoteEndpoint>,
) -> AiProviderHandles {
    let mut registry = ProviderRegistry::new();
    let policy = RetryConfig::from_app_config(config);
    let mut ollama_handle: Option<Arc<OllamaProvider>> = None;
    let mut lmstudio_handle: Option<Arc<LmStudioProvider>> = None;

    // Ollama — always available (local, no key needed).
    // Builder failures are logged and the provider skipped rather than
    // crashing startup, so a weird system HTTP config doesn't brick the app.
    let ollama_host = if config.ollama_host.is_empty() {
        "localhost"
    } else {
        &config.ollama_host
    };
    let ollama_url = format!("http://{}:{}", ollama_host, config.ollama_port);
    // Bearer is taken from the endpoint (if set) for proxied remote connections.
    let ollama_bearer = ollama_ep.as_ref().and_then(|ep| ep.bearer.clone());
    match OllamaProvider::new_with_endpoint(
        Some(&ollama_url),
        config.allow_public_endpoint,
        ollama_bearer,
        policy.clone(),
        ollama_ep,
    ) {
        Ok(p) => {
            info!(url = %ollama_url, "Registering Ollama provider");
            let arc = Arc::new(p);
            registry.register(Arc::clone(&arc) as Arc<dyn medical_core::traits::AiProvider>);
            ollama_handle = Some(arc);
        }
        Err(e) => {
            tracing::error!(error = %e, url = %ollama_url, "Failed to build Ollama provider; skipping")
        }
    }

    // LM Studio — always available (local or remote, no key needed)
    let lmstudio_host = if config.lmstudio_host.is_empty() {
        "localhost"
    } else {
        &config.lmstudio_host
    };
    let lmstudio_url = format!("http://{}:{}", lmstudio_host, config.lmstudio_port);
    let lmstudio_bearer = lmstudio_ep.as_ref().and_then(|ep| ep.bearer.clone());
    match LmStudioProvider::new_with_endpoint(
        Some(&lmstudio_url),
        config.allow_public_endpoint,
        lmstudio_bearer,
        policy.clone(),
        lmstudio_ep,
    ) {
        Ok(p) => {
            info!(url = %lmstudio_url, "Registering LM Studio provider");
            let arc = Arc::new(p);
            registry.register(Arc::clone(&arc) as Arc<dyn medical_core::traits::AiProvider>);
            lmstudio_handle = Some(arc);
        }
        Err(e) => {
            tracing::error!(error = %e, url = %lmstudio_url, "Failed to build LM Studio provider; skipping")
        }
    }

    info!("AI providers available: {:?}", registry.list_available());
    AiProviderHandles {
        registry,
        ollama: ollama_handle,
        lmstudio: lmstudio_handle,
    }
}

/// The return type of `init_stt_providers_with_config`, bundling the trait
/// object with a typed Arc handle for runtime endpoint updates.
pub struct SttProviderHandles {
    pub provider: Option<Arc<dyn SttProvider + Send + Sync>>,
    /// Non-`None` only when `stt_mode` is `Remote`; enables `set_endpoint`.
    pub remote: Option<Arc<medical_stt_providers::remote_provider::RemoteSttProvider>>,
}

/// Create the STT provider based on the user's chosen mode.
///
/// `whisper_ep` overrides the remote STT server address with a `RemoteEndpoint`
/// for LAN/Tailscale resolution. Pass `None` for local-only or static-address mode.
pub fn init_stt_providers_with_config(
    data_dir: &std::path::Path,
    config: &medical_core::types::settings::AppConfig,
    whisper_ep: Option<RemoteEndpoint>,
) -> SttProviderHandles {
    let seg_path = stt_models::pyannote_model_path(data_dir, "segmentation-3.0.onnx");
    let emb_path = stt_models::pyannote_model_path(data_dir, "wespeaker_en_voxceleb_CAM++.onnx");

    match config.stt_mode {
        medical_core::types::settings::SttMode::Local => {
            let whisper_filename = stt_models::whisper_model_filename(&config.whisper_model)
                .unwrap_or("ggml-large-v3-turbo.bin");
            let whisper_path = stt_models::whisper_model_path(data_dir, whisper_filename);
            info!(
                whisper = %whisper_path.display(),
                segmentation = %seg_path.display(),
                embedding = %emb_path.display(),
                "Initializing local STT provider"
            );
            SttProviderHandles {
                provider: Some(Arc::new(
                    medical_stt_providers::local_provider::LocalSttProvider::new(
                        whisper_path,
                        seg_path,
                        emb_path,
                    ),
                )),
                remote: None,
            }
        }
        medical_core::types::settings::SttMode::Remote => {
            // Load the remote API key from the keychain if present. A miss is
            // non-fatal — the provider just omits the Authorization header.
            // Mirror AppState::initialize's KeyStorage path: data_dir/config.
            let api_key = medical_security::key_storage::KeyStorage::open(&data_dir.join("config"))
                .ok()
                .and_then(|ks| ks.get_key("stt_remote_api_key").ok().flatten());
            // Bearer from the whisper endpoint (if set) overrides the keychain api_key.
            let bearer = whisper_ep
                .as_ref()
                .and_then(|ep| ep.bearer.clone())
                .or(api_key);

            let paired = whisper_ep.as_ref().map(|ep| {
                format!(
                    "lan={} tailscale={} port={}",
                    ep.lan.as_deref().unwrap_or("-"),
                    ep.tailscale.as_deref().unwrap_or("-"),
                    ep.port
                )
            });
            info!(
                fallback_host = %config.stt_remote_host,
                fallback_port = config.stt_remote_port,
                model = %config.stt_remote_model,
                has_bearer = bearer.is_some(),
                paired_endpoint = paired.as_deref().unwrap_or("none"),
                "Initializing remote STT provider"
            );

            match medical_stt_providers::remote_provider::RemoteSttProvider::new_with_endpoint(
                &config.stt_remote_host,
                config.stt_remote_port,
                &config.stt_remote_model,
                config.allow_public_endpoint,
                bearer,
                seg_path,
                emb_path,
                whisper_ep,
            ) {
                Ok(p) => {
                    let arc = Arc::new(p);
                    SttProviderHandles {
                        provider: Some(Arc::clone(&arc) as Arc<dyn SttProvider + Send + Sync>),
                        remote: Some(arc),
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to build remote STT provider");
                    SttProviderHandles {
                        provider: None,
                        remote: None,
                    }
                }
            }
        }
    }
}

impl AppState {
    /// Bootstrap all subsystems and return a ready-to-use `AppState`.
    ///
    /// Called once during app startup from `lib.rs::run()`. Opens the
    /// (optionally SQLCipher-encrypted) database, registers AI and STT
    /// providers, initializes the RAG subsystem, and wires up paired-endpoint
    /// metadata if this machine has previously paired with an office server.
    ///
    /// # Errors
    ///
    /// - `InitError::DatabaseRecoveryNeeded` -- encrypted DB exists but the
    ///   keychain entry is missing or inaccessible. The caller should boot in
    ///   recovery mode without managing `AppState`.
    /// - `InitError::Other` -- any other fatal error (DB corruption, I/O
    ///   failure). The caller panics so the process exits with a clear message.
    pub fn initialize() -> Result<Self, InitError> {
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rust-medical-assistant");
        std::fs::create_dir_all(&data_dir)?;

        let http_client = Arc::new(
            reqwest::Client::builder()
                .pool_max_idle_per_host(4)
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| {
                    InitError::Other(format!("Failed to build shared HTTP client: {e}").into())
                })?,
        );

        let db_path = data_dir.join("medical.db");

        // ── Database encryption setup ────────────────────────────────────
        // Look up the existing keychain entry (if any) and detect whether
        // the DB on disk is plaintext or encrypted.
        let keychain_lookup = medical_security::keychain::get_db_key();
        let plaintext_on_disk = medical_db::encryption::is_plaintext_db(&db_path).unwrap_or(false);

        let db_key: Option<[u8; 32]> = match (plaintext_on_disk, &keychain_lookup, db_path.exists())
        {
            // Encrypted DB exists, key found → normal start.
            (false, Ok(Some(key)), true) => Some(*key),

            // Encrypted DB exists but no key → recovery needed.
            (false, Ok(None), true) => {
                return Err(InitError::DatabaseRecoveryNeeded {
                    reason: "encrypted database exists but no keychain entry was found".into(),
                });
            }

            // Encrypted DB exists, keychain access failed → recovery (user must intervene).
            (false, Err(e), true) => {
                return Err(InitError::DatabaseRecoveryNeeded {
                    reason: format!("keychain access failed: {e}"),
                });
            }

            // Plaintext DB, no key yet → first-time encryption migration.
            (true, Ok(None), _) => match medical_security::keychain::get_or_create_db_key() {
                Ok(key) => {
                    match medical_db::encryption::migrate_plaintext_to_encrypted(&db_path, &key) {
                        Ok(_) => {
                            tracing::info!("Database encrypted on first launch");
                            Some(key)
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "DB encryption migration failed; continuing on plaintext");
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Could not access OS keychain; running on plaintext DB");
                    None
                }
            },

            // Plaintext DB exists but a key is also present (user replaced the file?).
            // Run on plaintext for this boot; warn loudly. A re-encryption flow
            // could be a follow-up.
            (true, Ok(Some(_)), _) => {
                tracing::warn!(
                    "Plaintext DB found but a keychain key exists; running on plaintext for safety"
                );
                None
            }

            // Plaintext DB but keychain access failed: run on plaintext, warn.
            (true, Err(e), _) => {
                tracing::warn!(error = %e, "Keychain access failed while plaintext DB exists; running on plaintext");
                None
            }

            // No DB at all (fresh install): generate key, store it; a fresh
            // encrypted DB will be created.
            (_, _, false) => match medical_security::keychain::get_or_create_db_key() {
                Ok(key) => Some(key),
                Err(e) => {
                    tracing::warn!(error = %e, "Fresh install: could not store DB key in keychain; using plaintext");
                    None
                }
            },
        };

        let db = Database::open(&db_path, db_key)?;
        let db = Arc::new(db);

        // Flip any recordings that were Processing when the previous run ended
        // (crash, hard-quit, SIGKILL) to Failed so the UI doesn't show them
        // spinning forever.  Best-effort: a DB hiccup here shouldn't block boot.
        if let Ok(conn) = db.conn() {
            match RecordingsRepo::fail_stuck_processing(
                &conn,
                "Processing interrupted — app was closed before the pipeline finished.",
            ) {
                Ok(0) => {}
                Ok(n) => info!("Marked {n} stuck Processing recording(s) as Failed on boot"),
                Err(e) => tracing::warn!("fail_stuck_processing on boot failed: {e}"),
            }
        }

        let config_dir = data_dir.join("config");
        let keys = KeyStorage::open(&config_dir)?;

        // Load saved settings to configure preferred providers
        let config = {
            let conn = db.conn().ok();
            conn.and_then(|c| medical_db::settings::SettingsRepo::load_config(&c).ok())
                .map(|mut c| {
                    c.migrate();
                    c
                })
        };
        let config_ref = config.as_ref().cloned().unwrap_or_default();

        // ── Paired-endpoint wiring ───────────────────────────────────────────
        // If this machine previously paired with an office server, load the
        // saved endpoint metadata and bearer token so providers can resolve
        // LAN / Tailscale addresses dynamically.
        let paired = load_paired_connection();
        let bearer = if paired.is_some() {
            load_sharing_bearer()
        } else {
            None
        };

        let (ollama_ep, lmstudio_ep, whisper_ep) = if let Some(ref p) = paired {
            (
                Some(RemoteEndpoint {
                    lan: p.lan.clone(),
                    tailscale: p.tailscale.clone(),
                    port: p.ports.ollama,
                    bearer: bearer.clone(),
                }),
                p.ports.lmstudio.map(|lms_port| RemoteEndpoint {
                    lan: p.lan.clone(),
                    tailscale: p.tailscale.clone(),
                    port: lms_port,
                    bearer: bearer.clone(),
                }),
                Some(RemoteEndpoint {
                    lan: p.lan.clone(),
                    tailscale: p.tailscale.clone(),
                    port: p.ports.whisper,
                    bearer: bearer.clone(),
                }),
            )
        } else {
            (None, None, None)
        };

        // Initialize provider registries from saved API keys + config
        let mut ai_handles = init_ai_providers(&config_ref, ollama_ep, lmstudio_ep);

        let stt_handles = init_stt_providers_with_config(&data_dir, &config_ref, whisper_ep);

        // Set the active AI provider from saved settings
        if let Some(ref cfg) = config
            && ai_handles.registry.set_active(&cfg.ai_provider)
        {
            info!(
                "Active AI provider set to '{}' from settings",
                cfg.ai_provider
            );
        }

        let ollama_provider = RwLock::new(ai_handles.ollama.take());
        let lmstudio_provider = RwLock::new(ai_handles.lmstudio.take());
        let remote_stt_provider = RwLock::new(stt_handles.remote.clone());

        // --- RAG subsystem ---
        let embedding_host = if config_ref.ollama_host.is_empty() {
            "localhost".to_string()
        } else {
            config_ref.ollama_host.clone()
        };
        let embedding_url = format!("http://{}:{}", embedding_host, config_ref.ollama_port);
        info!(url = %embedding_url, model = %config_ref.embedding_model, "RAG: using Ollama embeddings");
        let embedding_generator = Arc::new(EmbeddingGenerator::new_ollama(
            Some(&embedding_url),
            Some(&config_ref.embedding_model),
        )?);

        let vector_store = Arc::new(VectorStore::new(Arc::clone(&db)));
        let bm25_search = Arc::new(Bm25Search::new(Arc::clone(&db)));
        let graph_search = Arc::new(GraphSearch::new(Arc::clone(&db)));
        let ingestion = Arc::new(IngestionPipeline::new(
            Arc::clone(&embedding_generator),
            Arc::clone(&vector_store),
            Arc::clone(&graph_search),
        ));

        info!("RAG subsystem initialized");

        // Initialize the agent orchestrator with tools, including RAG-connected search
        let mut tool_registry = ToolRegistry::with_defaults();
        // Replace the default unconfigured RagSearchTool with a real one
        let rag_tool = RagSearchTool::with_rag(
            Arc::clone(&embedding_generator),
            Arc::clone(&vector_store),
            Arc::clone(&bm25_search),
        );
        tool_registry.register(Arc::new(rag_tool));
        let orchestrator = AgentOrchestrator::new(tool_registry);

        Ok(Self {
            db,
            keys: Arc::new(keys),
            data_dir,
            recording_active: Arc::new(Mutex::new(false)),
            ai_providers: Arc::new(Mutex::new(ai_handles.registry)),
            stt_providers: Arc::new(Mutex::new(stt_handles.provider)),
            orchestrator: Arc::new(orchestrator),
            capture_handle: Arc::new(std::sync::Mutex::new(SendCaptureHandle(None))),
            current_recording: Arc::new(std::sync::Mutex::new(None)),
            pipeline_cancels: Arc::new(std::sync::Mutex::new(HashMap::new())),
            embedding_generator,
            vector_store,
            bm25_search,
            graph_search,
            ingestion,
            sharing: Arc::new(RwLock::new(None)),
            vocab_api: RwLock::new(None),
            ollama_provider,
            lmstudio_provider,
            remote_stt_provider,
            http_client,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::settings::AppConfig;

    #[test]
    fn init_ai_providers_uses_configured_ollama_host() {
        let mut config = AppConfig::default();
        config.ollama_host = "tailnet-node".into();
        config.ollama_port = 11500;
        // Non-localhost hostname requires allow_public_endpoint = true.
        config.allow_public_endpoint = true;
        let handles = init_ai_providers(&config, None, None);
        assert!(
            handles
                .registry
                .list_available()
                .contains(&"ollama".to_string()),
            "ollama should still be registered with a custom host"
        );
        assert!(
            handles.ollama.is_some(),
            "ollama handle should be populated"
        );
    }

    #[test]
    fn init_stt_providers_remote_mode_builds_remote_provider() {
        use medical_core::types::settings::{AppConfig, SttMode};
        let mut cfg = AppConfig::default();
        cfg.stt_mode = SttMode::Remote;
        cfg.stt_remote_host = "tailnet-node".into();
        cfg.stt_remote_port = 8080;
        cfg.stt_remote_model = "whisper-1".into();
        // Non-localhost hostname requires allow_public_endpoint = true.
        cfg.allow_public_endpoint = true;

        let tmp = tempfile::tempdir().expect("tempdir");
        let handles = init_stt_providers_with_config(tmp.path(), &cfg, None);
        let provider = handles.provider.expect("provider should be built");
        assert_eq!(provider.name(), "remote");
        // Typed handle should be populated for remote mode.
        assert!(handles.remote.is_some(), "remote handle should be set");
    }

    #[test]
    fn init_stt_providers_local_mode_builds_local_provider() {
        use medical_core::types::settings::{AppConfig, SttMode};
        let mut cfg = AppConfig::default();
        cfg.stt_mode = SttMode::Local;
        cfg.whisper_model = "large-v3-turbo".into();

        let tmp = tempfile::tempdir().expect("tempdir");
        let handles = init_stt_providers_with_config(tmp.path(), &cfg, None);
        let provider = handles.provider.expect("provider should be built");
        assert_eq!(provider.name(), "local");
        assert!(handles.remote.is_none(), "no remote handle for local mode");
    }
}
