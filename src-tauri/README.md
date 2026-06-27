# src-tauri (rust-medical-assistant)

Tauri v2 app shell for FerriScribe. Registers every command the Svelte frontend
calls via `invoke()`, manages application state, and coordinates all 13 workspace
crates.

This is the **binary crate** that produces the desktop app. It contains no
reusable domain logic -- every interesting operation delegates to a workspace
crate (`medical-processing`, `medical-agents`, `medical-audio`, etc.).

> **Audience:** future-you returning to this crate after months away.

---

## How It Fits in the Workspace

```
                       ┌─── medical-core (types, traits, errors)
                       ├─── medical-db (SQLite + SQLCipher)
                       ├─── medical-security (keychain, key storage)
                       ├─── medical-audio (capture, devices)
                       ├─── medical-ai-providers (Ollama, LM Studio)
                       ├─── medical-stt-providers (local whisper.cpp, remote STT)
                       ├─── medical-tts-providers
                       ├─── medical-agents (orchestrator, agent definitions)
  src-tauri ───────────┤
  (app shell)          ├─── medical-rag (BM25, vector store, ingestion)
                       ├─── medical-processing (SOAP/referral/letter generation)
                       ├─── medical-export (PDF, DOCX, FHIR)
                       ├─── medical-translation
                       └─── medical-sharing (office-server, pairing, auth proxy)
```

Every workspace crate is a dependency. None depends on `src-tauri` -- the
dependency arrow points one way.

---

## Module Map

| Module | Purpose |
|---|---|
| `lib.rs` | Entry point `run()` -- logging setup, panic hook, `AppState::initialize`, plugin registration, command handler registration. |
| `main.rs` | `fn main()` shim that calls `lib::run()`. Separate so the lib target can be linked from integration tests. |
| `state` | `AppState` (managed state), `InitError`, `RecoveryState`, provider init helpers, paired-endpoint persistence. |
| `commands` | One submodule per domain. Each `#[tauri::command]` function is a frontend-callable RPC endpoint. |
| `commands::audio` | `list_audio_devices`, `start_recording`, `stop_recording`, `cancel_recording`, `pause_recording`, `resume_recording`, `check_recording_audio_levels`, `get_recording_state`. |
| `commands::chat` | `chat_send`, `chat_stream`, `chat_with_agent`, `list_ai_providers`, `set_active_provider`, `list_models`. |
| `commands::generation` | `generate_soap`, `generate_referral`, `generate_letter`, `generate_synopsis` (in submodules `soap`, `referral`, `letter`, `synopsis`). |
| `commands::pipeline` | `process_recording`, `cancel_pipeline` -- background transcribe-then-SOAP flow. |
| `commands::transcription` | `transcribe_recording`, `list_stt_providers`. |
| `commands::recordings` | CRUD on recordings: list, get, search, delete, import. |
| `commands::recordings_edit` | `save_recording_field` -- partial updates to recording metadata. |
| `commands::settings` | `get_settings`, `save_settings`, `get_api_key`, `set_api_key`, `list_api_keys`, `get_default_prompt`. |
| `commands::providers` | `reinit_providers`, `test_lmstudio_connection`, `test_stt_remote_connection`, `test_ollama_connection`, `probe_endpoint_reachable`. |
| `commands::export` | `export_pdf`, `export_docx`, `export_fhir`. |
| `commands::vocabulary` | CRUD for the vocabulary correction list, plus JSON import/export and test-correction. |
| `commands::context_templates` | CRUD for context templates, plus JSON import/export. |
| `commands::models` | `list_whisper_models`, `list_pyannote_models`, `download_model`, `delete_model`. |
| `commands::sharing` | Office-server lifecycle (`start_sharing`, `stop_sharing`, `sharing_status`), pairing (`pairing_qr`, `list_paired_clients`, `revoke_client`, `pair_with_server`, `unpair`), discovery (`discover_servers`, `discover_via_tailscale`). |
| `commands::recovery` | `get_database_recovery_state`, `recover_database_from_path`, `recover_database_wipe`, `database_encryption_status`. |
| `commands::user_dictionary` | `user_dict_list`, `user_dict_add`, `user_dict_remove`. |
| `commands::training_corpus` | `training_corpus_counts`, `training_corpus_list`, `training_corpus_set_status`. |
| `commands::training_corpus_export` | `training_corpus_export`. |
| `commands::logging` | `get_log_path`, `get_recent_logs`, `frontend_log`. |
| `commands::letter_audiences` | `list_letter_audiences`, `upsert_letter_audience`, `delete_letter_audience`. |
| `sharing_vocab_api` | Axum HTTP server for paired-client vocabulary, context-template, and user-dictionary sync. Runs alongside the sharing service. |
| `vocab_remote` | HTTP client for the office server's `/v1/vocabulary` API. Used when this machine is a paired client. |
| `templates_remote` | HTTP client for the office server's `/v1/context-templates` API. |
| `user_dict_remote` | HTTP client for the office server's `/v1/user-dictionary` API. |
| `corpus_export` | Training-corpus export pipeline (de-identification, packaging). |

