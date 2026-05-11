# PHI Leak Fix & Shared HTTP Client Follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close two follow-up items surfaced during the v0.10.54 review pass: a pre-existing PHI leak in the AI-completion error path, and two remaining per-request `reqwest::Client::new()` sites that should reuse the shared `Arc<reqwest::Client>` introduced in Plan C.

**Architecture:** Two independent fixes. Task 1 removes a log field that embeds up to 500 bytes of AI response body (which contains patient data when the active provider is generating SOAP notes); replace with a length-only marker. The same code also has a latent byte-index panic on UTF-8 boundaries — fix while we're here. Task 2 migrates `TemplatesRemote` and `VocabRemote` to accept a `&Arc<reqwest::Client>` instead of constructing a fresh client per call, matching the pattern already established for the connection-test commands in Plan C.

**Tech Stack:** Rust workspace, existing infrastructure. No new dependencies.

---

## File Structure

**Modified:**
- `crates/ai-providers/src/openai_compat/methods.rs` — replace the `body_preview` field in the JSON-parse-error `warn!` with `body_len`; remove the now-unused panic-prone byte slice
- `src-tauri/src/templates_remote.rs` — constructor accepts shared client; drop per-call `Self::client()` helper
- `src-tauri/src/vocab_remote.rs` — same as templates_remote.rs
- `src-tauri/src/commands/context_templates.rs` (or wherever `TemplatesRemote::from` is called) — pass `&state.http_client` to the constructor
- `src-tauri/src/commands/vocabulary.rs` (or wherever `VocabRemote::from` is called) — same

**No new files.**

---

## Task 1: Fix PHI leak (and latent UTF-8 panic) in AI parse-error log

**Files:**
- Modify: `crates/ai-providers/src/openai_compat/methods.rs:79-83`

**Why:**
The current code:

```rust
let resp: ChatResponse = serde_json::from_str(&raw_body)
    .map_err(|e| {
        warn!(body_preview = &raw_body[..raw_body.len().min(500)], "Failed to parse AI response JSON");
        AppError::AiProvider(format!("JSON parse error: {e}"))
    })?;
```

has two problems:

1. **PHI leak:** `raw_body` is the body of a successful HTTP response from the local AI provider (Ollama, LM Studio). When the active task is SOAP/letter/synopsis generation, that body contains the AI's response derived from a patient transcript. Logging up to 500 bytes of it as a tracing field violates the project's hard rule (CLAUDE.md): "Patient transcripts, SOAP content, medications, allergies, and conditions must never appear in `tracing::*` macros." The fact that this fires only on JSON parse failure doesn't excuse it — when it fires (malformed JSON from a misbehaving provider), the body has whatever the provider was about to send, which may include partial SOAP content.

2. **Latent panic:** `&raw_body[..raw_body.len().min(500)]` is byte-indexed. If `raw_body` is 501+ bytes and the 500th byte falls in the middle of a multi-byte UTF-8 codepoint (common for non-ASCII content), this slice will panic with `byte index N is not a char boundary`. The fix is to use `.chars().take(N).collect::<String>()` if we wanted to keep a preview, but per problem 1 we shouldn't keep a preview at all.

The right fix kills both birds: replace `body_preview` with `body_len` (a `usize` — no content, no panic potential).

- [ ] **Step 1: Locate the call site**

  Run: `grep -n "body_preview" crates/ai-providers/src/openai_compat/methods.rs`

  Expected: one match around line 81 inside the `complete()` method's JSON-parse error closure.

- [ ] **Step 2: Make the change**

  Replace:

  ```rust
  let resp: ChatResponse = serde_json::from_str(&raw_body)
      .map_err(|e| {
          warn!(body_preview = &raw_body[..raw_body.len().min(500)], "Failed to parse AI response JSON");
          AppError::AiProvider(format!("JSON parse error: {e}"))
      })?;
  ```

  With:

  ```rust
  let resp: ChatResponse = serde_json::from_str(&raw_body)
      .map_err(|e| {
          warn!(
              body_len = raw_body.len(),
              "Failed to parse AI response JSON"
          );
          AppError::AiProvider(format!("JSON parse error: {e}"))
      })?;
  ```

  Operators can still tell from `body_len` whether the response was empty (0), suspiciously small (<50, suggests truncation), or full-sized (1k+, suggests a serde shape mismatch).

