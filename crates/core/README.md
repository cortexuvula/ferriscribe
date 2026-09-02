# `medical-core`

Shared types, traits, and error handling for the FerriScribe workspace.

## Purpose

`medical-core` is the foundation leaf crate — every other workspace crate
depends on it, but it depends on none of them. It provides:

- **`AppError` / `AppResult`** — the single error type propagated across all
  crate boundaries, with variants for every subsystem and a custom
  `Serialize` impl that produces machine-readable JSON for the Tauri
  frontend.
- **Domain types** — `Recording`, `AppConfig`, `CompletionRequest`,
  `Transcript`, `PatientContext`, and the full set of structs/enums shared
  across crate boundaries.
- **Provider traits** — `AiProvider`, `SttProvider`, `TtsProvider`, `Agent`,
  `Tool`, `Exporter`, and `TranslationProvider` — the interfaces that
  provider crates implement.
- **Endpoint policy** — static (no-DNS) classification of host strings to
  enforce the local-only AI/STT constraint (PHI/HIPAA).
- **Preflight probes** — short-timeout connectivity checks run before
  expensive commands to surface offline endpoints early.

## How It Fits

```
                      ┌──────────────┐
                      │ medical-core │  ← you are here
                      └──────┬───────┘
          ┌──────────┬───────┼───────┬──────────┐
          ▼          ▼       ▼       ▼          ▼
       db     ai-providers  stt   agents   processing ...
```

Core is the leaf crate — every other workspace crate depends on it, but it
depends on none of them. If you're adding a new crate to the workspace,
`medical-core` is almost certainly in your `[dependencies]`:

```toml
[dependencies]
medical-core = { path = "../core" }
```

The package name in `Cargo.toml` is `medical-core`; import it as
`use medical_core::…`.

## Key Types

| Type | Module | What it is |
|---|---|---|
| `AppError` / `AppResult` | `error` | Workspace-wide error enum with 18+ variants, custom `Serialize` for the Tauri frontend |
| `AppConfig` | `types::settings` | The full application configuration (~60 fields), deserialized from JSON with `#[serde(default)]` everywhere for forward compatibility |
| `Recording` | `types::recording` | Central domain entity — a consultation with audio, transcript, SOAP note, and generated documents |
| `AiProvider` (trait) | `traits::ai_provider` | Async interface for AI completion (Ollama, LM Studio) — `complete`, `complete_stream`, `complete_with_tools` |
| `SttProvider` (trait) | `traits::stt_provider` | Async interface for speech-to-text — `transcribe`, `transcribe_stream` with cancellation support |
| `PatientContext` | `types::agent` | Patient-specific grounding data (medications, conditions, allergies) — **PHI, never log** |

## How It Works

### Error type design

`AppError` is a `thiserror`-derived enum with one variant per subsystem
(`Database`, `AiProvider`, `SttProvider`, `Audio`, `Export`, etc.) plus
structured variants for connectivity (`EndpointOffline`) and policy
violations (`InvalidEndpoint`). A custom `Serialize` impl produces a JSON
object with at least `kind` (stable variant name string) and `message`
(`Display` output). Structured variants add extra fields so the Svelte
frontend can render targeted UI — for example, `EndpointOffline` includes
`service`, `endpoint`, `reason`, and `provider_name` so the offline dialog
can show the exact provider and suggest fixes.

`ErrorContext` is a separate builder-pattern struct for structured logging
with severity, error codes, and timestamps. It's used alongside `AppError`
in the Tauri command layer — `AppError` propagates; `ErrorContext`
annotates.

### Provider traits

All provider traits are `Send + Sync` and use `#[async_trait]`. They
return `AppResult<T>` with subsystem-specific error variants on failure.
The `AiProvider` trait exposes three completion modes (batch, streaming,
tool-calling) that map directly to the OpenAI-compatible API surface used
by Ollama, LM Studio, and oMLX. `SttProvider` supports both buffer and
streaming transcription, with a `CancellationToken` for user-initiated
cancellation.

### Endpoint policy and preflight

The local-only constraint (PHI/HIPAA) is enforced in two layers:

1. **Static classification** (`endpoint_policy`) — classifies host strings
   without DNS lookups into `Loopback`, `LanRfc1918`, `Tailscale`,
   `Mdns`, `LinkLocal`, `Ula`, `Public`, or `Unknown`. Called at settings
   save to reject public endpoints (e.g. `api.openai.com`).

2. **Preflight probes** (`preflight`) — before expensive commands like SOAP
   generation, `preflight_for_command` inspects settings, builds probe
   URLs for the active provider, and runs short-timeout (3 s) GET requests
   in parallel. Loopback hosts are skipped entirely. Failed probes return
   `EndpointOffline` with a classified reason (`ConnectionRefused`,
   `Timeout`, `DnsFailure`, `TlsFailure`) so the UI can show a targeted
   dialog.

## Usage Examples

### Provider crate implementing a core trait

```rust
// crates/ai-providers/src/lib.rs
use medical_core::traits::AiProvider;
use medical_core::types::{CompletionRequest, CompletionResponse, ModelInfo};

pub struct OllamaProvider { /* ... */ }

#[async_trait]
impl AiProvider for OllamaProvider {
    fn name(&self) -> &str { "ollama" }
    async fn available_models(&self) -> AppResult<Vec<ModelInfo>> { /* ... */ }
    async fn complete(&self, req: CompletionRequest) -> AppResult<CompletionResponse> { /* ... */ }
    // ...
}
```