---

## Key Types

### `AppState` (in `state.rs`)

The managed state passed to every command via `tauri::State<'_, AppState>`.
Holds:

- **`db: Arc<Database>`** -- the SQLite (optionally SQLCipher-encrypted) database.
- **`keys: Arc<KeyStorage>`** -- OS-keychain-backed API key store.
- **`data_dir: PathBuf`** -- `~/{data}/rust-medical-assistant/`.
- **`ai_providers: Arc<Mutex<ProviderRegistry>>`** -- registered Ollama/LM Studio providers.
- **`stt_providers: Arc<Mutex<Option<Arc<dyn SttProvider>>>>`** -- active STT provider (local or remote).
- **`orchestrator: Arc<AgentOrchestrator>`** -- the agent orchestrator for chat and agent-driven commands.
- **`capture_handle`** -- the active audio capture stream (wrapped for `Send + Sync`).
- **`current_recording`** -- metadata about the in-progress recording session.
- **`pipeline_cancels`** -- cancel tokens for in-flight transcribe-then-SOAP pipelines, keyed by recording ID.
- **`sharing: Arc<RwLock<Option<Arc<SharingService>>>>`** -- lazy-initialized office-server service.
- **Typed provider handles** -- `ollama_provider`, `lmstudio_provider`, `remote_stt_provider` for runtime endpoint updates.
- **`http_client: Arc<reqwest::Client>`** -- shared, pooled HTTP client for connection tests and pairing.

### `InitError`

Returned by `AppState::initialize()` to signal special boot conditions:

- **`DatabaseRecoveryNeeded { reason }`** -- encrypted DB exists but the keychain entry is missing. The app boots in recovery mode with no `AppState` managed; the frontend renders the recovery dialog.
- **`Other`** -- fatal; the app panics.

### `RecoveryState`

Always managed (regardless of init outcome). `Some(reason)` means the frontend should show the recovery dialog; `None` means normal boot. Queried by the `get_database_recovery_state` command on frontend mount.

---

## How It Works

### Command Registration

All commands are registered in `lib.rs::run()` via `tauri::generate_handler![]`. Each command is a function annotated with `#[tauri::command]` that takes `tauri::State<'_, AppState>` (and optionally `tauri::AppHandle` for event emission) plus domain-specific parameters.

The Svelte frontend calls them via `invoke('command_name', { ...args })`. Tauri deserializes the JSON arguments into the Rust function's parameter types and serializes the return value (or error) back to the frontend.

### Command Call Flow Example

```
Svelte: invoke('generate_soap', { recordingId, template, context })
  │
  ▼
commands::generation::soap::generate_soap()     ← #[tauri::command]
  │  emits "generation-progress" { status: "started" }
  │
  ▼
generate_soap_inner()
  │  loads recording + settings from DB
  │  resolves AI provider from registry
  │
  ▼
medical_processing::soap_generator::generate()  ← workspace crate
  │  builds system + user prompts
  │  calls provider.complete()
  │  post-processes AI output
  │
  ▼
commands::generation::soap::generate_soap()
  │  persists SOAP note to DB
  │  emits "generation-progress" { status: "completed" }
  │  returns AppResult<String>
```

### Event System

Commands emit events to the frontend via `tauri::Emitter::emit()`. Key events:

| Event | Payload | Emitted by |
|---|---|---|
| `generation-progress` | `{ type, status, recording_id }` | `generate_soap`, `generate_referral`, `generate_letter`, `generate_synopsis` |
| `pipeline-complete` | `{ recording_id }` | `process_recording` (on success) |
| `pipeline-progress` | `{ recording_id, stage, error? }` | `process_recording` (per-stage updates including failures) |
| `transcription-progress` | `{ recording_id, status, ... }` | `transcribe_recording` |
| `chat-token` | `{ content }` | `chat_stream`, `chat_with_agent` (streaming tokens) |
| `chat-done` | `{ usage, finish_reason }` | `chat_stream`, `chat_with_agent` (stream complete) |
| `chat-error` | `{ message }` | `chat_stream`, `chat_with_agent` (stream error) |

