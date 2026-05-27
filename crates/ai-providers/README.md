# medical-ai-providers

Local AI provider integration crate for FerriScribe — a Tauri desktop medical
transcription app. This crate provides Ollama and LM Studio connectivity via
the OpenAI-compatible chat-completions wire protocol, with streaming (SSE),
tool calling, automatic retry, and LAN/Tailscale endpoint resolution.

> **Hard constraint:** only local and LAN-accessible AI providers are supported.
> Hosted APIs (OpenAI, Anthropic, Google, etc.) are intentionally **not**
> supported. This is a PHI/HIPAA requirement — patient data must never leave
> the local network. The endpoint-policy validation layer in `medical-core`
> rejects public URLs by default; see the `allow_public` flag on provider
> constructors.

## How It Fits

```
┌──────────────┐      ┌──────────────────────┐      ┌──────────────────────┐
│  src-tauri   │─────>│  medical-ai-providers │─────>│  Ollama / LM Studio  │
│  (commands)  │      │  (this crate)         │ HTTP │  (local AI servers)  │
└──────────────┘      └──────────────────────┘      └──────────────────────┘
                              │
                              ▼
                       ┌──────────────┐
                       │ medical-core │
                       │  (AiProvider │
                       │    trait)    │
                       └──────────────┘
```

- **Depends on:** `medical-core` — for the `AiProvider` trait, request/response
  types (`CompletionRequest`, `CompletionResponse`, `StreamChunk`, etc.),
  error types, and endpoint-policy validation.
- **Used by:** `src-tauri` — Tauri commands invoke `AiProvider` methods for
  chat completions, SOAP note generation, embeddings, referral letters, and
  any other AI-driven workflow.
- **Sibling crates:** `agents`, `processing`, `translation`, `rag` all depend
  on the `AiProvider` trait from `medical-core` and receive provider instances
  from this crate via the `ProviderRegistry`.

## Key Types

### Provider Registry

| Type | Purpose |
|------|---------|
| `ProviderRegistry` | Holds registered `Arc<dyn AiProvider>` instances keyed by name. Tracks the active provider. Used by `src-tauri` to switch between Ollama and LM Studio at runtime. |

### Providers

| Type | Purpose |
|------|---------|
| `OllamaProvider` | Wraps `OpenAiCompatibleClient` pointed at an Ollama server (default `http://localhost:11434/v1`). Supports `RemoteEndpoint` for LAN/Tailscale resolution. |
| `LmStudioProvider` | Wraps `OpenAiCompatibleClient` pointed at an LM Studio server (default `http://localhost:1234/v1`). Same `RemoteEndpoint` support. |

Both implement `medical_core::traits::AiProvider`.

### OpenAI-Compatible Client

| Type | Purpose |
|------|---------|
| `OpenAiCompatibleClient` | Generic HTTP client for any endpoint implementing the OpenAI chat-completions protocol. Handles request building, response parsing, streaming, and tool calls. Shared by both providers. |

### HTTP Infrastructure

| Type | Purpose |
|------|---------|
| `RetryConfig` | Exponential-backoff configuration with jitter. Constructed from `AppConfig` settings or manually. |
| `CircuitBreaker` | Simple failure-count circuit breaker (available but not currently wired into `send_with_retry`). |
| `RetryDecision` | Classification of HTTP outcomes: `Success`, `Permanent`, `Transient`, `TransientWithDelay`. |

### SSE Streaming

| Function | Purpose |
|----------|---------|
| `parse_sse_response` | Converts a streaming `reqwest::Response` into a `Stream<Item = Result<String, String>>`, filtering out empty lines and `[DONE]` sentinels. |

## How It Works

### Request Flow

```
complete(request)
    │
    ├── sync_client_url()         ← resolve LAN/Tailscale or use static URL
    │       │
    │       └── current_base_url()   ← 30s cached resolution
    │
    ├── client.complete(request)
    │       │
    │       ├── build_request()      ← core types → OpenAI wire format
    │       │
    │       ├── send_with_retry()    ← exponential backoff, Retry-After
    │       │       │
    │       │       └── POST {base_url}/chat/completions
    │       │
    │       └── parse_response()     ← OpenAI wire format → core types
    │
    └── CompletionResponse
```

