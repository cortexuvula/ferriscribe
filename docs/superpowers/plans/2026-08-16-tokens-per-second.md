# Tokens-per-Second Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record LLM generation throughput (tokens/sec) for every recording generation command and display it on the Recordings tab entry.

**Architecture:** Wrap each `provider.complete()` call in `std::time::Instant` timing, combine with the already-parsed `UsageInfo` token counts, and persist a `GenerationStat` under `recording.metadata["generation_stats"][doc_type]` (no DB migration — AGENTS.md blesses new metadata keys). `RecordingSummary` surfaces the latest stat's `tokens_per_second` to the frontend; `RecordingCard` renders it in the meta row. Spec: `docs/superpowers/specs/2026-08-16-tokens-per-second-design.md`.

**Tech Stack:** Rust (edition 2024, medical-core + src-tauri `rust-medical-assistant`), Svelte 5 runes + TypeScript, vitest + @testing-library/svelte.

**Execution note:** Per AGENTS.md branch hygiene, execute in a git worktree under `.worktrees/` on branch `feat/tokens-per-second` (use the superpowers:using-git-worktrees skill). Never commit implementation to `master`.

---

### Task 1: `GenerationStat` type + `from_completion` (medical-core)

**Files:**
- Modify: `crates/core/src/types/recording.rs` (add struct + method after the `RecordingSummary` `From` impl, before `#[cfg(test)] mod tests` at line 318; add tests inside the existing `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add at the end of the existing `mod tests` in `crates/core/src/types/recording.rs` (after the `summary_from_recording` test), plus a file-top import change.

Import change — the file currently starts with:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;
```

Add one line after the `chrono` import:

```rust
use super::ai::UsageInfo;
```

Tests (place inside `mod tests`, which already has `use super::*;`):

```rust
    #[test]
    fn generation_stat_from_completion_computes_throughput() {
        let usage = UsageInfo {
            prompt_tokens: 1000,
            completion_tokens: 200,
            total_tokens: 1200,
        };
        let stat = GenerationStat::from_completion(
            "ollama",
            "llama3",
            &usage,
            std::time::Duration::from_millis(4000),
        )
        .expect("throughput is computable");
        assert_eq!(stat.provider, "ollama");
        assert_eq!(stat.model, "llama3");
        assert_eq!(stat.prompt_tokens, 1000);
        assert_eq!(stat.completion_tokens, 200);
        assert_eq!(stat.duration_ms, 4000);
        assert_eq!(stat.tokens_per_second, 50.0);
    }

    #[test]
    fn generation_stat_from_completion_rejects_zero_completion_tokens() {
        let usage = UsageInfo::default(); // completion_tokens == 0
        assert!(GenerationStat::from_completion(
            "ollama",
            "llama3",
            &usage,
            std::time::Duration::from_secs(1)
        )
        .is_none());
    }

    #[test]
    fn generation_stat_from_completion_rejects_zero_elapsed() {
        let usage = UsageInfo {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        };
        assert!(
            GenerationStat::from_completion("ollama", "llama3", &usage, std::time::Duration::ZERO)
                .is_none()
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p medical-core --lib recording`
Expected: COMPILE ERROR (`failed to resolve: use of undeclared type 'GenerationStat'`).

- [ ] **Step 3: Write the implementation**

In `crates/core/src/types/recording.rs`, after the `impl From<&Recording> for RecordingSummary` block (ends ~line 316) and before `#[cfg(test)]`:

```rust
/// Throughput metrics for a single LLM generation, persisted under
/// `recording.metadata["generation_stats"][doc_type]`.
///
/// Contains only counts, durations, and provider/model names — no PHI
/// (AGENTS.md: log counts and lengths, never content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationStat {
    /// Provider that produced the generation (e.g. `"ollama"`, `"lmstudio"`).
    pub provider: String,
    /// Model used for the generation.
    pub model: String,
    /// Tokens consumed by the prompt (input).
    pub prompt_tokens: u32,
    /// Tokens produced by the completion (output).
    pub completion_tokens: u32,
    /// Wall-clock duration of the completion call, in milliseconds.
    pub duration_ms: u64,
    /// Effective throughput: completion tokens divided by wall-clock seconds.
    pub tokens_per_second: f64,
    /// When the generation completed.
    pub generated_at: DateTime<Utc>,
}

impl GenerationStat {
    /// Compute a stat from a completion response's usage plus the
    /// wall-clock time spent in `provider.complete()`.
    ///
    /// Returns `None` when no throughput can be derived (zero completion
    /// tokens or zero elapsed time) — nothing should be recorded then.
    pub fn from_completion(
        provider: &str,
        model: &str,
        usage: &UsageInfo,
        elapsed: std::time::Duration,
    ) -> Option<Self> {
        if usage.completion_tokens == 0 || elapsed.as_nanos() == 0 {
            return None;
        }
        let seconds = elapsed.as_secs_f64();
        Some(Self {
            provider: provider.to_string(),
            model: model.to_string(),
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            duration_ms: elapsed.as_millis() as u64,
            tokens_per_second: usage.completion_tokens as f64 / seconds,
            generated_at: Utc::now(),
        })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p medical-core --lib recording`
Expected: PASS (including the three new tests and all pre-existing ones).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/core/src/types/recording.rs
git commit -m "feat(core): add GenerationStat type with throughput computation"
```

---

### Task 2: `merge_generation_stat` + `latest_tokens_per_second` (medical-core)

**Files:**
- Modify: `crates/core/src/types/recording.rs` (helpers after the `GenerationStat` impl from Task 1; tests in `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` (a small local factory keeps the tests DRY):

```rust
    fn stat(tokens_per_second: f64, generated_at: chrono::DateTime<Utc>) -> GenerationStat {
        GenerationStat {
            provider: "ollama".to_string(),
            model: "llama3".to_string(),
            prompt_tokens: 10,
            completion_tokens: 100,
            duration_ms: 1000,
            tokens_per_second,
            generated_at,
        }
    }

    #[test]
    fn merge_generation_stat_overwrites_own_slot_only() {
        let mut metadata = serde_json::json!({ "context": "visit notes" });
        merge_generation_stat(&mut metadata, "soap", stat(20.0, Utc::now()));

        assert_eq!(metadata["context"], serde_json::json!("visit notes"));
        assert_eq!(
            metadata["generation_stats"]["soap"]["tokens_per_second"],
            serde_json::json!(20.0)
        );

        merge_generation_stat(&mut metadata, "referral", stat(150.0, Utc::now()));
        merge_generation_stat(&mut metadata, "soap", stat(75.5, Utc::now()));

        // soap slot overwritten by its newest write; referral slot preserved;
        // unrelated metadata keys untouched.
        assert_eq!(
            metadata["generation_stats"]["soap"]["tokens_per_second"],
            serde_json::json!(75.5)
        );
        assert_eq!(
            metadata["generation_stats"]["referral"]["tokens_per_second"],
            serde_json::json!(150.0)
        );
        assert_eq!(metadata["context"], serde_json::json!("visit notes"));
    }

    #[test]
    fn merge_generation_stat_initializes_null_metadata() {
        let mut metadata = serde_json::Value::Null;
        merge_generation_stat(&mut metadata, "soap", stat(20.0, Utc::now()));
        assert!(metadata["generation_stats"]["soap"].is_object());
    }

    #[test]
    fn latest_tokens_per_second_picks_newest_generated_at() {
        let older_at = Utc::now() - chrono::TimeDelta::hours(2);
        let mut metadata = serde_json::json!({});
        merge_generation_stat(&mut metadata, "soap", stat(20.0, older_at));
        merge_generation_stat(&mut metadata, "letter", stat(75.5, Utc::now()));

        assert_eq!(latest_tokens_per_second(&metadata), Some(75.5));
    }

    #[test]
    fn latest_tokens_per_second_none_without_stats() {
        assert_eq!(latest_tokens_per_second(&serde_json::Value::Null), None);
        assert_eq!(
            latest_tokens_per_second(&serde_json::json!({ "context": "x" })),
            None
        );
    }

    #[test]
    fn latest_tokens_per_second_skips_malformed_entries() {
        let metadata = serde_json::json!({
            "generation_stats": {
                "soap": { "tokens_per_second": 99.0 },
                "referral": stat(40.0, Utc::now())
            }
        });
        // "soap" is missing required fields → skipped; the valid referral
        // entry wins despite the lower value.
        assert_eq!(latest_tokens_per_second(&metadata), Some(40.0));
    }
```

Note: `stat(...)` is usable inside `serde_json::json!` because it's a plain function call expression.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p medical-core --lib recording`
Expected: COMPILE ERROR (`cannot find function 'merge_generation_stat'`).

- [ ] **Step 3: Write the implementation**

Add directly after the `impl GenerationStat` block from Task 1:

```rust
/// Doc-type keys that may appear under `generation_stats`.
pub const GENERATION_STAT_DOC_TYPES: [&str; 5] =
    ["soap", "referral", "letter", "synopsis", "peer_discussion"];

/// Merge `stat` into `metadata["generation_stats"][doc_type]`, creating the
/// nested object when absent. Never touches any other metadata key.
pub fn merge_generation_stat(
    metadata: &mut serde_json::Value,
    doc_type: &str,
    stat: GenerationStat,
) {
    if !metadata.is_object() {
        *metadata = serde_json::json!({});
    }
    let Some(obj) = metadata.as_object_mut() else {
        return;
    };
    let stats = obj
        .entry("generation_stats".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !stats.is_object() {
        *stats = serde_json::json!({});
    }
    if let Some(stats_obj) = stats.as_object_mut() {
        stats_obj.insert(
            doc_type.to_string(),
            serde_json::to_value(stat).unwrap_or(serde_json::Value::Null),
        );
    }
}

/// The `tokens_per_second` of the most recent generation across doc types
/// (newest `generated_at`), or `None` when no valid stats are recorded.
/// Entries that fail to deserialize as [`GenerationStat`] are skipped.
pub fn latest_tokens_per_second(metadata: &serde_json::Value) -> Option<f64> {
    let stats = metadata.get("generation_stats")?;
    let mut best: Option<(DateTime<Utc>, f64)> = None;
    for key in GENERATION_STAT_DOC_TYPES {
        let Some(raw) = stats.get(key) else { continue };
        let Ok(stat) = serde_json::from_value::<GenerationStat>(raw.clone()) else {
            continue;
        };
        if best.is_none_or(|(best_at, _)| stat.generated_at >= best_at) {
            best = Some((stat.generated_at, stat.tokens_per_second));
        }
    }
    best.map(|(_, tokens_per_second)| tokens_per_second)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p medical-core --lib recording`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/core/src/types/recording.rs
git commit -m "feat(core): add generation-stats metadata merge + latest extraction helpers"
```

---

### Task 3: `RecordingSummary.tokens_per_second` (medical-core)

**Files:**
- Modify: `crates/core/src/types/recording.rs:245-316` (struct + Debug + From impls; tests in `mod tests`)

- [ ] **Step 1: Write the failing test**

Add inside `mod tests`:

```rust
    #[test]
    fn summary_tokens_per_second_from_metadata() {
        let mut rec = Recording::new("visit.wav", PathBuf::from("/audio/visit.wav"));
        rec.transcript = Some("Hello".into());

        // No stats recorded yet → None.
        assert_eq!(RecordingSummary::from(&rec).tokens_per_second, None);

        let older_at = Utc::now() - chrono::TimeDelta::hours(1);
        merge_generation_stat(&mut rec.metadata, "soap", stat(50.0, older_at));
        merge_generation_stat(&mut rec.metadata, "referral", stat(100.0, Utc::now()));

        let summary = RecordingSummary::from(&rec);
        assert_eq!(summary.tokens_per_second, Some(100.0));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p medical-core --lib summary_tokens_per_second`
Expected: COMPILE ERROR (no `tokens_per_second` field on `RecordingSummary`).

- [ ] **Step 3: Write the implementation**

Three edits in `crates/core/src/types/recording.rs`:

1. In `pub struct RecordingSummary`, add after the `is_remote` field (line ~273):

```rust
    /// Throughput (tokens/sec) of the most recent AI generation for this
    /// recording, from `metadata.generation_stats` — `None` when no
    /// generation has recorded stats.
    pub tokens_per_second: Option<f64>,
```

2. In the manual `Debug` impl (line ~277), add before `.finish()`:

```rust
            .field("tokens_per_second", &self.tokens_per_second)
```

3. In `impl From<&Recording> for RecordingSummary` (line ~297), add after `is_remote`:

```rust
            tokens_per_second: latest_tokens_per_second(&r.metadata),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p medical-core --lib recording`
Expected: PASS (new test + all existing summary tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/core/src/types/recording.rs
git commit -m "feat(core): surface tokens_per_second on RecordingSummary"
```

---

### Task 4: Mock `AiProvider` + provider-injectable test state (src-tauri)

**Files:**
- Modify: `src-tauri/Cargo.toml` (`[dev-dependencies]`, line 61)
- Modify: `src-tauri/src/commands/generation/test_helpers.rs` (refactor + new helper + mock)

- [ ] **Step 1: Add the dev-dependency**

In `src-tauri/Cargo.toml` `[dev-dependencies]` (currently `tempfile`, `filetime`, `wiremock`), add:

```toml
async-trait = { workspace = true }
```

(`async-trait = "0.1"` already exists in the workspace `[workspace.dependencies]`, Cargo.toml line 62.)

- [ ] **Step 2: Refactor the state builder to accept a provider override**

In `src-tauri/src/commands/generation/test_helpers.rs`:

Rename the existing `build_test_state_with_recording` function (line 27) to `build_test_state_inner` and add a parameter `provider_override: Option<Arc<dyn medical_core::traits::AiProvider>>`:

```rust
async fn build_test_state_inner(
    config: AppConfig,
    transcript_text: &str,
    provider_override: Option<Arc<dyn medical_core::traits::AiProvider>>,
) -> (AppState, String) {
```

Replace the registry section (the `if let Ok(p) = ... { registry.register(...) }` block) with:

```rust
    match provider_override {
        Some(provider) => {
            registry.register(provider);
            registry.set_active(&config.ai_provider);
        }
        None => {
            let ollama_host = if config.ollama_host.is_empty() {
                "localhost"
            } else {
                config.ollama_host.as_str()
            };
            let ollama_url = format!("http://{}:{}", ollama_host, config.ollama_port);
            if let Ok(p) = medical_ai_providers::ollama::OllamaProvider::new_with_endpoint(
                Some(&ollama_url),
                config.allow_public_endpoint,
                None,
                medical_ai_providers::http_client::RetryConfig::default(),
                None,
            ) {
                registry.register(Arc::new(p) as Arc<dyn medical_core::traits::AiProvider>);
                registry.set_active(&config.ai_provider);
            }
        }
    }
```

Then reinstate the original public helper plus the new one (place both directly above `build_test_state_inner`):

```rust
pub(super) async fn build_test_state_with_recording(
    config: AppConfig,
    transcript_text: &str,
) -> (AppState, String) {
    build_test_state_inner(config, transcript_text, None).await
}

/// Like [`build_test_state_with_recording`], but registers `provider`
/// (under its own `name()`, set active) instead of a real Ollama provider.
/// `config.ai_provider` must match `provider.name()` so `resolve_provider`
/// finds it, and `config.ollama_host` should be loopback so the pre-flight
/// probe is skipped.
pub(super) async fn build_test_state_with_provider(
    config: AppConfig,
    transcript_text: &str,
    provider: Arc<dyn medical_core::traits::AiProvider>,
) -> (AppState, String) {
    build_test_state_inner(config, transcript_text, Some(provider)).await
}
```

- [ ] **Step 3: Add the mock provider**

Append to `test_helpers.rs` (add `use medical_core::error::AppError;` and `use medical_core::error::AppResult;` to the file's existing imports):

```rust
/// Deterministic in-process `AiProvider` for generation success-path tests.
///
/// `complete()` returns a fixed non-empty completion with a known token
/// usage; every other method is unused by these tests and returns an error
/// or an empty list. Never performs network I/O.
pub(super) struct MockCompletionProvider {
    name: &'static str,
    content: String,
    usage: medical_core::types::UsageInfo,
}

impl MockCompletionProvider {
    /// `completion_tokens` drives the recorded throughput stat.
    pub(super) fn new(name: &'static str, content: &str, completion_tokens: u32) -> Self {
        Self {
            name,
            content: content.to_string(),
            usage: medical_core::types::UsageInfo {
                prompt_tokens: 128,
                completion_tokens,
                total_tokens: 128 + completion_tokens,
            },
        }
    }
}

#[async_trait::async_trait]
impl medical_core::traits::AiProvider for MockCompletionProvider {
    fn name(&self) -> &str {
        self.name
    }

    async fn available_models(&self) -> AppResult<Vec<medical_core::types::ModelInfo>> {
        Ok(Vec::new())
    }

    async fn complete(
        &self,
        _request: medical_core::types::CompletionRequest,
    ) -> AppResult<medical_core::types::CompletionResponse> {
        Ok(medical_core::types::CompletionResponse {
            content: self.content.clone(),
            model: "mock-model".to_string(),
            usage: self.usage.clone(),
            tool_calls: Vec::new(),
        })
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
        Err(AppError::ai_provider(
            "mock provider does not support streaming".to_string(),
        ))
    }

    async fn complete_with_tools(
        &self,
        _request: medical_core::types::CompletionRequest,
        _tools: Vec<medical_core::types::ToolDef>,
    ) -> AppResult<medical_core::types::ToolCompletionResponse> {
        Err(AppError::ai_provider(
            "mock provider does not support tools".to_string(),
        ))
    }
}
```

(`futures-util` is already a dependency of src-tauri, Cargo.toml line 31; `futures_util::Stream` is a re-export of the `futures_core::Stream` the trait signature uses, so the types are identical.)

- [ ] **Step 4: Verify it compiles**

Run: `cargo test -p rust-medical-assistant --lib --no-run`
Expected: COMPILES (existing preflight tests still reference `build_test_state_with_recording`, which keeps its signature).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add src-tauri/Cargo.toml src-tauri/src/commands/generation/test_helpers.rs
git commit -m "test(tauri): mock AiProvider + provider-injectable test state helper"
```

---

### Task 5: Record stats in `generate_soap_inner` (src-tauri)

**Files:**
- Modify: `src-tauri/src/commands/generation/soap.rs` (imports; complete call ~line 192; metadata section ~line 297; new test mod)

- [ ] **Step 1: Write the failing test**

Add a new test module at the end of `soap.rs`:

```rust
#[cfg(test)]
mod stats_tests {
    use super::super::test_helpers::{
        MockCompletionProvider, build_test_state_with_provider,
    };
    use super::*;
    use medical_core::types::settings::AppConfig;

    #[tokio::test]
    async fn generate_soap_records_generation_stats_in_metadata() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        // Loopback → preflight probe is skipped; the mock serves completions.
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = Arc::new(MockCompletionProvider::new(
            "ollama",
            "S: Headache for 3 days.\nA: Tension headache.\nP: Rest, follow up in 2 weeks.",
            200,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        let soap = generate_soap_inner(&state, &recording_id, None, None, None)
            .await
            .expect("generation with mock provider succeeds");
        assert!(!soap.is_empty());

        let uuid = Uuid::parse_str(&recording_id).expect("valid uuid");
        let conn = state.db.conn().expect("conn");
        let rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
            .expect("recording persisted");

        let stat: GenerationStat = serde_json::from_value(
            rec.metadata["generation_stats"]["soap"].clone(),
        )
        .expect("soap stat recorded");
        assert_eq!(stat.provider, "ollama");
        assert_eq!(stat.model, "llama3");
        assert_eq!(stat.prompt_tokens, 128);
        assert_eq!(stat.completion_tokens, 200);
        assert!(stat.tokens_per_second.is_finite());
        assert!(stat.tokens_per_second > 0.0);

        assert_eq!(
            medical_core::types::recording::latest_tokens_per_second(&rec.metadata),
            Some(stat.tokens_per_second)
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rust-medical-assistant --lib stats_tests`
Expected: COMPILE ERROR (`GenerationStat` not in scope / metadata index returns Null at runtime — add the import first and it fails at `.expect("soap stat recorded")` with `soap stat recorded`).

- [ ] **Step 3: Write the implementation**

Three edits in `soap.rs`:

1. Imports — extend the existing `use medical_core::types::PatientContext;` (line 6) area with:

```rust
use medical_core::types::recording::{GenerationStat, merge_generation_stat};
```

2. Time the completion call. Replace (line ~192):

```rust
    let response = provider.complete(request).await.map_err(|e| match e {
```

with:

```rust
    let generation_start = std::time::Instant::now();
    let response = provider.complete(request).await.map_err(|e| match e {
```

and after the closing `})?;` of that call (line ~200), add:

```rust
    let generation_elapsed = generation_start.elapsed();
```

3. Record the stat. In the metadata section, after the `if let Some(obj) = recording.metadata.as_object_mut() { ... }` block (which inserts `context`/`patient_context`, ends line ~297) and before the `// Persist to DB (on blocking thread)` comment:

```rust
    if let Some(stat) = GenerationStat::from_completion(
        provider.name(),
        &model_name,
        &response.usage,
        generation_elapsed,
    ) {
        tracing::debug!(
            doc_type = "soap",
            tokens_per_second = stat.tokens_per_second,
            completion_tokens = stat.completion_tokens,
            duration_ms = stat.duration_ms,
            "generation throughput recorded"
        );
        merge_generation_stat(&mut recording.metadata, "soap", stat);
    }
```

(`response.content` is moved into `raw_soap` earlier; borrowing `response.usage` afterwards is a legal partial-move borrow.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rust-medical-assistant --lib stats_tests`
Expected: PASS. Also run `cargo test -p rust-medical-assistant --lib` — all preflight tests still pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add src-tauri/src/commands/generation/soap.rs
git commit -m "feat(tauri): record SOAP generation throughput in recording metadata"
```

---

### Task 6: Record stats in `generate_from_soap` (referral + letter)

**Files:**
- Modify: `src-tauri/src/commands/generation/helpers.rs` (imports; `generate_from_soap` signature line ~207; complete call ~line 265; stat merge before return ~line 282)
- Modify: `src-tauri/src/commands/generation/referral.rs` (2 call sites: main ~line 41, test ~line 96; new test mod)
- Modify: `src-tauri/src/commands/generation/letter.rs` (2 call sites: main ~line 52, test ~line 107)

- [ ] **Step 1: Write the failing test**

Add a new test module at the end of `referral.rs`:

```rust
#[cfg(test)]
mod stats_tests {
    use super::super::test_helpers::{
        MockCompletionProvider, build_test_state_with_provider,
    };
    use super::*;
    use medical_core::types::settings::AppConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn generate_from_soap_records_referral_stats() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = Arc::new(MockCompletionProvider::new(
            "ollama",
            "Dear Cardiology, please assess this patient for chest pain.",
            64,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        // generate_from_soap requires an existing SOAP note.
        {
            let uuid = uuid::Uuid::parse_str(&recording_id).expect("uuid");
            let conn = state.db.conn().expect("conn");
            let mut rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
                .expect("recording");
            rec.soap_note = Some("S: Chest pain.\nA: Angina.\nP: Cardiology referral.".to_string());
            medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
        }

        let (mut recording, settings, config) =
            load_recording_and_settings(&state.db, &recording_id).await.unwrap();

        let text = generate_from_soap(
            &state,
            &mut recording,
            &settings,
            &config,
            medical_core::preflight::CommandKind::GenerateReferral,
            "referral letter",
            "referral",
            |soap_note, settings| {
                document_generator::build_referral_prompt(
                    soap_note,
                    "Specialist",
                    "routine",
                    settings.custom_referral_prompt.as_deref(),
                    None,
                )
            },
            |rec, text| {
                rec.referral = Some(text);
            },
        )
        .await
        .expect("referral generation succeeds");
        assert!(!text.is_empty());

        assert_eq!(
            recording.metadata["generation_stats"]["referral"]["completion_tokens"],
            serde_json::json!(64)
        );
        assert_eq!(
            recording.metadata["generation_stats"]["referral"]["model"],
            serde_json::json!("llama3")
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rust-medical-assistant --lib stats_tests`
Expected: COMPILE ERROR (`generate_from_soap` takes 8 args, 9 supplied — the new `stats_key` param doesn't exist yet).

- [ ] **Step 3: Write the implementation**

Edits in `helpers.rs`:

1. Import — replace line 7 `use medical_core::types::recording::Recording;` with:

```rust
use medical_core::types::recording::{GenerationStat, Recording, merge_generation_stat};
```

2. `generate_from_soap` signature — add `stats_key: &'static str,` immediately after `doc_type_label: &str,` (line ~214):

```rust
pub(super) async fn generate_from_soap<F, S>(
    state: &AppState,
    recording: &mut Recording,
    settings: &GenerationSettings,
    config: &AppConfig,
    command_kind: medical_core::preflight::CommandKind,
    doc_type_label: &str,
    stats_key: &'static str,
    build_prompt: F,
    set_field: S,
) -> AppResult<String>
```

Also update the doc-comment bullet list above it: add `- stats_key: canonical key under metadata.generation_stats (e.g. "referral").`

3. Time the completion call. Replace (line ~265):

```rust
    let response = provider.complete(request).await.map_err(|e| match e {
```

with:

```rust
    let generation_start = std::time::Instant::now();
    let response = provider.complete(request).await.map_err(|e| match e {
```

and after the closing `})?;`, add:

```rust
    let generation_elapsed = generation_start.elapsed();
```

4. Record the stat. Replace the tail of the function:

```rust
    set_field(recording, text.clone());
    Ok(text)
```

with:

```rust
    set_field(recording, text.clone());

    if let Some(stat) = GenerationStat::from_completion(
        provider.name(),
        &settings.model,
        &response.usage,
        generation_elapsed,
    ) {
        tracing::debug!(
            doc_type = stats_key,
            tokens_per_second = stat.tokens_per_second,
            completion_tokens = stat.completion_tokens,
            duration_ms = stat.duration_ms,
            "generation throughput recorded"
        );
        merge_generation_stat(&mut recording.metadata, stats_key, stat);
    }

    Ok(text)
```

(`build_completion_request` at line 259 receives `settings.model.clone()`, so `settings.model` is still intact here.)

Caller updates — each `generate_from_soap(` call gains the canonical key right after its human label:

- `referral.rs` main flow (~line 41): the argument `"referral letter",` becomes:

```rust
            "referral letter",
            "referral",
```

- `referral.rs` test module (~line 96): same addition after its `"referral letter",`.
- `letter.rs` main flow (~line 55): the argument `"letter",` becomes:

```rust
            "letter",
            "letter",
```

- `letter.rs` test module (~line 107): same addition after its `"letter",`.

(Find all sites with `grep -n "generate_from_soap(" src-tauri/src/commands/generation/referral.rs src-tauri/src/commands/generation/letter.rs` — there must be exactly two per file.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rust-medical-assistant --lib`
Expected: PASS (new stats test + both files' preflight tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add src-tauri/src/commands/generation/helpers.rs src-tauri/src/commands/generation/referral.rs src-tauri/src/commands/generation/letter.rs
git commit -m "feat(tauri): record referral/letter generation throughput"
```

---

### Task 7: Record stats in synopsis + peer discussion

**Files:**
- Modify: `src-tauri/src/commands/generation/synopsis.rs` (imports; model capture ~line 71; complete call ~line 79; merge before persist ~line 106; new test mod)
- Modify: `src-tauri/src/commands/generation/peer_discussion.rs` (same pattern around lines 161-190; new test mod)

- [ ] **Step 1: Write the failing tests**

Add a new test module at the end of `synopsis.rs`:

```rust
#[cfg(test)]
mod stats_tests {
    use super::super::test_helpers::{
        MockCompletionProvider, build_test_state_with_provider,
    };
    use super::*;
    use medical_core::types::settings::AppConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn generate_synopsis_records_generation_stats() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = Arc::new(MockCompletionProvider::new(
            "ollama",
            "Brief synopsis: tension headache, plan follow-up.",
            32,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        // Synopsis generation reads recording.soap_note.
        {
            let uuid = uuid::Uuid::parse_str(&recording_id).expect("uuid");
            let conn = state.db.conn().expect("conn");
            let mut rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
                .expect("recording");
            rec.soap_note = Some("S: Headache.\nA: Tension headache.\nP: Follow up.".to_string());
            medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
        }

        let synopsis = generate_synopsis_inner(&state, &recording_id)
            .await
            .expect("synopsis generation succeeds");
        assert!(!synopsis.is_empty());

        let uuid = uuid::Uuid::parse_str(&recording_id).expect("uuid");
        let conn = state.db.conn().expect("conn");
        let rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
            .expect("recording persisted");
        assert_eq!(
            rec.metadata["generation_stats"]["synopsis"]["completion_tokens"],
            serde_json::json!(32)
        );
        // The synopsis text itself stays where it always was.
        assert!(rec.metadata["synopsis"].is_string());
    }
}
```

Add a new test module at the end of `peer_discussion.rs`:

```rust
#[cfg(test)]
mod stats_tests {
    use super::super::test_helpers::{
        MockCompletionProvider, build_test_state_with_provider,
    };
    use super::*;
    use medical_core::types::settings::AppConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn generate_peer_discussion_records_generation_stats() {
        let mut config = AppConfig::default();
        config.ai_provider = "ollama".to_string();
        config.ollama_host = "localhost".to_string();
        config.ai_model = "llama3".to_string();

        let provider = Arc::new(MockCompletionProvider::new(
            "ollama",
            "Discussed the case with cardiology; agreed on outpatient workup.",
            48,
        ));
        let (state, recording_id) = build_test_state_with_provider(
            config,
            "Patient reports headache and fatigue.",
            provider,
        )
        .await;

        let text = generate_peer_discussion_inner(
            &state,
            &recording_id,
            "Smith",
            "Cardiology",
            "chest pain evaluation",
            None,
        )
        .await
        .expect("peer discussion generation succeeds");
        assert!(!text.is_empty());

        let uuid = uuid::Uuid::parse_str(&recording_id).expect("uuid");
        let conn = state.db.conn().expect("conn");
        let rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
            .expect("recording persisted");
        assert_eq!(
            rec.metadata["generation_stats"]["peer_discussion"]["completion_tokens"],
            serde_json::json!(48)
        );
        assert!(rec.metadata["peer_discussion_context"].is_object());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rust-medical-assistant --lib stats_tests`
Expected: FAIL — both tests panic because `generation_stats` is absent (index returns `Value::Null` and `assert_eq!` on `json!(32)` mismatches `Null`).

- [ ] **Step 3: Write the implementation**

Edits in `synopsis.rs`:

1. Import — add after the existing `use medical_core::error::...` line:

```rust
use medical_core::types::recording::{GenerationStat, merge_generation_stat};
```

2. Capture the model name. Replace (~line 70):

```rust
    let request = build_completion_request(
        system_prompt,
        user_prompt,
        settings.model,
        settings.temperature,
        None,
    );
```

with:

```rust
    let model_name = settings.model.clone();
    let request = build_completion_request(
        system_prompt,
        user_prompt,
        model_name.clone(),
        settings.temperature,
        None,
    );
```

3. Time the call. Replace (~line 79):

```rust
    let response = provider.complete(request).await.map_err(|e| match e {
```

with:

```rust
    let generation_start = std::time::Instant::now();
    let response = provider.complete(request).await.map_err(|e| match e {
```

and after the closing `})?;`, add:

```rust
    let generation_elapsed = generation_start.elapsed();
```

4. Record the stat. After the `if let Some(obj) = recording.metadata.as_object_mut() { ... }` block that inserts the `synopsis` key, and before `persist_recording(&state.db, recording).await?;` (~line 106), insert:

```rust
    if let Some(stat) = GenerationStat::from_completion(
        provider.name(),
        &model_name,
        &response.usage,
        generation_elapsed,
    ) {
        tracing::debug!(
            doc_type = "synopsis",
            tokens_per_second = stat.tokens_per_second,
            completion_tokens = stat.completion_tokens,
            duration_ms = stat.duration_ms,
            "generation throughput recorded"
        );
        merge_generation_stat(&mut recording.metadata, "synopsis", stat);
    }
```

Edits in `peer_discussion.rs` — identical pattern:

1. Import: `use medical_core::types::recording::{GenerationStat, merge_generation_stat};`
2. Capture the model name before `build_completion_request` (~line 161) exactly as in synopsis (add `let model_name = settings.model.clone();` and pass `model_name.clone()`).
3. Wrap the `provider.complete` call (~line 166) with `generation_start`/`generation_elapsed` exactly as in synopsis.
4. After the `if let Some(obj) = recording.metadata.as_object_mut() { ... }` block that inserts `peer_discussion_context`, before `recording.peer_discussion = Some(discussion_text.clone());` (~line 190), insert the same `if let Some(stat) = ...` block with `doc_type = "peer_discussion"` and `merge_generation_stat(&mut recording.metadata, "peer_discussion", stat);`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rust-medical-assistant --lib`
Expected: PASS (both new stats tests + all preflight tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add src-tauri/src/commands/generation/synopsis.rs src-tauri/src/commands/generation/peer_discussion.rs
git commit -m "feat(tauri): record synopsis + peer-discussion generation throughput"
```

---

### Task 8: Frontend types + `latestTokensPerSecond` helper

**Files:**
- Modify: `src/lib/types/index.ts` (new `GenerationStat` interface; `Recording.metadata`; `RecordingSummary` ~line 48-62)
- Create: `src/lib/utils/generationStats.ts`
- Create: `src/lib/utils/generationStats.test.ts`

- [ ] **Step 1: Write the failing tests**

Create `src/lib/utils/generationStats.test.ts` (node environment — pure function, no DOM):

```ts
import { describe, it, expect } from 'vitest';
import { latestTokensPerSecond } from './generationStats';
import type { Recording } from '../types';

const stat = (tokens_per_second: number, generated_at: string) => ({
  provider: 'ollama',
  model: 'llama3',
  prompt_tokens: 10,
  completion_tokens: 100,
  duration_ms: 1000,
  tokens_per_second,
  generated_at,
});

describe('latestTokensPerSecond', () => {
  it('returns null for null metadata', () => {
    expect(latestTokensPerSecond(null)).toBeNull();
  });

  it('returns null when generation_stats is absent', () => {
    const metadata = { context: 'x' } as Recording['metadata'];
    expect(latestTokensPerSecond(metadata)).toBeNull();
  });

  it('picks the newest generated_at across doc types', () => {
    const metadata = {
      generation_stats: {
        soap: stat(10, '2026-08-16T10:00:00Z'),
        referral: stat(42.5, '2026-08-16T11:00:00Z'),
        letter: stat(20, '2026-08-16T09:00:00Z'),
      },
    } as Recording['metadata'];
    expect(latestTokensPerSecond(metadata)).toBe(42.5);
  });

  it('skips entries with unparseable generated_at', () => {
    const metadata = {
      generation_stats: {
        soap: stat(10, 'not-a-date'),
        referral: stat(30, '2026-08-16T11:00:00Z'),
      },
    } as Recording['metadata'];
    expect(latestTokensPerSecond(metadata)).toBe(30);
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `npx vitest run src/lib/utils/generationStats.test.ts`
Expected: FAIL (`Failed to resolve import "./generationStats"`).

- [ ] **Step 3: Write the implementation**

Create `src/lib/utils/generationStats.ts`:

```ts
import type { Recording } from '../types';

type RecordingMetadata = Recording['metadata'];

const DOC_TYPES = ['soap', 'referral', 'letter', 'synopsis', 'peer_discussion'] as const;

/**
 * Mirror of the Rust `latest_tokens_per_second` helper
 * (crates/core/src/types/recording.rs): the `tokens_per_second` of the
 * stat with the newest `generated_at` across doc types, or null when no
 * stats are recorded. Entries with a non-numeric throughput or an
 * unparseable `generated_at` are skipped, matching Rust's strict
 * deserialization behavior. The data arrives as freeform JSON from the
 * backend, so each entry is treated as unknown-shaped rather than
 * trusting the declared `GenerationStat` type.
 */
export function latestTokensPerSecond(metadata: RecordingMetadata): number | null {
  if (!metadata) return null;
  const stats = metadata.generation_stats;
  if (!stats) return null;

  let bestTps: number | null = null;
  let bestAt = Number.NEGATIVE_INFINITY;
  for (const docType of DOC_TYPES) {
    const stat = stats[docType] as
      | { tokens_per_second?: unknown; generated_at?: unknown }
      | undefined;
    if (!stat) continue;
    if (typeof stat.tokens_per_second !== 'number') continue;
    if (typeof stat.generated_at !== 'string') continue;
    const parsed = Date.parse(stat.generated_at);
    if (Number.isNaN(parsed)) continue;
    if (bestTps === null || parsed >= bestAt) {
      bestTps = stat.tokens_per_second;
      bestAt = parsed;
    }
  }
  return bestTps;
}
```

Three edits in `src/lib/types/index.ts`:

1. New interface before the `── Recording ──` section:

```ts
// ── Generation Stats ──────────────────────────────────────────────────────────

/** Mirrors Rust `GenerationStat` — throughput metrics for one LLM generation. */
export interface GenerationStat {
  provider: string;
  model: string;
  prompt_tokens: number;
  completion_tokens: number;
  duration_ms: number;
  tokens_per_second: number;
  generated_at: string;
}
```

2. In `Recording`'s `metadata` type, add the typed key (the index signature still admits future keys):

```ts
  metadata: {
    context?: string;
    patient_context?: PatientContext;
    generation_stats?: { [docType: string]: GenerationStat };
    [key: string]: unknown;
  } | null;
```

3. In `RecordingSummary`, after `is_remote: boolean;`:

```ts
  tokens_per_second: number | null;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `npx vitest run src/lib/utils/generationStats.test.ts`
Expected: PASS. Also run `npm run check` — expect type errors in `recordings.test.ts` / nowhere else yet (`makeSummary` missing the new field is caught in Task 10; if `npm run check` flags it now, that's expected until Task 10).

- [ ] **Step 5: Commit**

```bash
git add src/lib/types/index.ts src/lib/utils/generationStats.ts src/lib/utils/generationStats.test.ts
git commit -m "feat(web): mirror generation stats types + latestTokensPerSecond helper"
```

---

### Task 9: `formatTokensPerSecond` helper

**Files:**
- Modify: `src/lib/utils/format.ts` (append)
- Modify: `src/lib/utils/format.test.ts` (append)

- [ ] **Step 1: Write the failing tests**

Append to `src/lib/utils/format.test.ts` (and add `formatTokensPerSecond` to the import on line 2):

```ts
describe('formatTokensPerSecond', () => {
  it('formats null/undefined as empty string', () => {
    expect(formatTokensPerSecond(null)).toBe('');
    expect(formatTokensPerSecond(undefined)).toBe('');
  });

  it('formats small values with one decimal', () => {
    expect(formatTokensPerSecond(41.52)).toBe('41.5 tok/s');
    expect(formatTokensPerSecond(7.25)).toBe('7.3 tok/s');
  });

  it('formats values of 100 or more with no decimals', () => {
    expect(formatTokensPerSecond(99.9)).toBe('99.9 tok/s');
    expect(formatTokensPerSecond(100)).toBe('100 tok/s');
    expect(formatTokensPerSecond(1234.56)).toBe('1235 tok/s');
  });

  it('rejects non-finite and negative values', () => {
    expect(formatTokensPerSecond(Number.NaN)).toBe('');
    expect(formatTokensPerSecond(Number.POSITIVE_INFINITY)).toBe('');
    expect(formatTokensPerSecond(-5)).toBe('');
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `npx vitest run src/lib/utils/format.test.ts`
Expected: FAIL (`formatTokensPerSecond is not defined` / import error).

- [ ] **Step 3: Write the implementation**

Append to `src/lib/utils/format.ts`:

```ts
/** Format a tokens-per-second value as a compact badge label. */
export function formatTokensPerSecond(v: number | null | undefined): string {
  if (v === null || v === undefined || !Number.isFinite(v) || v < 0) return '';
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} tok/s`;
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `npx vitest run src/lib/utils/format.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/lib/utils/format.ts src/lib/utils/format.test.ts
git commit -m "feat(web): formatTokensPerSecond helper"
```

---

### Task 10: RecordingCard badge + store search mapping

**Files:**
- Modify: `src/lib/components/RecordingCard.svelte` (imports line 3, derived state ~line 42, aria-label line 57, meta row lines 71-75)
- Create: `src/lib/components/RecordingCard.test.ts`
- Modify: `src/lib/stores/recordings.svelte.ts` (import; `search()` mapping ~line 100)
- Modify: `src/lib/stores/recordings.test.ts` (`makeSummary` ~line 14)

- [ ] **Step 1: Write the failing component test**

Create `src/lib/components/RecordingCard.test.ts`:

```ts
// @vitest-environment jsdom
/**
 * RecordingCard — component-level render tests for the tokens-per-second
 * meta-row display. The card has no backend/store dependencies, so no
 * module mocks are needed — only direct prop rendering.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/svelte';
import RecordingCard from './RecordingCard.svelte';
import type { RecordingSummary } from '../types';

function makeSummary(overrides: Partial<RecordingSummary> = {}): RecordingSummary {
  return {
    id: 'r1',
    filename: 'consult.wav',
    patient_name: null,
    status: { status: 'completed', completed_at: '2026-08-16T00:00:00Z' },
    duration_seconds: 61,
    created_at: '2026-08-16T00:00:00Z',
    tags: [],
    has_transcript: true,
    has_soap_note: true,
    has_referral: false,
    has_letter: false,
    has_peer_discussion: false,
    is_remote: false,
    tokens_per_second: null,
    ...overrides,
  };
}

afterEach(cleanup);

describe('RecordingCard — tokens per second', () => {
  it('renders tokens per second in the meta row when present', () => {
    render(RecordingCard, { recording: makeSummary({ tokens_per_second: 41.52 }) });
    expect(screen.getByText('41.5 tok/s')).toBeTruthy();
  });

  it('hides the tokens-per-second span when null', () => {
    render(RecordingCard, { recording: makeSummary() });
    expect(screen.queryByText(/tok\/s/)).toBeNull();
  });

  it('includes throughput in the aria-label when present', () => {
    render(RecordingCard, { recording: makeSummary({ tokens_per_second: 41.52 }) });
    const label = screen.getByRole('button').getAttribute('aria-label') ?? '';
    expect(label).toContain('41.5 tok/s');
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/lib/components/RecordingCard.test.ts`
Expected: FAIL — `Unable to find element with text '41.5 tok/s'`.

- [ ] **Step 3: Write the implementation**

Edits in `RecordingCard.svelte`:

1. Import (line 3):

```ts
  import { formatDate, formatDuration, formatTokensPerSecond } from '../utils/format';
```

2. Derived state — replace `const displayName = $derived(...)` (line 42) with:

```ts
  const displayName = $derived(recording.patient_name ?? recording.filename);
  const tpsLabel = $derived(formatTokensPerSecond(recording.tokens_per_second));
  const ariaLabel = $derived(
    `Recording: ${displayName}, ${statusLabel(recording.status)}` +
      (tpsLabel ? `, ${tpsLabel} generation speed` : '')
  );
```

3. Use it in the card's `aria-label` attribute (line 57): replace

```svelte
  aria-label="Recording: {displayName}, {statusLabel(recording.status)}"
```

with

```svelte
  aria-label={ariaLabel}
```

4. Meta row (lines 71-75) — replace

```svelte
    <div class="card-meta">
      <span>{formatDate(recording.created_at)}</span>
      <span class="sep">·</span>
      <span>{formatDuration(recording.duration_seconds)}</span>
    </div>
```

with

```svelte
    <div class="card-meta">
      <span>{formatDate(recording.created_at)}</span>
      <span class="sep">·</span>
      <span>{formatDuration(recording.duration_seconds)}</span>
      {#if tpsLabel}
        <span class="sep">·</span>
        <span title="Latest AI generation speed (tokens/sec)">{tpsLabel}</span>
      {/if}
    </div>
```

Store mapping — in `src/lib/stores/recordings.svelte.ts`:

1. Add to the imports at the top: `import { latestTokensPerSecond } from '../utils/generationStats';`
2. In `search()`, inside the `summaries` map literal, after `is_remote: r.metadata?.synced_from != null,` add:

```ts
          tokens_per_second: latestTokensPerSecond(r.metadata),
```

Test factory — in `src/lib/stores/recordings.test.ts` `makeSummary()`, add after `is_remote: false,`:

```ts
    tokens_per_second: null,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `npx vitest run`
Expected: PASS (new card tests + all existing suites, including the updated recordings store tests).

- [ ] **Step 5: Commit**

```bash
git add src/lib/components/RecordingCard.svelte src/lib/components/RecordingCard.test.ts src/lib/stores/recordings.svelte.ts src/lib/stores/recordings.test.ts
git commit -m "feat(web): show tokens/sec on recording cards + search mapping"
```

---

### Task 11: Full verification gates

**Files:** none (verification only)

- [ ] **Step 1: Rust gates**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
```

Expected: fmt clean, clippy zero warnings, all tests pass (AGENTS.md: integration-test crates like `medical-db`/`medical-sharing` are not covered by `--lib`; the feature adds no integration tests, so the lib gate is sufficient).

- [ ] **Step 2: Frontend gates**

```bash
npx vitest run
npm run check
npm run lint
```

Expected: all green.

- [ ] **Step 3: Manual smoke check (optional but recommended)**

Run `npm run tauri dev` with Ollama or LM Studio loaded; generate a SOAP note for any recording; confirm the Recordings tab entry now shows `NN.N tok/s` after the duration.

- [ ] **Step 4: Final commit (if fmt/clippy touched anything)**

```bash
git add -A
git commit -m "chore: tokens-per-second feature gate fixes" --allow-empty
```

Then merge the worktree branch per the usual flow (PR against `master`).
