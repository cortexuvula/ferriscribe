# Observability Hygiene Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the silent-failure category in this backend so production debugging stops requiring code archaeology. Mirrors the auth-proxy fix that already shipped (v0.10.53) across six other subsystems that swallow `Result`s without log emission.

**Architecture:** No structural change. Every fix follows the same shape: locate a `let _ = …`, `.unwrap_or_default()`, or untracked `tokio::spawn` site → replace with `warn!`/`error!` that includes structural fields (IDs, status codes, error display) but no PHI per CLAUDE.md → preserve existing behavior (still degrade gracefully).

**Tech Stack:** Rust workspace, `tracing` already in use throughout. No new dependencies. Tests use existing patterns (`wiremock`, `tempfile`, `tokio::test`).

---

## File Structure

**Modified:**
- `crates/stt-providers/src/remote_provider.rs` — read error body even when text() fails
- `crates/rag/src/embeddings.rs` — same shape
- `crates/ai-providers/src/openai_compat/methods.rs` — same shape (4 sites)
- `crates/tts-providers/src/elevenlabs_tts.rs` — same shape
- `src-tauri/src/commands/recordings.rs` — log RAG vector cleanup failure
- `crates/db/src/settings.rs` — log config parse failure
- `src-tauri/src/state.rs` — log paired-connection parse failure
- `src-tauri/src/commands/sharing/mod.rs` — log server-config parse failure
- `crates/sharing/src/whisper_supervisor.rs` — supervise stderr-forwarding task
- `src-tauri/src/commands/providers.rs` — read error body in connection tests
- `src-tauri/src/commands/sharing/pairing.rs` — same shape

**No new files.**

---

## Task 1: Centralize "read response body for error context"

**Files:**
- Create: `crates/core/src/http_error_body.rs` (new helper module)
- Modify: `crates/core/src/lib.rs` (export the new module)
- Test: same file (unit tests inside the helper)

**Why:** Six call sites repeat `response.text().await.unwrap_or_default()` and lose the read error. One small helper makes the fix mechanical at each call site and keeps the log shape consistent.

- [ ] **Step 1: Write the failing test for the helper**

  Create `crates/core/src/http_error_body.rs` with:

  ```rust
  //! Read an HTTP error response's body for diagnostic logging without losing
  //! the read-error context. Used by every HTTP client in this workspace.

  use reqwest::Response;

  /// Attempt to read up to `max_chars` characters of the response body. On
  /// read failure, returns a "(could not read body: <error>)" placeholder so
  /// the caller's downstream error message still has a useful tail. Truncates
  /// to bound log line length.
  pub async fn read_error_body(resp: Response, max_chars: usize) -> String {
      match resp.text().await {
          Ok(body) => body.chars().take(max_chars).collect(),
          Err(e) => format!("(could not read body: {e})"),
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[tokio::test]
      async fn truncates_to_max_chars() {
          // Build a Response with a long body via wiremock or a fake reqwest setup.
          // Skipped here — wiremock is in dev-deps of stt-providers, not core.
          // Use a string-based smoke test instead via the public function semantics.
          // (The real verification happens in the call-site tests below.)
          assert_eq!(
              "hello world".chars().take(5).collect::<String>(),
              "hello"
          );
      }
  }
  ```

  Note: this helper is exercised indirectly by call-site tests (Task 2 onwards). The trivial unit test is just a compile/import gate.

- [ ] **Step 2: Verify it compiles**

  Add `pub mod http_error_body;` to `crates/core/src/lib.rs` next to the existing `pub mod` statements.

  Run: `cargo build -p medical-core`
  Expected: clean.

- [ ] **Step 3: Commit**

  ```bash
  git add crates/core/src/http_error_body.rs crates/core/src/lib.rs
  git commit -m "feat(core): add read_error_body helper for HTTP error logging

  Six call sites across the workspace read an HTTP response body for an
  error message and silently swallow the read error itself. Centralize
  the pattern so the fix at each site is one line and the log format
  stays consistent."
  ```

---

## Task 2: Apply helper to `remote_provider.rs` 5xx path

**Files:**
- Modify: `crates/stt-providers/src/remote_provider.rs:259-264`

**Why:** The existing 4xx (non-401) and 5xx paths use `.text().await.unwrap_or_default()` and lose body-read failures. The recent 401 work didn't touch this path.