### Streaming Flow

```
complete_stream(request)
    │
    ├── sync_client_url()
    │
    ├── client.complete_stream(request)
    │       │
    │       ├── POST with stream=true, stream_options.include_usage=true
    │       │
    │       ├── parse_sse_response()     ← SSE event stream → data lines
    │       │
    │       └── map each data line:
    │               ├── ChatDelta.content    → StreamChunk::Delta { text }
    │               ├── ChatDelta.tool_calls → StreamChunk::ToolCallDelta { ... }
    │               ├── usage object         → StreamChunk::Usage(...)
    │               └── (implicit)           → StreamChunk::Done
    │
    └── Box<dyn Stream<Item = AppResult<StreamChunk>>>
```

### Endpoint Resolution (LAN/Tailscale)

Both providers support `RemoteEndpoint` for multi-network deployments — e.g.,
a clinic where the AI server runs on a LAN machine but the clinician's laptop
connects via Tailscale when working remotely.

1. If a `RemoteEndpoint` is configured, `current_base_url()` probes LAN first,
   then Tailscale, using `RemoteEndpoint::resolve_base_url()`.
2. The resolved URL is cached for 30 seconds (`CACHE_TTL`).
3. `set_endpoint()` replaces the endpoint, clears the cache, and propagates
   the new bearer token into the inner HTTP client.
4. If no endpoint is configured, the static `base_url` from construction is used.

### How Ollama and LM Studio Differ

| Aspect | Ollama | LM Studio |
|--------|--------|-----------|
| Default port | 11434 | 1234 |
| Base URL suffix | `/v1` | `/v1` |
| Model listing | `/v1/models` (OpenAI-compat) | `/v1/models` (OpenAI-compat) |
| Fallback model | `llama3` | `default` |
| Provider name | `"ollama"` | `"lmstudio"` |

Both use the identical `OpenAiCompatibleClient` under the hood. The providers
differ only in defaults, naming, and the endpoint-policy field name
(`ollama_host` vs `lmstudio_host`).

## Examples

### Sending a Chat Completion

```rust
use medical_ai_providers::ollama::OllamaProvider;
use medical_ai_providers::http_client::RetryConfig;
use medical_core::traits::AiProvider;
use medical_core::types::{CompletionRequest, Message, MessageContent, Role};

let provider = OllamaProvider::new(None, false, None, RetryConfig::default())?;

let request = CompletionRequest {
    model: "llama3".into(),
    messages: vec![Message {
        role: Role::User,
        content: MessageContent::Text("Summarize this consultation.".into()),
        tool_calls: vec![],
    }],
    temperature: Some(0.3),
    max_tokens: Some(512),
    system_prompt: Some("You are a medical scribe.".into()),
};

let response = provider.complete(request).await?;
println!("Model: {}, tokens: {}", response.model, response.usage.total_tokens);
```

### Streaming a Response

```rust
use futures_util::StreamExt;
use medical_core::types::StreamChunk;

let mut stream = provider.complete_stream(request).await?;
while let Some(chunk) = stream.next().await {
    match chunk? {
        StreamChunk::Delta { text } => print!("{text}"),
        StreamChunk::Usage(usage) => eprintln!("\n[tokens: {}]", usage.total_tokens),
        StreamChunk::Done => break,
        StreamChunk::ToolCallDelta { .. } => { /* tool call streaming */ }
    }
}
```

### Using the Provider Registry

```rust
use medical_ai_providers::ProviderRegistry;
use std::sync::Arc;

let mut registry = ProviderRegistry::new();
registry.register(Arc::new(ollama_provider));
registry.register(Arc::new(lmstudio_provider));

// First registered provider becomes active by default.
registry.set_active("lmstudio");

if let Some(provider) = registry.active() {
    let models = provider.available_models().await?;
}
```

## Retry and Error Handling

### Retry Policy