- [ ] **Step 3: Run the ai-providers tests**

  Run: `cargo test -p medical-ai-providers`

  Expected: 49+ pass with no regressions. The change doesn't affect observable behavior on either path — error message text is unchanged, parse logic is unchanged.

- [ ] **Step 4: Verify the PHI-leak grep is now clean**

  Run: `grep -rn "body_preview" crates/ src-tauri/src/ --include="*.rs"`

  Expected: zero matches. (If any remain, they're either tests or new sites — confirm by reading each.)

- [ ] **Step 5: Commit**

  ```bash
  git add crates/ai-providers/src/openai_compat/methods.rs
  git commit -m "fix(ai): drop body content from JSON-parse-error log + remove latent UTF-8 panic

  The JSON-parse-error path in OpenAiCompatibleClient::complete() logged
  up to 500 bytes of the AI response body via body_preview. For local
  AI providers generating SOAP notes, that body contains patient data —
  a violation of the project's no-PHI-in-logs rule (CLAUDE.md).

  The same byte slice (&raw_body[..raw_body.len().min(500)]) was also
  byte-indexed: a multi-byte UTF-8 codepoint at byte 500 would panic
  with 'not a char boundary'.

  Replace body_preview with body_len (a usize — no content, no panic
  potential). Operators can still distinguish empty / truncated / full
  responses without exposing patient data."
  ```

---

## Task 2: Migrate `TemplatesRemote` and `VocabRemote` to the shared HTTP client

**Files:**
- Modify: `src-tauri/src/templates_remote.rs` (constructor signature + drop `Self::client()` helper)
- Modify: `src-tauri/src/vocab_remote.rs` (same shape)
- Modify: All call sites of `TemplatesRemote::from(...)` and `VocabRemote::from(...)` in `src-tauri/src/commands/` — pass `&state.http_client`

**Why:**
Both files currently have a private `Self::client()` method:

```rust
fn client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Other(format!("templates_remote http client: {e}")))
}
```

Every method calls it (`Self::client()?.get(...).send()`), so every list/upsert/delete/etc. allocates a new `reqwest::Client` (with its own TLS context and connection pool) per call. The shared `Arc<reqwest::Client>` on `AppState` (introduced in Plan C, commit `8ef543e`) was designed for exactly this case. The same pattern Plan C applied to the connection-test commands belongs here too.

Note: the existing per-call client used a 5s connect / 15s total. The shared client uses 10s connect / 30s total. Most of these calls are quick list/upsert operations, so applying a per-request `.timeout(Duration::from_secs(15))` to preserve the previous total bound is appropriate.

- [ ] **Step 1: Locate all `TemplatesRemote::from` and `VocabRemote::from` call sites**

  Run:
  ```
  grep -rn "TemplatesRemote::from\|VocabRemote::from" src-tauri/src --include="*.rs"
  ```

  Expected: a handful of call sites in `src-tauri/src/commands/context_templates.rs` and `src-tauri/src/commands/vocabulary.rs` (file names may differ — confirm by reading each match's surrounding 5 lines).

- [ ] **Step 2: Modify `TemplatesRemote` constructor and methods**

  In `src-tauri/src/templates_remote.rs`:

  Change the struct definition (around line 16):

  ```rust
  pub struct TemplatesRemote<'a> {
      pub conn: &'a PairedConnection,
      pub bearer: String,
      pub client: std::sync::Arc<reqwest::Client>,
  }
  ```

  Change the `from` constructor (around line 21):

  ```rust
  impl<'a> TemplatesRemote<'a> {
      pub fn from(
          conn: &'a PairedConnection,
          bearer: Option<String>,
          client: std::sync::Arc<reqwest::Client>,
      ) -> Option<Self> {
          let bearer = bearer?;
          conn.ports.vocab?;
          Some(Self { conn, bearer, client })
      }

      // ... base_url stays unchanged ...
  ```

  Remove the `fn client() -> AppResult<reqwest::Client>` helper (around lines 41-47).

  Replace every `Self::client()?.get(...)` (and `.post(...)`, `.put(...)`, `.delete(...)`) with `self.client.<method>(...).timeout(std::time::Duration::from_secs(15))`. The per-request `.timeout()` override preserves the previous 15s total bound.

  Example transformation, for the `list` method:

  Before:
  ```rust
  let resp = Self::client()?
      .get(&url)
      .bearer_auth(&self.bearer)
      .send()
      .await
      .map_err(|e| AppError::Other(format!("templates list: {e}")))?;
  ```

  After:
  ```rust
  let resp = self.client
      .get(&url)
      .timeout(std::time::Duration::from_secs(15))
      .bearer_auth(&self.bearer)
      .send()
      .await
      .map_err(|e| AppError::Other(format!("templates list: {e}")))?;
  ```

  Apply this transformation to every method in the file (`list`, `upsert`, and any others — read the file to enumerate).

- [ ] **Step 3: Apply the identical changes to `VocabRemote`**

  In `src-tauri/src/vocab_remote.rs`, make the same shape of changes:
  - Add `client: std::sync::Arc<reqwest::Client>` field
  - Constructor accepts `client: std::sync::Arc<reqwest::Client>` parameter
  - Remove `fn client()` helper
  - Replace every `Self::client()?` call with `self.client` + per-request timeout

  Use the same 15s per-request timeout (matches the previous per-call default).

- [ ] **Step 4: Update all call sites to pass `&state.http_client`**

  At each `TemplatesRemote::from(...)` and `VocabRemote::from(...)` call site found in Step 1, add the new argument. Example:

  Before:
  ```rust
  let remote = TemplatesRemote::from(&conn, bearer);
  ```

  After:
  ```rust
  let remote = TemplatesRemote::from(&conn, bearer, state.http_client.clone());
  ```

  Tauri command functions that don't already take `state: tauri::State<'_, AppState>` will need to add it as the first parameter. Check each call site before assuming.

- [ ] **Step 5: Build the workspace**

  Run: `cargo build -p rust-medical-assistant`

  Expected: clean. Compile errors usually mean a missing `state: tauri::State` parameter on a command — fix and re-run.

- [ ] **Step 6: Run tests**

  Run: `cargo test --workspace --lib`

  Expected: 43+ pass. (No new tests in this task; existing tests must continue to pass.)

- [ ] **Step 7: Frontend invoke audit**

  Run: `grep -rn "invoke.*template\|invoke.*vocab\|invoke.*list_context_templates\|invoke.*list_vocabulary" src/ --include="*.ts" --include="*.svelte"`

  Confirm each frontend `invoke(...)` call passes only user-facing parameters — no extraneous `state` argument. Tauri injects `state` server-side; the frontend should be unchanged. If any call passes positional arguments that would now conflict, STOP and report.

- [ ] **Step 8: Commit**

  ```bash
  git add src-tauri/src/templates_remote.rs src-tauri/src/vocab_remote.rs src-tauri/src/commands/context_templates.rs src-tauri/src/commands/vocabulary.rs
  git commit -m "refactor: migrate TemplatesRemote and VocabRemote to shared HTTP client

  Both clients had a private Self::client() helper that built a fresh
  reqwest::Client per call. Take an Arc<reqwest::Client> via the
  constructor and reuse it across all methods — same pattern Plan C
  applied to the connection-test commands. Per-request 15s timeout
  preserves the previous per-call default."
  ```

  (Adjust the `git add` list to match the actual files modified. If the command files weren't named `context_templates.rs` / `vocabulary.rs`, the implementer should report the real names in the report.)

---

## Final verification

- [ ] **Workspace test sweep**

  ```bash
  cargo test --workspace --lib
  ```

  Expected: all pass.

- [ ] **PHI-policy grep**

  Run: `grep -rn "body_preview\|patient_transcript\|raw_body\b" crates/ src-tauri/src/ --include="*.rs" | grep -E "tracing::|warn!|info!|error!|debug!"`

  Expected: no lines where any patient-data-bearing string is logged. Read each match if any appear.

- [ ] **Shared-client grep**

  Run: `grep -rn "reqwest::Client::new()\|reqwest::Client::builder()" src-tauri/src --include="*.rs" | grep -v "tests/"`

  Expected: only the AppState constructor remains (in `state.rs`). All command-level call sites should be gone.