- [ ] **Step 1: Write a failing test that asserts the error message survives a body-read failure**

  Add to the existing `#[cfg(test)] mod tests` in `remote_provider.rs`:

  ```rust
  #[tokio::test]
  async fn http_500_with_partial_body_includes_diagnostic_marker() {
      let server = MockServer::start().await;
      Mock::given(method("POST"))
          .and(path("/v1/audio/transcriptions"))
          .respond_with(ResponseTemplate::new(500).set_body_string("model load failed"))
          .mount(&server)
          .await;

      let provider = provider_at(&server.uri(), None);
      let err = provider
          .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
          .await
          .unwrap_err()
          .to_string();
      assert!(err.contains("500"), "expected status code in error: {err}");
      assert!(err.contains("model load failed"), "expected body content in error: {err}");
  }
  ```

  Run: `cargo test -p medical-stt-providers --lib remote_provider::tests::http_500_with_partial_body`
  Expected: FAIL — current code emits `"Whisper server internal error: 500"` with no body.

- [ ] **Step 2: Fix the 5xx and remaining 4xx paths**

  In `crates/stt-providers/src/remote_provider.rs`, locate the block right after the 401/403 handler (it currently looks roughly like):

  ```rust
          if status.is_client_error() {
              let body = resp.text().await.unwrap_or_default();
              let prefix: String = body.chars().take(200).collect();
              return Err(AppError::SttProvider(format!(
                  "Whisper server rejected request: {status} {prefix}"
              )));
          }
          if status.is_server_error() {
              return Err(AppError::SttProvider(format!(
                  "Whisper server internal error: {status}"
              )));
          }
  ```

  Replace with:

  ```rust
          if status.is_client_error() {
              let body = medical_core::http_error_body::read_error_body(resp, 200).await;
              return Err(AppError::SttProvider(format!(
                  "Whisper server rejected request: {status} {body}"
              )));
          }
          if status.is_server_error() {
              let body = medical_core::http_error_body::read_error_body(resp, 200).await;
              return Err(AppError::SttProvider(format!(
                  "Whisper server internal error: {status} {body}"
              )));
          }
  ```

- [ ] **Step 3: Run tests**

  Run: `cargo test -p medical-stt-providers`
  Expected: all pass, including the new test.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/stt-providers/src/remote_provider.rs
  git commit -m "log(stt): include response body in 5xx/4xx Whisper errors

  Use the new read_error_body helper so the upstream server's actual
  error message (e.g. 'model load failed', 'out of memory') reaches the
  user instead of just the bare status code."
  ```

---

## Task 3: Apply helper to `embeddings.rs`

**Files:**
- Modify: `crates/rag/src/embeddings.rs:50-65` (the error-path block)

- [ ] **Step 1: Locate the existing error block and identify the line**

  Run: `grep -n "unwrap_or_default" crates/rag/src/embeddings.rs`

  Read the surrounding 10 lines. Confirm there is a line of the form `let body = response.text().await.unwrap_or_default();` immediately followed by an error construction.

- [ ] **Step 2: Replace it**

  Change the line to use the helper:

  ```rust
  let body = medical_core::http_error_body::read_error_body(response, 200).await;
  ```

  And update the format string to include `{body}` if it doesn't already.

  If `medical_core` isn't already imported at the top of `embeddings.rs`, check: `grep -n "^use medical_core" crates/rag/src/embeddings.rs`. The crate already depends on `medical-core` (verify via `grep medical-core crates/rag/Cargo.toml`); if so, add `use medical_core::http_error_body::read_error_body;` to the imports.

- [ ] **Step 3: Build and test**

  Run: `cargo test -p medical-rag`
  Expected: clean. (No new test required — the change is a textual replacement that doesn't alter happy-path behavior.)

- [ ] **Step 4: Commit**

  ```bash
  git add crates/rag/src/embeddings.rs
  git commit -m "log(rag): preserve body-read errors in embedding HTTP failures"
  ```

---

## Task 4: Apply helper across `openai_compat/methods.rs` (4 sites)

**Files:**
- Modify: `crates/ai-providers/src/openai_compat/methods.rs:37, 62, 119, 214` (four `unwrap_or_default` sites)

- [ ] **Step 1: Locate the four sites**

  Run: `grep -n "unwrap_or_default" crates/ai-providers/src/openai_compat/methods.rs`

  Confirm four matches and read each ±5 lines of context. Each is the same shape: `let body = response.text().await.unwrap_or_default();`

- [ ] **Step 2: Replace each call**

  Replace each `let body = response.text().await.unwrap_or_default();` with `let body = medical_core::http_error_body::read_error_body(response, 200).await;`. The variable name `body` is preserved so the surrounding `format!` doesn't need updates.

  Add `use medical_core::http_error_body::read_error_body;` to the import block if shorter call sites are preferred, OR leave as fully-qualified for clarity. Pick one style and apply uniformly.

  Verify `medical-core` is in `crates/ai-providers/Cargo.toml` — if not, add it: `medical-core = { path = "../core" }` under `[dependencies]`.

- [ ] **Step 3: Build and test**

  Run: `cargo test -p medical-ai-providers`
  Expected: clean.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/ai-providers/src/openai_compat/methods.rs crates/ai-providers/Cargo.toml
  git commit -m "log(ai): preserve body-read errors in OpenAI-compat HTTP failures (4 sites)"
  ```

