# Tokens-per-second for local LLM generations — Design

**Date:** 2026-08-16
**Status:** Implemented on branch `feat/tokens-per-second`

## Problem

When generating documentation with a local LLM (Ollama or LM Studio), there is
no visibility into how fast the model actually ran. The provider response
already carries token counts (`CompletionResponse.usage`), but every generation
command discards them, and nothing per-recording is persisted or displayed.

**Goal:** after each LLM generation on a recording, record the generation
throughput and display it on the Recordings tab entry for that recording.

## Approach considered

1. **Client-side wall-clock + already-parsed usage (chosen).** Wrap
   `provider.complete()` with `std::time::Instant` and compute
   `tokens_per_second = usage.completion_tokens / elapsed_seconds`. The
   OpenAI-compat client (`crates/ai-providers/src/openai_compat/`) already
   parses `usage` into `UsageInfo` for both providers — Ollama and LM Studio
   are thin wrappers over the same client (`ollama.rs:64`, `lmstudio.rs:64`),
   so this works uniformly with zero protocol changes.
   *Known caveat:* wall-clock includes prompt prefill and HTTP overhead, so the
   number slightly understates the model's pure decode rate. For localhost
   providers with a few-thousand-token prompt this is a small effect and the
   value remains a fair "effective throughput" figure.
2. **Native provider APIs.** Ollama's `/api/chat` returns exact
   `eval_count`/`eval_duration`; LM Studio exposes a non-standard
   `stats.tokens_per_second`. Rejected: needs a second Ollama client beside the
   OpenAI-compat one, depends on a non-standard LM Studio field, and diverges
   the two providers — too much machinery for a diagnostic badge.
3. **Streaming + live in-flight counter.** SOAP generation is a single
   non-streamed request today; a live counter would require switching the
   generation path to streaming. Out of scope; the persisted record (tokens +
   duration stored separately) lets a future streaming effort reuse the same
   display without schema changes.

## Design

### Measurement

At each generation call site:

```rust
let start = std::time::Instant::now();
let response = provider.complete(request).await ...;
let stat = GenerationStat::from_completion(
    provider.name(), &model_name, &response.usage, start.elapsed(),
);
```

`from_completion` returns `None` when `completion_tokens == 0` or the elapsed
time is zero — in that case nothing is recorded (and no badge is shown). Stats
are computed only on the success path (an errored generation has no
throughput worth recording).

### Data model — `recordings.metadata`

New metadata key `generation_stats`, one slot per document type, latest
generation overwrites its own slot (sibling slots are preserved):

```json
{
  "context": "...",
  "patient_context": { ... },
  "generation_stats": {
    "soap": {
      "provider": "ollama",
      "model": "llama3:8b",
      "prompt_tokens": 2048,
      "completion_tokens": 512,
      "duration_ms": 12345,
      "tokens_per_second": 41.5,
      "generated_at": "2026-08-16T12:34:56Z"
    },
    "referral": { ... },
    "letter": { ... },
    "synopsis": { ... },
    "peer_discussion": { ... }
  }
}
```

Per AGENTS.md, new metadata keys are non-breaking — **no DB migration is
needed**, and metadata round-trips to the frontend on the full `Recording`.

Typed in `crates/core/src/types/recording.rs`:

```rust
pub struct GenerationStat {
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub duration_ms: u64,
    pub tokens_per_second: f64,
    pub generated_at: DateTime<Utc>,
}
```

plus free functions (pure, unit-testable without a live provider):

- `GenerationStat::from_completion(provider, model, usage, elapsed) -> Option<Self>`
- `merge_generation_stat(metadata: &mut serde_json::Value, doc_type: &str, stat: GenerationStat)` —
  creates/updates `metadata.generation_stats[doc_type]`, never touches other keys
- `latest_tokens_per_second(metadata: &serde_json::Value) -> Option<f64>` —
  the stat with the newest `generated_at` across the five doc-type keys;
  entries with an unparseable `generated_at` (or that fail to deserialize as `GenerationStat`) are skipped

### Backend call sites

| Command | File | Stats key |
|---|---|---|
| `generate_soap` (and the record-tab pipeline via `generate_soap_inner`) | `src-tauri/src/commands/generation/soap.rs:192` | `soap` |
| referral, letter (via `generate_from_soap`) | `src-tauri/src/commands/generation/helpers.rs:265` | `referral` / `letter` (new canonical `stats_key` param; callers already pass `"referral"` / `"letter"` to `run_generation_command`) |
| `generate_synopsis` | `src-tauri/src/commands/generation/synopsis.rs:79` | `synopsis` |
| `generate_peer_discussion` | `src-tauri/src/commands/generation/peer_discussion.rs:166` | `peer_discussion` |

