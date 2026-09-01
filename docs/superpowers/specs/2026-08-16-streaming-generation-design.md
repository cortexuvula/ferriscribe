# Streaming generation — Design

**Date:** 2026-08-16
**Status:** Approved (approach A, subagent execution)

## Problem

Generation commands use one non-streamed `provider.complete()` request. A
provider-level *total* timeout (now 900s) is the only ceiling, so a reasoning
model that thinks longer than the budget fails even though it would have
finished, and the user sees no progress while the model thinks. The client
already has a working SSE streaming path (`complete_stream`, used by chat)
that generation does not use.

## Goals

- Generation cannot hit a wall-clock cliff mid-generation: timeouts become
  **idle-based** (no data for 120s), with a 1-hour hard cap per request.
- Live progress while generating: token count and tok/s, visible in the
  Generate tab and the record pipeline's "generating SOAP" stage.
- All five generation commands stream (SOAP, referral, letter, synopsis,
  peer discussion) through one shared helper; downstream behavior
  (post-processing, `generation_stats` persistence, events on
  start/complete/fail) is unchanged.
- Reasoning ("thinking") models: reasoning deltas keep progress and the idle
  timer alive, while the reasoning text itself never leaves the provider
  layer (lengths only — PHI constraint enforced by type).

## Approach

**A (chosen): stream-to-completion helper.** A new src-tauri helper consumes
`complete_stream`, assembles the content, emits throttled count-only progress
events, and returns an assembled `CompletionResponse`. Call sites swap
`provider.complete(request)` for the helper; nothing downstream changes.

**B (rejected): longer non-streamed timeouts** — v0.46.1 already; no progress.
**C (rejected): pipe raw text to the UI** — larger PHI surface, no added value.

## Design

### 1. Provider layer (`medical-core` + `medical-ai-providers`)

- `StreamChunk` gains `ReasoningDelta { len: usize }` — **length only**; the
  reasoning text cannot be logged, emitted, or stored downstream.