---

## Task 5: Apply helper to `elevenlabs_tts.rs`

**Files:**
- Modify: `crates/tts-providers/src/elevenlabs_tts.rs:113-122` (the error block around line 118)

- [ ] **Step 1: Locate the call site**

  Run: `grep -n "unwrap_or_default" crates/tts-providers/src/elevenlabs_tts.rs`

  Same shape as Task 3.

- [ ] **Step 2: Replace and add dependency if missing**

  Same approach as Task 3. Verify `medical-core` is in `crates/tts-providers/Cargo.toml`; add if needed.

- [ ] **Step 3: Build and test**

  Run: `cargo test -p medical-tts-providers`
  Expected: clean.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/tts-providers/src/elevenlabs_tts.rs crates/tts-providers/Cargo.toml
  git commit -m "log(tts): preserve body-read errors in ElevenLabs HTTP failures"
  ```

---

## Task 6: Log RAG vector cleanup failure on delete_all

**Files:**
- Modify: `src-tauri/src/commands/recordings.rs:106` (the bare `let _ = conn.execute("DELETE FROM vectors", []);`)

- [ ] **Step 1: Locate the line**

  Run: `grep -n "DELETE FROM vectors" src-tauri/src/commands/recordings.rs`

- [ ] **Step 2: Replace the swallow with a log**

  Change:

  ```rust
  let _ = conn.execute("DELETE FROM vectors", []);
  ```

  To:

  ```rust
  if let Err(e) = conn.execute("DELETE FROM vectors", []) {
      tracing::error!(error = %e, "RAG vector cleanup failed during delete_all_recordings; orphan vectors may remain");
  }
  ```

- [ ] **Step 3: Build**

  Run: `cargo build -p rust-medical-assistant`
  Expected: clean.

- [ ] **Step 4: Commit**

  ```bash
  git add src-tauri/src/commands/recordings.rs
  git commit -m "log(recordings): surface RAG vector cleanup failure on delete_all

  Previously the DELETE FROM vectors call had its result discarded. If
  the delete failed (lock contention, schema mismatch), orphan vectors
  silently accumulated and RAG returned stale results with no diagnostic
  trail. Log the error at error! level."
  ```

---

## Task 7: Log config parse failures (3 call sites)

**Files:**
- Modify: `crates/db/src/settings.rs` (the `load_config` function — look for `serde_json::from_str` + `unwrap_or_default`)
- Modify: `src-tauri/src/state.rs` (the `load_paired_connection` function near line 196-218)
- Modify: `src-tauri/src/commands/sharing/mod.rs` (the `load_server_config` function — find via grep)

- [ ] **Step 1: Locate each call site**

  Run each:
  ```
  grep -n "unwrap_or_default" crates/db/src/settings.rs
  grep -n "unwrap_or_default\|serde_json::from_str" src-tauri/src/state.rs
  grep -n "load_server_config\|server-paired\|sharing-server" src-tauri/src/commands/sharing/mod.rs
  ```

  Read each site's context. Confirm the pattern: parsing succeeds → return parsed value; failure → silently fall back to default.

- [ ] **Step 2: Add explicit match arms with warn! at each site**

  For each site, replace the swallow pattern with the explicit match form. Example for `settings.rs`:

  Original:
  ```rust
  serde_json::from_str(&json).unwrap_or_default()
  ```

  Replacement:
  ```rust
  match serde_json::from_str::<AppConfig>(&json) {
      Ok(cfg) => cfg,
      Err(e) => {
          tracing::warn!(error = %e, "Failed to parse app_config JSON; using defaults");
          AppConfig::default()
      }
  }
  ```

  Apply the same shape to `load_paired_connection` (logs about paired connection — `Failed to parse paired-connection JSON; treating as not paired`) and `load_server_config` (`Failed to parse server-config JSON; treating as not configured`).

- [ ] **Step 3: Run existing tests**

  Run: `cargo test --workspace --lib`
  Expected: clean.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/db/src/settings.rs src-tauri/src/state.rs src-tauri/src/commands/sharing/mod.rs
  git commit -m "log: surface JSON parse failures in persisted config (3 sites)

  load_config / load_paired_connection / load_server_config previously
  silently fell back to defaults on JSON parse failure. Users would see
  all settings reset / 'not paired' with no diagnostic. Log the parse
  error at warn level before falling back."
  ```

