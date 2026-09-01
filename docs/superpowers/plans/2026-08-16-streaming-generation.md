# Streaming Generation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** All five generation commands stream via a shared `stream_to_completion` helper with idle-based timeouts (120s), a 1h hard cap, live token/tok-s progress in the UI, and reasoning-delta support (lengths only).

**Architecture:** The client's existing SSE path (`complete_stream`) gains `reasoning_content` mapping and a per-request 3600s timeout. A new pure src-tauri helper assembles the stream into a `CompletionResponse`, strips leading `<think>` blocks, enforces a per-chunk idle timeout, and reports throttled count-only stats through a callback that call sites wire to `generation-progress` events. Spec: `docs/superpowers/specs/2026-08-16-streaming-generation-design.md`.

**Tech Stack:** Rust (edition 2024), tokio-stream/async-stream, Svelte 5 runes, vitest.

**Execution:** worktree `.worktrees/streaming-generation`, branch `feat/streaming-generation`. Never commit to master directly.

---

### Task 1: `StreamChunk::ReasoningDelta` + wire `reasoning_content`

**Files:**
- Modify: `crates/core/src/types/ai.rs` (StreamChunk enum, ~line 167)
- Modify: `crates/ai-providers/src/openai_compat/wire.rs` (ChatDelta struct + tests)

- [ ] **Step 1: Failing wire test** — in `wire.rs` tests mod, extend the `deserialize_streaming_delta` fixture or add:

```rust
    #[test]
    fn deserialize_streaming_reasoning_delta() {
        let raw = r#"{"choices":[{"delta":{"reasoning_content":"thinking hard"}}]}"#;
        let resp: ChatResponse = serde_json::from_str(raw).expect("parse");
        let delta = resp.choices[0].delta.as_ref().expect("delta present");
        assert_eq!(delta.reasoning_content.as_deref(), Some("thinking hard"));
        assert_eq!(delta.content, None);
    }
```

Run `cargo test -p medical-ai-providers --lib wire` → COMPILE ERROR (no field).

- [ ] **Step 2: Implement.** In `wire.rs`, add to `ChatDelta` (next to `content`):

```rust
    /// Reasoning/"thinking" delta (LM Studio and friends). Used ONLY to
    /// compute a length for progress counting — never surfaced as text.
    #[serde(default)]
    pub reasoning_content: Option<String>,
```

In `crates/core/src/types/ai.rs`, add to `StreamChunk`:

```rust
    /// A reasoning/"thinking" delta, reduced to its byte length. The
    /// reasoning text itself never crosses the provider boundary — lengths
    /// and counts are safe to log/emit (AGENTS.md), content is not.
    ReasoningDelta {
        /// Byte length of the reasoning delta text.
        len: usize,
    },
```