- Wire `ChatDelta` gains `reasoning_content: Option<String>` (serde-tolerant,
  ignored by providers that don't send it). `complete_stream` maps a present,
  non-empty `reasoning_content` delta to `ReasoningDelta { len: text.len() }`.
- The streaming request sets a **per-request total timeout of 3600s** via
  reqwest `RequestBuilder::timeout` (supported in reqwest 0.13.4), overriding
  the client's 900s default for stream requests only. Non-streamed requests
  keep client defaults. The OpenAI-compat client grows a
  `post_json_with_timeout` helper; `complete_stream` uses it.
- Non-streamed `complete()` is unchanged (chat/tools paths unaffected).

### 2. Generation stream helper (`src-tauri/src/commands/generation/stream.rs`)

```rust
pub(super) async fn stream_to_completion(
    provider: &Arc<dyn AiProvider>,
    mut on_progress: impl FnMut(&GenerationProgressStats),
    request: CompletionRequest,
) -> AppResult<CompletionResponse>
```

`on_progress` is invoked at most every 500ms with live counts; call sites
close over `Option<&tauri::AppHandle>` and emit the `generation-progress`
event (no-op when `None`, e.g. in tests). This keeps the helper pure and
unit-testable with a collector closure.

Behavior:
- Consume chunks: `Delta` → append text; `ReasoningDelta` → count only;
  `Usage(u)` → remember; `Done` → finish. `ToolCallDelta` → ignored.
- **Idle timeout:** each `stream.next().await` is wrapped in
  `tokio::time::timeout(IDLE, …)` with `IDLE = 120s`; expiry →
  `AppError::ai_provider("generation stalled — no data from the model for
  120s…")`. (Injectable const for tests.)
- **Live stats:** tokens ≈ number of delta events (providers emit ~1 token
  per SSE event; documented approximation — the persisted stat stays exact
  via the usage chunk). tok/s = tokens since first event / elapsed since
  first event.
- **Progress events:** throttled to ≥500ms, `generation-progress` with
  status `"generating"` and optional `progress` payload (below). Emitted
  only while streaming; counts/durations only — never text. `app == None`
  (tests) → no events.
- **Inline-thinking hardening:** strip a leading `<think>…</think>` block
  (with surrounding whitespace) from the assembled content before returning
  — Ollama-style models inline reasoning into content; one strip point
  covers all five commands.
- **Return:** `CompletionResponse { content, model: request.model.clone(),
  usage, tool_calls: vec![] }`. Missing terminal usage → `usage:
  UsageInfo::default()` (stat recording then records nothing — existing
  semantics).
- Elapsed for stat recording: the whole stream duration (callers time the
  helper exactly as they timed `complete`).

### 3. Call-site integration

All five generation paths replace `provider.complete(request)` with
`stream_to_completion(...)`:
- `soap.rs::generate_soap_inner` (doc_type `"soap"`)
- `helpers.rs::generate_from_soap` (`stats_key`; gains an
  `Option<&AppHandle>` param threaded from referral/letter commands)
- `synopsis.rs::generate_synopsis_inner` (`"synopsis"`)
- `peer_discussion.rs::generate_peer_discussion_inner` (`"peer_discussion"`)

Inner functions gain an `app: Option<&tauri::AppHandle>` parameter (outer
commands pass `Some(&app)`; tests pass `None`). The `process_recording`
pipeline passes its app handle so the record tab gets progress too. Timing
(`Instant` around the call) and `record_completion_stat` are untouched.

### 4. Events + frontend

- `GenerationProgress` gains `#[serde(skip_serializing_if = "Option::is_none")]
  progress: Option<GenerationProgressStats>` where
  `GenerationProgressStats { tokens: u64, elapsed_ms: u64, tokens_per_second: f64 }`.
  Existing consumers ignore it (absent on started/completed/failed).
- `pipeline-progress` payload gains the same optional `progress` field so
  the record pipeline's `generating_soap` stage can render the counter.
- Frontend: type mirrors in `types/index.ts`; generation store keeps the
  latest `progress` per doc type; `GenerateItem` renders
  `Generating… {tokens} tokens · {tps} tok/s` in the existing
  `progress-phase` span (aria-live polite, 500ms throttle ≈ 2 updates/s);
  `PipelineStatus` shows the same counter during `generating_soap`.
  `formatTokensPerSecond` is reused for the live value.

### 5. Test scaffolding

`MockCompletionProvider` gains a `complete_stream` implementation that
replays a scripted `Vec<StreamChunk>` (deltas, `ReasoningDelta`s, terminal
`Usage`+`Done`) via `tokio_stream::iter`, so the five existing stats wiring
tests keep passing once call sites stream.

## Privacy

Progress events and logs carry counts, lengths, and durations only. Reasoning
text is reduced to `len` at the provider boundary and never crosses it. No
new PHI-bearing fields anywhere. (AGENTS.md: log counts and lengths, never
content.)

## Error handling

- Stream error mid-generation → fail fast with the existing mapping (no
  retry; provider-level transport retries still apply only to the initial
  connect, per `send_with_retry`).
- Idle timeout → actionable "generation stalled" error.
- Provider closes without usage → content still returned; no stat recorded.
- Cancellation: dropping the future drops the reqwest stream → connection
  closes → LM Studio stops generating (better than non-streamed cancel).

## Testing

- Wire: `reasoning_content` parsing; `ReasoningDelta` mapping; per-request
  timeout construction (unit).
- Helper: scripted-stream assembly; idle-timeout expiry (tiny injected
  idle); `<think>` stripping; usage-less stream; progress throttling
  (assert ≥2 events over a scripted long stream; no `app` → none).
- Call sites: existing five stats tests unchanged (mock streams now);
  progress events asserted in one wiring test.
- Frontend: progress text formatting; store keeps latest per doc type;
  `npm run check`/vitest green.
- Gates: `cargo fmt`, `clippy -D warnings`, workspace lib tests, vitest.

## Out of scope

- Streaming raw text into the UI; chat path changes; per-provider reasoning
  controls (unsupported on current LM Studio builds); changing the persisted
  `generation_stats` shape.