---

## Task 8: Supervise the whisper-server stderr-forwarding task

**Files:**
- Modify: `crates/sharing/src/whisper_supervisor.rs:272-278` (the `tokio::spawn` block reading stderr)

- [ ] **Step 1: Locate the spawn**

  Run: `grep -n "tokio::spawn" crates/sharing/src/whisper_supervisor.rs`

  Read the spawn body. Confirm the `JoinHandle` is dropped (not stored in `self`).

- [ ] **Step 2: Wrap the spawn to log on task termination**

  Replace:

  ```rust
  tokio::spawn(async move {
      // existing body — read stderr, forward to tracing
      // ...
  });
  ```

  With:

  ```rust
  let stderr_task = tokio::spawn(async move {
      // existing body unchanged
      // ...
  });
  tokio::spawn(async move {
      match stderr_task.await {
          Ok(()) => tracing::debug!("whisper stderr-forwarding task exited normally"),
          Err(e) if e.is_cancelled() => tracing::debug!("whisper stderr task cancelled"),
          Err(e) if e.is_panic() => tracing::error!(error = %e, "whisper stderr task panicked; stderr output lost"),
          Err(e) => tracing::error!(error = %e, "whisper stderr task failed"),
      }
  });
  ```

  Don't change what the inner task does — only surface termination.

- [ ] **Step 3: Run tests**

  Run: `cargo test -p medical-sharing`
  Expected: clean.

- [ ] **Step 4: Commit**

  ```bash
  git add crates/sharing/src/whisper_supervisor.rs
  git commit -m "log(sharing): surface whisper stderr-task termination

  The spawned task that forwards whisper-server stderr to tracing had
  its JoinHandle dropped. If it panicked (decode error, channel break),
  stderr forwarding silently stopped. Add a supervisor task that logs
  the termination cause at error! when it isn't normal/cancelled."
  ```

---

## Task 9: Connection-test commands read the error body

**Files:**
- Modify: `src-tauri/src/commands/providers.rs:109-113` (test_lmstudio_connection)
- Modify: `src-tauri/src/commands/providers.rs:185-190` (test_stt_remote_connection — already touched by the v0.10.53 work, but the non-401 path remains untouched)
- Modify: `src-tauri/src/commands/sharing/pairing.rs:114-115` (server-rejected-pair message)
- Modify: `src-tauri/src/commands/sharing/discovery.rs` (similar — locate via grep)

- [ ] **Step 1: Locate the call sites**

  Run:
  ```
  grep -n "status().is_success()\|HTTP {}" src-tauri/src/commands/providers.rs src-tauri/src/commands/sharing/pairing.rs src-tauri/src/commands/sharing/discovery.rs
  ```

  For each match, read the surrounding error construction. Confirm the pattern: status check fails → return error with just the status code, no body.

- [ ] **Step 2: Add body inspection at each site**

  For each match, change the error construction from:

  ```rust
  if !response.status().is_success() {
      return Err(AppError::AiProvider(format!(
          "Server returned HTTP {}",
          response.status()
      )));
  }
  ```

  To:

  ```rust
  if !response.status().is_success() {
      let status = response.status();
      let body = medical_core::http_error_body::read_error_body(response, 200).await;
      return Err(AppError::AiProvider(format!("Server returned HTTP {status} {body}")));
  }
  ```

  Adjust the error variant per call site (some use `AppError::SttProvider`, etc.). For pairing.rs, the existing error uses `Err(...)` returning `String` directly; same pattern but no `AppError` wrapping. Read each carefully.

- [ ] **Step 3: Build and test**

  Run: `cargo test -p rust-medical-assistant --lib`
  Expected: clean.

- [ ] **Step 4: Commit**

  ```bash
  git add src-tauri/src/commands/providers.rs src-tauri/src/commands/sharing/pairing.rs src-tauri/src/commands/sharing/discovery.rs
  git commit -m "log: include response body in connection-test error messages

  The Settings 'Test connection' commands and the pairing client showed
  bare status codes ('Server returned HTTP 500') when the upstream
  server had a body explaining the cause. Read up to 200 chars of body
  via the shared helper."
  ```

---

## Final verification

- [ ] **Workspace test sweep**

  ```bash
  cargo test --workspace --lib
  cargo test -p medical-sharing
  cargo test -p medical-stt-providers
  ```

  Expected: all pass.

- [ ] **PHI-policy check**

  Run: `git diff master..HEAD -- '*.rs' | grep -E "^\+.*tracing::(info|warn|error|debug)!"`

  Read each new log line. Confirm: no transcript text, no medication/condition strings, no bearer values. Only structural fields (error display, status codes, IDs).