- [ ] **Step 3: Fix exhaustive matches.** `grep -rn "StreamChunk::Delta\|StreamChunk::Usage" crates src-tauri/src --include="*.rs" | grep -v test` — every `match` on StreamChunk needs a new arm (ignore unless spec'd): `StreamChunk::ReasoningDelta { .. } => {}` (chat.rs worker, agents orchestrator if it matches, anywhere else). Follow compiler errors.
- [ ] **Step 4:** `cargo test -p medical-core --lib && cargo test -p medical-ai-providers --lib` → PASS. `cargo clippy -p medical-core -p medical-ai-providers --all-targets -- -D warnings`; `cargo fmt --all`.
- [ ] **Step 5: Commit** `feat(core): add ReasoningDelta stream chunk carrying length only`

---

### Task 2: Stream-request timeout + reasoning mapping in `complete_stream`

**Files:**
- Modify: `crates/ai-providers/src/openai_compat/client.rs` (post_json_with_timeout)
- Modify: `crates/ai-providers/src/openai_compat/methods.rs` (complete_stream)

- [ ] **Step 1: Failing test.** In `methods.rs` tests (or wire.rs if methods has no test mod — put it in `client.rs` tests if needed; pick ONE location, `methods.rs` preferred). Extract-first approach: the per-event mapping closure in `complete_stream` maps `ChatResponse → Vec<AppResult<StreamChunk>>`. Refactor it into a testable pure fn at the bottom of `methods.rs`:

```rust
/// Map one SSE `ChatResponse` event into stream chunks (pure; testable).
fn map_chat_event(resp: &ChatResponse) -> Vec<AppResult<StreamChunk>> {
    // (body moved verbatim from the existing closure in complete_stream,
    //  plus the new reasoning arm below)
}
```

Then `complete_stream` uses `.map(|item| item.map(map_chat_event).unwrap_or_else(|e| vec![Err(AppError::ai_provider(e))]))` — preserve the existing malformed-JSON error path exactly (warn! + propagate). Test:

```rust
    #[test]
    fn maps_reasoning_delta_to_length_only() {
        let raw = r#"{"choices":[{"delta":{"reasoning_content":"abc"}}]}"#;
        let resp: ChatResponse = serde_json::from_str(raw).unwrap();
        let chunks = map_chat_event(&resp);
        assert!(matches!(chunks[0], Ok(StreamChunk::ReasoningDelta { len: 3 })));
    }
```

Inside the mapping (after the content-delta block):

```rust
    if let Some(reasoning) = &delta.reasoning_content
        && !reasoning.is_empty()
    {
        out.push(Ok(StreamChunk::ReasoningDelta {
            len: reasoning.len(),
        }));
    }
```

Run → fails to compile (no `map_chat_event` / no reasoning field on delta if Task 1 not landed — Task 1 is a dependency).

- [ ] **Step 2: Per-request timeout.** In `client.rs` next to `post_json`:

```rust
    /// Like [`Self::post_json`], but overrides the client-level total
    /// timeout for this request. Streaming requests need a generous hard
    /// cap instead of the short non-streamed budget.
    pub(super) fn post_json_with_timeout<T: serde::Serialize>(
        &self,
        url: &str,
        body: &T,
        timeout: std::time::Duration,
    ) -> reqwest::RequestBuilder {
        self.post_json(url, body).timeout(timeout)
    }
```

In `methods.rs` `complete_stream`, add at the top of the fn:

```rust
    /// Hard ceiling for a single streamed generation. Long enough for a
    /// reasoning model to think + write; the meaningful limit is the
    /// consumer-side idle timeout (no data for N seconds).
    const STREAM_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);
```

and change the send line to use `post_json_with_timeout(&url, &body, STREAM_TOTAL_TIMEOUT)`.

- [ ] **Step 3:** `cargo test -p medical-ai-providers --lib` PASS; clippy; fmt. **Commit** `feat(ai-providers): stream requests get 1h cap + reasoning-length mapping`

---

### Task 3: Mock providers stream

**Files:**
- Modify: `src-tauri/src/commands/generation/test_helpers.rs`

- [ ] **Step 1:** `MockCompletionProvider::complete_stream` currently returns an error. Replace with a replay of the non-streamed shape so the five wiring tests keep passing once call sites stream:

```rust
    async fn complete_stream(
        &self,
        _request: medical_core::types::CompletionRequest,
    ) -> AppResult<
        Box<
            dyn futures_util::Stream<Item = AppResult<medical_core::types::StreamChunk>>
                + Send
                + Unpin,
        >,
    > {
        let chunks = vec![
            Ok(medical_core::types::StreamChunk::Delta {
                text: self.content.clone(),
            }),
            Ok(medical_core::types::StreamChunk::Usage(self.usage.clone())),
            Ok(medical_core::types::StreamChunk::Done),
        ];
        Ok(Box::pin(tokio_stream::iter(chunks)))
    }
```

Add `tokio-stream` to src-tauri `[dev-dependencies]` if absent (`tokio-stream = { workspace = true }` — verify workspace has it).

Also add a scripted-stream mock for Task 4's unit tests (same file):

```rust
/// Stream-provider mock whose SSE stream replays `chunks`, then optionally
/// stalls forever after the last chunk (for idle-timeout tests).
pub(super) struct ScriptedStreamProvider {
    pub name: &'static str,
    pub chunks: Vec<AppResult<medical_core::types::StreamChunk>>,
    pub stall_after_last: bool,
}

#[async_trait::async_trait]
impl medical_core::traits::AiProvider for ScriptedStreamProvider {
    fn name(&self) -> &str { self.name }
    async fn available_models(&self) -> AppResult<Vec<medical_core::types::ModelInfo>> {
        Ok(Vec::new())
    }
    async fn complete(
        &self,
        _request: medical_core::types::CompletionRequest,
    ) -> AppResult<medical_core::types::CompletionResponse> {
        Err(AppError::ai_provider("scripted provider is stream-only".to_string()))
    }
    async fn complete_stream(
        &self,
        _request: medical_core::types::CompletionRequest,
    ) -> AppResult<
        Box<
            dyn futures_util::Stream<Item = AppResult<medical_core::types::StreamChunk>>
                + Send
                + Unpin,
        >,
    > {
        let chunks = self.chunks.clone();
        let stall = self.stall_after_last;
        let stream = async_stream::stream! {
            for c in chunks {
                yield c;
            }
            if stall {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        };
        Ok(Box::pin(stream))
    }
    async fn complete_with_tools(
        &self,
        _request: medical_core::types::CompletionRequest,
        _tools: Vec<medical_core::types::ToolDef>,
    ) -> AppResult<medical_core::types::ToolCompletionResponse> {
        Err(AppError::ai_provider("scripted provider does not support tools".to_string()))
    }
}
```

(`async-stream` is already a src-tauri dependency; the stream must be `Unpin` — `async_stream::stream!` boxed via `Box::pin` satisfies the trait bound.)

- [ ] **Step 2:** `cargo test -p rust-medical-assistant --lib stats_tests` → still 4/4 (mock now streams; call sites still non-streamed — complete() unchanged). `--no-run` compile check + clippy + fmt.
- [ ] **Step 3: Commit** `test(tauri): mock providers with scripted streaming`

---

### Task 4: `stream_to_completion` helper

**Files:**
- Create: `src-tauri/src/commands/generation/stream.rs`
- Modify: `src-tauri/src/commands/generation/mod.rs` (`pub(super) mod stream;` + `GenerationProgressStats`)

- [ ] **Step 1: Types in `mod.rs`** (near GenerationProgress):

```rust
/// Live throughput stats for an in-flight streaming generation.
/// Counts and durations only — never content (AGENTS.md PHI rule).
#[derive(Debug, Clone, Copy, Serialize)]
pub(super) struct GenerationProgressStats {
    /// Approximate tokens streamed so far (one SSE delta ≈ one token;
    /// the persisted generation stat remains exact via the usage chunk).
    pub tokens: u64,
    /// Ms since the first streamed chunk.
    pub elapsed_ms: u64,
    /// Tokens/sec since the first chunk.
    pub tokens_per_second: f64,
}
```

Add `progress: Option<GenerationProgressStats>` to `GenerationProgress` with `#[serde(skip_serializing_if = "Option::is_none")]`; update ALL existing `GenerationProgress { .. }` literals (grep `grep -rn "GenerationProgress {" src-tauri/src`) adding `progress: None`.

- [ ] **Step 2: Failing tests** in `stream.rs` (write the file with tests first; implementation second — red = compile error, then behavior):

```rust
//! Shared streaming driver for the generation commands: consumes the
//! provider's SSE stream, assembles the completion, enforces an idle
//! timeout, and reports throttled count-only progress via a callback.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::traits::AiProvider;
use medical_core::types::{CompletionRequest, CompletionResponse, StreamChunk, UsageInfo};

use super::test_helpers::ScriptedStreamProvider;
use super::GenerationProgressStats;

/// No data from the model for this long → stalled. Generous enough for
/// reasoning pauses; the request itself is capped at 1h by the provider
/// layer (see complete_stream's STREAM_TOTAL_TIMEOUT).
pub(super) const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Minimum spacing between on_progress callbacks.
const PROGRESS_THROTTLE: std::time::Duration = std::time::Duration::from_millis(500);

pub(super) async fn stream_to_completion(
    provider: &Arc<dyn AiProvider>,
    mut on_progress: impl FnMut(&GenerationProgressStats),
    request: CompletionRequest,
) -> AppResult<CompletionResponse> {
    stream_with_idle_timeout(provider, &mut on_progress, request, STREAM_IDLE_TIMEOUT).await
}

pub(super) async fn stream_with_idle_timeout(
    provider: &Arc<dyn AiProvider>,
    on_progress: &mut dyn FnMut(&GenerationProgressStats),
    request: CompletionRequest,
    idle: std::time::Duration,
) -> AppResult<CompletionResponse> {
    use futures_util::StreamExt;

    let model = request.model.clone();
    let mut stream = provider.complete_stream(request).await?;

    let mut content = String::new();
    let mut usage = UsageInfo::default();
    let mut tokens: u64 = 0;
    let mut first_chunk_at: Option<std::time::Instant> = None;
    let mut last_emit: Option<std::time::Instant> = None;

    loop {
        let chunk = match tokio::time::timeout(idle, stream.next()).await {
            Err(_) => {
                return Err(AppError::ai_provider(format!(
                    "generation stalled — no data from the model for {}s",
                    idle.as_secs()
                )));
            }
            Ok(None) => break, // stream ended (with or without Done)
            Ok(Some(result)) => result?,
        };
        let now = std::time::Instant::now();
        if first_chunk_at.is_none() {
            first_chunk_at = Some(now);
        }
        match chunk {
            StreamChunk::Delta { text } => {
                content.push_str(&text);
                tokens += 1;
            }
            StreamChunk::ReasoningDelta { .. } => {
                tokens += 1; // length only; reasoning text stays in the provider layer
            }
            StreamChunk::Usage(u) => usage = u,
            StreamChunk::Done => break,
            StreamChunk::ToolCallDelta { .. } => {}
        }
        let start = first_chunk_at.unwrap_or(now);
        let elapsed = now.duration_since(start);
        let due = last_emit.is_none_or(|t| now.duration_since(t) >= PROGRESS_THROTTLE);
        if due {
            on_progress(&GenerationProgressStats {
                tokens,
                elapsed_ms: elapsed.as_millis() as u64,
                tokens_per_second: tokens as f64 / elapsed.as_secs_f64().max(f64::EPSILON),
            });
            last_emit = Some(now);
        }
    }

    Ok(CompletionResponse {
        content: strip_leading_think_block(&content).to_string(),
        model,
        usage,
        tool_calls: vec![],
    })
}

/// Strip a leading `<think>…</think>` block (some providers, notably Ollama
/// with reasoning models, inline reasoning into content). If the block is
/// never closed, everything is reasoning — return the empty remainder.
pub(super) fn strip_leading_think_block(content: &str) -> &str {
    let trimmed = content.trim_start();
    let Some(rest) = trimmed.strip_prefix("<think>") else {
        return content;
    };
    match rest.find("</think>") {
        Some(end) => rest[end + "</think>".len()..].trim_start(),
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::{Message, MessageContent, Role};

    fn req() -> CompletionRequest {
        CompletionRequest {
            model: "llama3".into(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("hi".into()),
                tool_calls: vec![],
            }],
            temperature: None,
            max_tokens: None,
            system_prompt: None,
        }
    }

    fn provider(chunks: Vec<AppResult<StreamChunk>>, stall: bool) -> Arc<dyn AiProvider> {
        Arc::new(ScriptedStreamProvider {
            name: "scripted",
            chunks,
            stall_after_last: stall,
        })
    }

    #[tokio::test]
    async fn assembles_deltas_and_usage() {
        let p = provider(
            vec![
                Ok(StreamChunk::Delta { text: "**S:** ".into() }),
                Ok(StreamChunk::ReasoningDelta { len: 900 }),
                Ok(StreamChunk::Delta { text: "headache".into() }),
                Ok(StreamChunk::Usage(UsageInfo {
                    prompt_tokens: 10,
                    completion_tokens: 3,
                    total_tokens: 13,
                })),
                Ok(StreamChunk::Done),
            ],
            false,
        );
        let mut seen = Vec::new();
        let resp = stream_to_completion(&p, |s| seen.push(*s), req()).await.unwrap();
        assert_eq!(resp.content, "**S:** headache");
        assert_eq!(resp.model, "llama3");
        assert_eq!(resp.usage.completion_tokens, 3);
        assert!(!seen.is_empty());
        assert!(seen.last().unwrap().tokens >= 3);
    }

    #[tokio::test]
    async fn idle_timeout_errors_when_stalled() {
        let p = provider(
            vec![Ok(StreamChunk::Delta { text: "x".into() })],
            true, // stall forever after the chunk
        );
        let err = stream_with_idle_timeout(&p, &mut |_| {}, req(), std::time::Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("stalled"));
    }

    #[tokio::test]
    async fn strips_leading_think_block() {
        let p = provider(
            vec![
                Ok(StreamChunk::Delta { text: "<think>secret reasoning</think>\n\nSOAP".into() }),
                Ok(StreamChunk::Done),
            ],
            false,
        );
        let resp = stream_to_completion(&p, |_| {}, req()).await.unwrap();
        assert_eq!(resp.content, "SOAP");
    }

    #[tokio::test]
    async fn unclosed_think_block_yields_empty() {
        assert_eq!(strip_leading_think_block("  <think> forever"), "");
        assert_eq!(strip_leading_think_block("no tags"), "no tags");
    }

    #[tokio::test]
    async fn missing_usage_records_default() {
        let p = provider(
            vec![Ok(StreamChunk::Delta { text: "ok".into() }), Ok(StreamChunk::Done)],
            false,
        );
        let resp = stream_to_completion(&p, |_| {}, req()).await.unwrap();
        assert_eq!(resp.usage.completion_tokens, 0);
    }
}
```

Write tests + skeleton, confirm red (compile error), implement, `cargo test -p rust-medical-assistant --lib stream` → PASS (5 tests).

- [ ] **Step 3:** clippy + fmt. **Commit** `feat(tauri): stream_to_completion helper with idle timeout + progress`

---

### Task 5: Wire all five call sites

**Files:**
- Modify: `soap.rs`, `helpers.rs`, `referral.rs`, `letter.rs`, `synopsis.rs`, `peer_discussion.rs` (+ `pipeline.rs` if it calls the inner fn — check)

- [ ] **Step 1: Signature threading.** Inner fns gain `app: Option<&tauri::AppHandle>` as the 2nd param: `generate_soap_inner(state, app, recording_id, …)`, `generate_from_soap(…, app, …)` (param order: after `state`), `generate_synopsis_inner(state, app, recording_id)`, `generate_peer_discussion_inner(state, app, …)`. Outer commands pass `Some(&app)`; the pipeline (`process_recording`) passes `Some(app)` (it owns an AppHandle — verify); ALL existing test call sites pass `None` (grep every call). `run_generation_command` closures capture `app.clone()` (tauri::AppHandle is Clone) when needed.

- [ ] **Step 2: Swap the call.** In each of the five, replace `provider.complete(request)` with:

```rust
    let generation_start = std::time::Instant::now();
    let response = crate::commands::generation::stream::stream_to_completion(
        &provider,
        |stats| {
            if let Some(app) = app {
                let _ = app.emit(
                    "generation-progress",
                    GenerationProgress {
                        doc_type: "soap".into(), // per call site
                        status: "generating".into(),
                        recording_id: recording_id.to_string(),
                        progress: Some(stats),
                    },
                );
            }
        },
        request,
    )
    .await
    .map_err(|e| match e {
        AppError::EndpointOffline { .. } => e,
        _ => AppError::ai_provider(format!(
            "AI completion failed: {}",
            crate::commands::unwrap_app_error_message(e)
        )),
    })?;
    let generation_elapsed = generation_start.elapsed();
```

(Adapt per site: `generate_from_soap` uses `stats_key` + `recording.id`; synopsis/peer use their doc strings + `recording_id`. In helpers.rs the closure captures `stats_key`/`doc label` — mind closure borrows; `app: Option<&AppHandle>` is Copy.)

- [ ] **Step 3:** `cargo test -p rust-medical-assistant --lib` (full — stats tests 4/4 via streaming mock, preflight 10/10) + clippy + fmt. **Commit** `feat(tauri): all five generation commands stream with live progress`

---

### Task 6: Frontend progress display

**Files:**
- Modify: `src/lib/types/index.ts` (GenerationProgressStats)
- Modify: `src/lib/stores/generation.svelte.ts` (+progress state, setProgress)
- Modify: `src/App.svelte` (generation-progress listener passes payload through)
- Modify: `src/lib/components/GenerateControls.svelte` (progressText derivation — grep `progressText` to find the exact construction site)
- Modify: `src/lib/components/PipelineStatus.svelte` (counter during generating_soap — read generation store's progress; no pipeline.rs changes)
- Test: `src/lib/utils/format.test.ts` or a new `generationProgress` util test

- [ ] **Step 1:** Type:

```ts
/** Mirrors Rust `GenerationProgressStats` — counts/durations only, no content. */
export interface GenerationProgressStats {
  tokens: number;
  elapsed_ms: number;
  tokens_per_second: number;
}
```

generation store: add `progress: GenerationProgressStats | null` to state (null on setBusy/clear), plus `setProgress(p: GenerationProgressStats)`; App.svelte `generation-progress` handler forwards `payload.progress ?? null` when status is `generating`, clears otherwise.
- [ ] **Step 2:** progressText: where GenerateControls builds the generating text, when `progress` present show `` `Generating… ${progress.tokens} tokens · ${formatTokensPerSecond(progress.tokens_per_second)}` `` (reuse existing util). Extract `export function generationProgressText(p: GenerationProgressStats): string` into `src/lib/utils/generationStats.ts` + unit tests (with/without data). PipelineStatus: when `stage === 'generating_soap'` and generation-store progress exists, show the same `generationProgressText` output.
- [ ] **Step 3:** `npx vitest run` + `npm run check` green (update makeSummary-style factories if type errors appear). **Commit** `feat(web): live token/tok-s progress during generation`

---

### Task 7: Gates + final review

- [ ] `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --lib` (use `-- --skip probe_endpoint` + isolated probe run if the wiremock startup flake strikes); `npx vitest run`; `npm run check`; `npm run lint`.
- [ ] Final whole-branch reviewer subagent (base = branch point).
- [ ] Manual smoke (optional): `npm run tauri dev` + LM Studio qwen3.8 — generate SOAP, watch live counter, confirm badge after.