### Pipeline Cancellation

`process_recording` inserts a `CancellationToken` into `AppState::pipeline_cancels` keyed by recording ID. The frontend calls `cancel_pipeline(recording_id)` to signal cancellation. Poll points in the transcription and generation stages check the token and bail with `AppError::Cancelled`.

### Database Recovery

When `AppState::initialize()` returns `InitError::DatabaseRecoveryNeeded`, `lib.rs` skips managing `AppState` and instead populates `RecoveryState`. The frontend queries `get_database_recovery_state` on mount and renders the recovery dialog. Recovery commands (`recover_database_from_path`, `recover_database_wipe`) don't depend on `AppState`.

### Office-Server Auto-Resume

If the user previously enabled office-server mode, `sharing-server.json` persists on disk. On startup, `lib.rs::run()` reads it and spawns `start_sharing_inner` in a background task. Failures are logged and never block app startup.

### Plugin Registration

Plugins are registered in order:
1. `tauri_plugin_deep_link` -- handles `ferriscribe://pair?...` URLs for pairing
2. `tauri_plugin_opener` -- opens external URLs/files
3. `tauri_plugin_dialog` -- native file dialogs
4. `tauri_plugin_clipboard_manager` -- clipboard access

### Logging

Two-layer `tracing` setup:
- **Console** -- compact format for terminal output.
- **Rolling file** -- daily rotation, kept for 7 days, at `~/{data}/rust-medical-assistant/logs/ferri-scribe.log`.

Controlled by `RUST_LOG` env var. Defaults filter per-crate (`rust_medical_assistant=debug`, `medical_processing=debug`, `medical_ai_providers=info`, etc.). A panic hook captures panics to the tracing log.

---

## Remote Sync APIs

When this machine runs as an office server, `sharing_vocab_api::spawn()` starts an Axum HTTP server on the vocab port. Paired clients reach these routes:

- **`/v1/vocabulary`** -- CRUD for vocabulary corrections.
- **`/v1/context-templates`** -- CRUD for context templates.
- **`/v1/user-dictionary`** -- CRUD for the spellcheck dictionary.

When this machine is a paired client, the `*_remote.rs` modules (`vocab_remote`, `templates_remote`, `user_dict_remote`) provide HTTP clients that route commands through the office server instead of the local DB, keeping the server as the canonical source of truth.

---

## Gotchas

- **State lifetime.** `AppState` is tied to the Tauri app handle. Commands receive it via `tauri::State<'_, AppState>` which borrows from the app's managed state. Don't try to move it out or store it beyond the command's lifetime.

- **Event listener cleanup on HMR.** The Svelte frontend registers event listeners in `onMount`. During HMR (hot module replacement), listeners from the previous mount may still be active. The frontend must clean up listeners in `onDestroy` to avoid duplicate handlers.

- **Deep-link handling.** The `tauri_plugin_deep_link` plugin handles `ferriscribe://pair?...` URLs. The pairing flow exchanges a one-time code for a long-lived bearer token. The URL is parsed by `medical_sharing::qr::decode_pair_url`.

- **CaptureHandle and Send.** `cpal::Stream` is `!Send` on all platforms as a defensive measure. `SendCaptureHandle` wraps it with manual `Send + Sync` impls. Access is serialized through `AppState::capture_handle` (a `std::sync::Mutex`), and the handle is moved to `spawn_blocking` before drop.

- **Encrypted database.** The DB is SQLCipher-encrypted with a key stored in the OS keychain. If the keychain entry is lost (keychain reset, migration to a new machine), `AppState::initialize()` returns `InitError::DatabaseRecoveryNeeded` and the app boots in recovery mode.

- **Provider reinit.** `reinit_providers` rebuilds all AI and STT providers from current settings. The typed provider handles (`ollama_provider`, `lmstudio_provider`, `remote_stt_provider`) are replaced under `RwLock` to keep the registry and typed handle in sync.

- **No PHI in logs.** Patient transcripts, SOAP content, medications, allergies, and conditions must never appear in `tracing::*` macros. Log counts, lengths, IDs -- never content.

- **Command module organization.** Commands are organized by domain (audio, chat, generation, pipeline, etc.). The `generation` module is further split into submodules (`soap`, `referral`, `letter`, `synopsis`) with shared helpers. The `sharing` module splits into `lifecycle`, `pairing`, and `discovery` submodules.

- **Workspace package name.** The Cargo package is `rust-medical-assistant` (not `medical-tauri`). Build with `cargo build -p rust-medical-assistant`.