### Consumer crate using core error types

```rust
// crates/stt-providers/src/client.rs
use medical_core::error::{AppError, AppResult, ServiceKind};
use medical_core::preflight::classify_reqwest_error;

async fn call_stt_api(audio: &[f32]) -> AppResult<Transcript> {
    let resp = client.post(url).body(audio).send().await
        .map_err(|e| match classify_reqwest_error(&e) {
            Some(reason) => AppError::EndpointOffline {
                service: ServiceKind::RemoteStt,
                endpoint: url.to_string(),
                reason,
                provider_name: "Whisper STT".into(),
            },
            None => AppError::SttProvider(e.to_string()),
        })?;
    // ...
}
```

### Using domain types from a processing crate

```rust
// crates/processing/src/soap_generator/user_prompt.rs
use medical_core::types::PatientContext;

fn build_prompt(ctx: &PatientContext) -> String {
    let mut parts = Vec::new();
    if !ctx.medications.is_empty() {
        parts.push(format!("Current medications: {}", ctx.medications.join(", ")));
    }
    // ...
}
```

## Module Layout

| Module | Contents |
|---|---|
| `error` | `AppError`, `AppResult`, `ErrorSeverity`, `ErrorContext` |
| `endpoint_policy` | `EndpointKind`, `classify_endpoint`, `validate_local_endpoint` |
| `preflight` | `probe_endpoint`, `preflight_for_command`, `CommandKind` |
| `http_error_body` | `read_error_body` — bounded HTTP error body reader |
| `types::ai` | `CompletionRequest`, `CompletionResponse`, `Message`, `StreamChunk` |
| `types::stt` | `AudioData`, `SttConfig`, `Transcript`, `TranscriptSegment` |
| `types::tts` | `TtsConfig`, `VoiceInfo` |
| `types::agent` | `AgentContext`, `PatientContext`, `ToolDef`, `ToolOutput` |
| `types::recording` | `Recording`, `ProcessingStatus`, `RecordingSummary` |
| `types::processing` | `QueueTask`, `BatchProcessingOptions`, `ProcessingEvent` |
| `types::rag` | `RagResult`, `SearchConfig`, `DocumentChunk`, `GraphEntity` |
| `types::settings` | `AppConfig`, `SttMode`, `Theme`, `IcdVersion`, `SoapTemplate` |
| `types::vocabulary` | `VocabularyEntry`, `CorrectionResult` |
| `types::endpoint` | `RemoteEndpoint`, `http_url` |
| `types::letter_audience` | `LetterAudience` |
| `traits` | `AiProvider`, `SttProvider`, `TtsProvider`, `Agent`, `Tool`, `Exporter`, `TranslationProvider` |

## Edge Cases & Gotchas

### Error variant selection

When adding a new error case, resist the urge to use `AppError::Other`.
There's a variant for almost every subsystem. `Other` is a last resort —
it serializes with `kind: "Other"`, which gives the frontend no signal
for targeted UI. If you need a new subsystem, add a new variant.

### `AppError::EndpointOffline` vs. `AppError::AiProvider`

`EndpointOffline` is for **connectivity** failures — the server didn't
respond at all. `AiProvider` is for **API-level** failures — the server
responded but returned an error (bad API key, model not found, rate
limit). Preflight probes produce `EndpointOffline`; runtime API calls
produce `AiProvider`.

### `PatientContext` is PHI

The `PatientContext` struct contains medications, conditions, allergies,
and patient names. **Never log these fields** via `tracing::*` macros,
`println!`, or `eprintln!`. Log counts and IDs, never content. This is a
HIPAA constraint enforced by project policy.

### `AppConfig` uses `#[serde(default)]` everywhere

Every field in `AppConfig` has a serde default, so older config files
missing new fields deserialize successfully. When adding a new field,
always add a `#[serde(default = "default_fn_name")]` attribute and a
corresponding `fn default_fn_name() -> T` function. This is how the app
achieves forward-compatible settings migration.

### `AppConfig::migrate()` must be called after deserialization

The `migrate()` method corrects stale values (e.g. cloud provider names
from older versions). Call it immediately after deserializing the config
from disk — the db crate does this automatically.

### `RemoteEndpoint` redacts bearer tokens in `Debug`

The `Debug` impl replaces bearer tokens with `"<redacted>"` so they
never appear in `tracing::debug!(?endpoint, ...)` output. Do not add a
`#[derive(Debug)]` to `RemoteEndpoint` — it would override the manual
impl and leak tokens.

### Serialization contracts

`ServiceKind` and `OfflineReason` serialize as PascalCase
(`"AiProvider"`, `"ConnectionRefused"`). `Role` serializes as
`snake_case` (`"assistant"`). These are deliberate — the frontend
dispatches on these strings. Changing the serialization format is a
**breaking change** for the Tauri IPC contract.

### Trait objects must be `Send + Sync`

All provider traits require `Send + Sync` because provider instances are
shared across async tasks (stored in `Arc`, passed to Tauri commands).
If your provider implementation holds non-`Send` state, wrap it in a
`Mutex` or move it behind a channel.