The stats are merged into `recording.metadata` before each flow's existing
persistence — directly in `generate_soap_inner`, `synopsis`, and
`peer_discussion` (which persist themselves), and inside `generate_from_soap`
for referral/letter (whose callers persist immediately after) — so no extra
DB write is introduced. Best-effort: a stats failure can
never fail the generation (log at `warn` and continue — metadata merge is
infallible in practice, but the code path stays non-fatal by construction).

Out of scope: `generate_letter_from_document` (standalone Letter Writer — no
recording row to attach to) and streaming chat (already reports ephemeral
usage via the `chat-done` event).

A `debug!` log line records `tokens_per_second`, `completion_tokens`, and
`duration_ms` after each generation — counts and durations only, no PHI.

### Surfacing to the UI

`RecordingSummary` (`crates/core/src/types/recording.rs`) gains:

```rust
pub tokens_per_second: Option<f64>,
```

populated in `From<&Recording>` via `latest_tokens_per_second(&r.metadata)` and
added to the manual `Debug` impl (structural metadata, not PHI).

Frontend:

- `src/lib/types/index.ts` — `RecordingSummary.tokens_per_second?: number | null`;
  `Recording.metadata` type gains an optional `generation_stats` key.
- `src/lib/stores/recordings.svelte.ts` — the `search()` hand-mapping computes
  the same value via a new util (mirrors the existing dual-side `is_remote`
  pattern for `synced_from`):
  `src/lib/utils/generationStats.ts` → `latestTokensPerSecond(metadata)`.
- `src/lib/components/RecordingCard.svelte` — meta row becomes
  `Apr 14, 2026 · 3:42 · 41.5 tok/s`. Rendered as a muted `<span>` with
  `title="Latest AI generation speed (tokens/sec)"`, appended to the card's
  `aria-label` when present, hidden entirely when `null`.
- `src/lib/utils/format.ts` — `formatTokensPerSecond(v)`:
  `v >= 100` → 0 decimals, else 1 decimal, suffixed ` tok/s`.

### Error handling and edge cases

- Provider returns no `usage` (or zero completion tokens) → no stat recorded,
  badge stays hidden. Regeneration simply overwrites the slot.
- Very short generations can round to high tok/s values — that is genuine.
- Elapsed of exactly zero (clock granularity) → no stat, avoids `inf`.
- Legacy recordings without stats → `tokens_per_second: null`, badge hidden.
- The merge never removes `context` / `patient_context` / `synopsis` /
  `peer_discussion_context` / `synced_from` keys.

### Privacy

`GenerationStat` contains provider name, model name, token counts, duration,
and a timestamp — no transcripts, no SOAP content, no patient identifiers.
It is safe to log (counts/lengths only per AGENTS.md) and is stored inside the
already-encrypted SQLCipher database. Synced machines pick it up transparently
through the existing metadata replication.

### Testing

Rust (`cargo test --workspace --lib`):

- `from_completion`: correct math; `None` on zero tokens / zero elapsed /
  missing usage.
- `merge_generation_stat`: creates nested object; overwrites own slot only;
  preserves sibling doc-type stats and unrelated metadata keys; handles
  `metadata: null`.
- `latest_tokens_per_second`: picks newest `generated_at`; `None` when no
  stats; unparseable `generated_at` ranks oldest.
- `RecordingSummary::from` maps the field; existing summary tests updated.

Frontend (`npx vitest run`):

- `format.test.ts` — `formatTokensPerSecond` rounding cases.
- New `RecordingCard.test.ts` (jsdom, following `ConditionChips.test.ts`
  patterns) — badge renders when `tokens_per_second` present, absent when null.
- `generationStats.test.ts` — `latestTokensPerSecond` mirrors Rust semantics.
- `recordings.test.ts` — `makeSummary()` gains the field.

Gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
-- -D warnings`, `npm run check`, `npm run lint`.

## Future enhancements (explicitly out of scope)

- Live in-flight tok/s during generation (requires streaming generation).
- Native Ollama `eval_rate` for decode-only precision (stored tokens/duration
  make this a drop-in math change).
- A per-recording stats history / detail view in the Editor tab (data is
  already in `Recording.metadata`).