`send_with_retry` wraps every HTTP call with exponential backoff:

- **Transient** (retryable): HTTP 408, 429, 500, 502, 503, 504, plus
  transport-level timeouts and generic request errors.
- **Permanent** (non-retryable): 4xx (except 408/429), connection-refused,
  body/decode errors.
- **Retry-After:** honored when the server sends it (delta-seconds format only;
  HTTP-date is not supported). Capped at `RetryConfig::max_delay`.
- **Jitter:** ±25% random jitter applied to each backoff delay.
- **Default policy:** 3 retries, 1s initial delay, 2× backoff, 30s max delay.
  Configurable via `AppConfig.auto_retry_failed` and `AppConfig.max_retry_attempts`.

### Error Classification

Errors are mapped to two `AppError` variants:

| Variant | When |
|---------|------|
| `EndpointOffline { service, endpoint, reason, provider_name }` | Connectivity issues — connection refused, timeout, DNS failure. The `OfflineReason` enum distinguishes `ConnectionRefused`, `Timeout`, `DnsFailure`, etc. |
| `AiProvider(String)` | Application-layer errors — HTTP 4xx, JSON parse failures, empty responses, context window exceeded. |

## Gotchas

### SSE Parsing Edge Cases

- The `[DONE]` sentinel is filtered by `parse_sse_response`. If a provider
  sends a different terminator, the stream will attempt to JSON-parse it and
  silently drop it (the `serde_json::from_str` in `complete_stream` returns
  `Err` which maps to an empty `Vec`).
- Some providers send empty `data:` lines between events. These are filtered
  by the SSE parser (`data.is_empty()` check).
- Usage information arrives in a **separate** SSE event from the final content
  delta. The stream emits `StreamChunk::Usage` followed by `StreamChunk::Done`
  when it sees the `usage` field.

### No Hosted APIs

The `allow_public: bool` parameter on provider constructors defaults to
`false`, which causes `endpoint_policy::validate_url` to reject any URL that
isn't a private/LAN address. Passing `true` disables this check — but doing
so for production use violates the PHI constraint in CLAUDE.md. The parameter
exists for testing and development only.

### Bearer Token Lifecycle

When `set_endpoint()` is called with a new `RemoteEndpoint`, the bearer token
is propagated to the inner `OpenAiCompatibleClient`. This is critical for
in-session Unpair → Pair flows: without the propagation, the inner client
would still carry the old (revoked) token, causing 401 errors.

### Mutex Contention on the Client

Both providers wrap `OpenAiCompatibleClient` in a `tokio::sync::Mutex`. The
lock is held for the **entire duration** of each request (including the HTTP
round-trip). This means concurrent requests to the same provider are
serialized. For most clinical workflows this is acceptable — AI calls are
infrequent and latency-tolerant.

### Provider-Specific Quirks

- **Ollama** may return an empty `choices` array when the model's context
  window is exceeded. The `complete()` method detects this (empty content +
  `finish_reason: "length"`) and returns a descriptive error suggesting a
  larger-context model.
- **LM Studio** sometimes omits the `model` field in responses. The parser
  falls back to the model name from the request.
- Both providers' `available_models()` falls back to a single hardcoded model
  entry if the `/v1/models` endpoint fails or returns empty — this ensures
  the UI always has at least one model to display.

## Module Structure

```
src/
├── lib.rs              — ProviderRegistry + module re-exports
├── http_client.rs      — RetryConfig, CircuitBreaker, send_with_retry, classify_*
├── sse.rs              — parse_sse_response (SSE stream → data lines)
├── ollama.rs           — OllamaProvider (AiProvider impl)
├── lmstudio.rs         — LmStudioProvider (AiProvider impl)
└── openai_compat/
    ├── mod.rs          — module declarations, re-exports OpenAiCompatibleClient
    ├── client.rs       — OpenAiCompatibleClient struct + constructors + message conversion
    ├── methods.rs      — list_models, complete, complete_stream, complete_with_tools
    └── wire.rs         — private serde types for the OpenAI wire protocol
```
