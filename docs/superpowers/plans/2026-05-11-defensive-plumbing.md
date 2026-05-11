# Security & Defensive Plumbing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close five small, mostly independent stability/security gaps surfaced in the 2026-05-11 backend audit. Each fix is targeted (≤30 LOC per task) and behavior-preserving except where the goal is explicitly to fail loudly instead of silently.

**Architecture:** No architectural change. Five separable fixes:
- Master-key derivation refuses to use a guessable fallback string
- `EmbeddingGenerator` HTTP client has connect + request timeouts
- `CancelGuard::drop` logs when the mutex is poisoned instead of silently leaking the entry
- Audio ring-capacity sizing uses `saturating_mul` against exotic device specs
- A shared `Arc<reqwest::Client>` lives on `AppState` and is reused by connection-test commands

**Tech Stack:** Rust workspace. No new dependencies. Tests use existing patterns.

---

## File Structure

**Modified:**
- `crates/security/src/key_storage.rs` — return `Err` instead of falling back to `"fallback"`
- `crates/security/src/lib.rs` — possibly new error variant
- `crates/rag/src/embeddings.rs` — add timeouts on the `Client::builder()`
- `src-tauri/src/commands/pipeline.rs` — log on poisoned cancel-map
- `crates/audio/src/capture.rs` — `saturating_mul` for ring capacity
- `src-tauri/src/state.rs` — add `http_client: Arc<reqwest::Client>` to `AppState`
- `src-tauri/src/commands/providers.rs` — use shared client (3 commands)
- `src-tauri/src/commands/sharing/pairing.rs` — use shared client
- `src-tauri/src/commands/sharing/lifecycle.rs` — use shared client (lmstudio detection)

**No new files.**

---

## Task 1: Master-key derivation refuses guessable fallback

**Files:**
- Modify: `crates/security/src/key_storage.rs:177-184` (the `derive_master_key` function)
- Possibly modify: `crates/security/src/lib.rs` (add a new `SecurityError::NoMasterKey` variant if appropriate)

**Why:** Today if `MEDICAL_ASSISTANT_MASTER_KEY` env var is unset AND `get_machine_id()` returns `Err`, `derive_master_key` quietly uses the literal string `"fallback"` as the PBKDF2 password. Salt is stored next to the encrypted blob, so anyone with disk access can derive the key in O(1). Returning `Err` lets the caller surface a clear "couldn't initialize secure storage" message instead.

Note: This affects `key_storage`-encrypted secrets only (e.g. `stt_remote_api_key`), not the DB (separately keychain-backed).

- [ ] **Step 1: Read the existing error type**

  Read `crates/security/src/lib.rs` and identify the `SecurityError` enum. Decide whether to add a new variant `MasterKeyUnavailable` or reuse an existing one like `KeychainError`. Adding `MasterKeyUnavailable { reason: String }` is cleaner — the caller can surface a specific message.

- [ ] **Step 2: Add the variant (if going that route)**

  In `crates/security/src/lib.rs`, add to `SecurityError`:

  ```rust
  #[error("master key unavailable: {reason}")]
  MasterKeyUnavailable { reason: String },
  ```

- [ ] **Step 3: Write a failing test**

  In `crates/security/src/key_storage.rs`, add a test that uses an explicit "neither env var nor machine id" scenario:

  ```rust
  #[test]
  fn derive_master_key_errors_when_no_source_available() {
      // Save and clear MEDICAL_ASSISTANT_MASTER_KEY for this test scope.
      let prior = std::env::var("MEDICAL_ASSISTANT_MASTER_KEY").ok();
      // SAFETY: test is single-threaded with #[test]; we restore in a guard.
      // SAFETY: This test only works if get_machine_id() can fail. If on
      // this platform machine_id is always available, this test will pass
      // for the wrong reason. Document and accept.
      unsafe { std::env::remove_var("MEDICAL_ASSISTANT_MASTER_KEY"); }

      // Patch get_machine_id() to return Err using a feature-flagged hook
      // OR test the function via a test-only `_with_machine_id_provider`
      // overload (see Step 4).

      // ... see Step 4 for the actual approach.

      if let Some(v) = prior {
          unsafe { std::env::set_var("MEDICAL_ASSISTANT_MASTER_KEY", v); }
      }
  }
  ```

  The direct test is awkward because `get_machine_id` is not easily mockable. Instead, refactor `derive_master_key` to accept a provider closure (Step 4) so the test can inject a failing provider directly.

- [ ] **Step 4: Refactor `derive_master_key` to accept an injected source**

  Replace the existing function with:

  ```rust
  fn derive_master_key(salt: &[u8]) -> SecurityResult<[u8; 32]> {
      derive_master_key_from(salt, || std::env::var("MEDICAL_ASSISTANT_MASTER_KEY").ok(), get_machine_id)
  }

  fn derive_master_key_from<E, M>(
      salt: &[u8],
      env_var: E,
      machine_id: M,
  ) -> SecurityResult<[u8; 32]>
  where
      E: FnOnce() -> Option<String>,
      M: FnOnce() -> SecurityResult<String>,
  {
      let password = match env_var() {
          Some(v) if !v.is_empty() => v,
          _ => machine_id().map_err(|e| SecurityError::MasterKeyUnavailable {
              reason: format!(
                  "MEDICAL_ASSISTANT_MASTER_KEY not set and machine id lookup failed: {e}"
              ),
          })?,
      };

      let mut key = [0u8; 32];
      pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, PBKDF2_ITERATIONS, &mut key);
      Ok(key)
  }
  ```

  Now write the test against `derive_master_key_from`:

  ```rust
  #[test]
  fn derive_master_key_from_errors_when_env_empty_and_machine_id_fails() {
      let result = derive_master_key_from(
          &[0u8; 16],
          || None,
          || Err(SecurityError::Other("simulated".to_string())),
      );
      assert!(matches!(result, Err(SecurityError::MasterKeyUnavailable { .. })));
  }

  #[test]
  fn derive_master_key_from_uses_env_when_present() {
      let result = derive_master_key_from(
          &[0u8; 16],
          || Some("explicit".to_string()),
          || panic!("machine id should not be called"),
      );
      assert!(result.is_ok());
  }
  ```

  (Use whichever existing `SecurityError` variant fits for the simulated machine-id failure — likely `SecurityError::Other` or `SecurityError::Io`.)

- [ ] **Step 5: Run the tests**

  Run: `cargo test -p medical-security`
  Expected: both new tests pass; all existing tests still pass.

- [ ] **Step 6: Verify callers handle the new error**

  Run: `grep -rn "derive_master_key\|KeyStorage::open" crates/ src-tauri/src/ --include="*.rs"`

  Any caller that opens `KeyStorage` already handles `SecurityResult<_>`. Confirm no caller calls `.unwrap()` on the result in production code. If a caller does, replace the unwrap with explicit error propagation.

- [ ] **Step 7: Commit**

  ```bash
  git add crates/security/src/key_storage.rs crates/security/src/lib.rs
  git commit -m "fix(security): refuse guessable 'fallback' password for master key

  When neither MEDICAL_ASSISTANT_MASTER_KEY env nor get_machine_id()
  succeeded, derive_master_key silently used the literal string
  'fallback' as PBKDF2 password. With the salt on disk next to the
  encrypted blob, this made any KeyStorage-protected secret (e.g.
  stt_remote_api_key) trivially decryptable. Return MasterKeyUnavailable
  instead so the caller surfaces a clear error to the user. DB encryption
  is unaffected (uses a separate keychain-backed key)."
  ```

---

## Task 2: HTTP timeouts on the embedding client

**Files:**
- Modify: `crates/rag/src/embeddings.rs` (the `EmbeddingGenerator::new_ollama` constructor — around line 30 where `Client::new()` is called)

**Why:** `EmbeddingGenerator` builds a `reqwest::Client` with no timeout. If Ollama hangs (model loading, GPU lock, network stall), embedding requests hang forever and RAG ingestion stalls.

- [ ] **Step 1: Locate the client construction**

  Run: `grep -n "Client::new\|Client::builder" crates/rag/src/embeddings.rs`

  Read the surrounding 10 lines to confirm this is the constructor body.

- [ ] **Step 2: Replace with a timeout-equipped builder**

  Change:

  ```rust
  let client = reqwest::Client::new();
  ```

  to:

  ```rust
  // Embedding requests are short. Cap connection establishment at 10 s and
  // total request at 120 s — if Ollama is loading a model it can take a
  // while, but indefinite hangs stall RAG ingestion with no progress.
  let client = reqwest::Client::builder()
      .connect_timeout(std::time::Duration::from_secs(10))
      .timeout(std::time::Duration::from_secs(120))
      .build()
      .map_err(|e| AppError::AiProvider(format!("Failed to build embedding HTTP client: {e}")))?;
  ```

  If the constructor doesn't currently return `Result`, you'll need to adjust the signature (or use `.expect("reqwest client builder failed")` — pick based on existing code style in the file).

- [ ] **Step 3: Build**

  Run: `cargo build -p medical-rag`
  Expected: clean.

- [ ] **Step 4: Run tests**

  Run: `cargo test -p medical-rag`
  Expected: pass.

- [ ] **Step 5: Commit**

  ```bash
  git add crates/rag/src/embeddings.rs
  git commit -m "fix(rag): add timeouts to embedding HTTP client

  Client was built via Client::new() with no timeout. If Ollama hangs
  (model load, GPU lock), embedding requests hang indefinitely and RAG
  ingestion stalls with no progress signal. Cap connect at 10 s, total
  at 120 s, matching the pattern used by remote_provider.rs."
  ```

---

## Task 3: Log when `CancelGuard::drop` finds a poisoned mutex

**Files:**
- Modify: `src-tauri/src/commands/pipeline.rs:210-220` (the `CancelGuard::drop` impl)

**Why:** Current code does `if let Ok(mut guard) = ... { guard.remove(&self.key); }`. Poisoning is silently ignored, leaking the cancel-token entry forever — second pipeline run for the same recording_id behaves cryptically.

- [ ] **Step 1: Locate `CancelGuard::drop`**

  Run: `grep -n "impl.*Drop.*CancelGuard\|fn drop" src-tauri/src/commands/pipeline.rs`

  Read the existing impl.

- [ ] **Step 2: Replace the swallow with a logged branch**

  Change:

  ```rust
  impl Drop for CancelGuard {
      fn drop(&mut self) {
          if let Ok(mut guard) = self.cancels.lock() {
              guard.remove(&self.key);
          }
      }
  }
  ```

  To:

  ```rust
  impl Drop for CancelGuard {
      fn drop(&mut self) {
          match self.cancels.lock() {
              Ok(mut guard) => {
                  guard.remove(&self.key);
              }
              Err(poisoned) => {
                  tracing::error!(
                      key = %self.key,
                      "Pipeline cancel-token map is poisoned; entry leaked. \
                       Subsequent pipeline runs for this recording_id may behave incorrectly."
                  );
                  // Best-effort cleanup using the poisoned inner.
                  let mut inner = poisoned.into_inner();
                  inner.remove(&self.key);
              }
          }
      }
  }
  ```

  The `poisoned.into_inner()` cleanup is best-effort — the lock is poisoned because a holder panicked, so internal state may already be inconsistent, but removing the entry is still the right thing to attempt.

- [ ] **Step 3: Build and run tests**

  Run: `cargo test -p rust-medical-assistant --lib`
  Expected: pass.

- [ ] **Step 4: Commit**

  ```bash
  git add src-tauri/src/commands/pipeline.rs
  git commit -m "log(pipeline): surface poisoned cancel-token map in CancelGuard::drop

  Previously silently no-op'd on poison, leaking the entry forever. Log
  an error and attempt best-effort cleanup via into_inner so subsequent
  runs for the same recording_id aren't permanently broken."
  ```

---

## Task 4: `saturating_mul` for audio ring capacity

**Files:**
- Modify: `crates/audio/src/capture.rs:191` (the ring-capacity calculation)

**Why:** `(actual_rate as usize) * (actual_channels as usize) * 2` is fine on typical hardware but a single multiplication-overflow on an exotic device spec (192 kHz × 32 ch ASIO) would produce a tiny buffer and corrupt audio. `saturating_mul` is a one-line defensive replacement.

- [ ] **Step 1: Locate the line**

  Run: `grep -n "actual_rate.*actual_channels\|ring_capacity" crates/audio/src/capture.rs`

  Read the context. Confirm the calculation pattern.

- [ ] **Step 2: Replace the multiplication chain**

  Change:

  ```rust
  let ring_capacity = (actual_rate as usize) * (actual_channels as usize) * 2;
  ```

  To:

  ```rust
  let ring_capacity = (actual_rate as usize)
      .saturating_mul(actual_channels as usize)
      .saturating_mul(2)
      .max(config.buffer_size.saturating_mul(4));
  ```

  The `.max(config.buffer_size * 4)` floor guarantees a minimum capacity even if some adjacent product underflows to a tiny number; preserve the same minimum the original code implies.

  Verify by reading the actual variable name (`config.buffer_size` may differ — adjust to match the existing code).

- [ ] **Step 3: Build**

  Run: `cargo build -p medical-audio`
  Expected: clean.

- [ ] **Step 4: Run tests**

  Run: `cargo test -p medical-audio`
  Expected: pass.

- [ ] **Step 5: Commit**

  ```bash
  git add crates/audio/src/capture.rs
  git commit -m "fix(audio): saturating_mul for ring capacity to defend against device overflow

  (actual_rate * actual_channels * 2) is safe on typical hardware but
  an exotic ASIO device reporting very high sample rate × channels
  could overflow usize, producing a tiny buffer. saturating_mul is a
  one-line defensive fix."
  ```

---

## Task 5: Shared `Arc<reqwest::Client>` on AppState

**Files:**
- Modify: `src-tauri/src/state.rs` — add `pub http_client: Arc<reqwest::Client>` to `AppState`
- Modify: `src-tauri/src/commands/providers.rs` — three `test_*_connection` commands (lines 87, 151, 224)
- Modify: `src-tauri/src/commands/sharing/pairing.rs:107` — pair_with_server's POST
- Modify: `src-tauri/src/commands/sharing/lifecycle.rs:248` — `lmstudio_running_port`

**Why:** Five sites create a fresh `reqwest::Client` per call. Each `Client::new()` allocates a new TLS context and connection pool. Pooling via a single shared `Arc<Client>` is the standard pattern and the existing `auth_proxy.rs` already does this.

- [ ] **Step 1: Add `http_client` to `AppState`**

  In `src-tauri/src/state.rs`, find the `pub struct AppState` declaration. Add a field:

  ```rust
  pub struct AppState {
      // ... existing fields ...
      /// Shared HTTP client for connection-test and pairing commands.
      /// Pooled per-host; reuse this instead of `reqwest::Client::new()`.
      pub http_client: Arc<reqwest::Client>,
  }
  ```

  In `AppState::initialize`, construct the client once near the start (before any subsystem that might need it):

  ```rust
  let http_client = Arc::new(
      reqwest::Client::builder()
          .pool_max_idle_per_host(4)
          .connect_timeout(std::time::Duration::from_secs(10))
          .timeout(std::time::Duration::from_secs(30))
          .build()
          .map_err(|e| InitError::Other(format!("Failed to build shared HTTP client: {e}")))?
  );
  ```

  And add `http_client,` to the struct literal returned from `initialize`.

- [ ] **Step 2: Update each call site to use the shared client**

  For each of the five call sites, change:

  ```rust
  let client = reqwest::Client::new();
  let response = client.get(&url).send().await?;
  ```

  To:

  ```rust
  let response = state.http_client.get(&url).send().await?;
  ```

  Or for command signatures that don't already take `state`, add `state: tauri::State<'_, AppState>` to the signature.

  Check each command's signature first — some are `pub async fn test_lmstudio_connection(host: String, port: u16)` without state. Add `state: State<'_, AppState>` as the first parameter.

  Note: changing a Tauri command signature may break frontend invocations. Search the Svelte frontend for the command names and confirm they don't pass extra positional args that would conflict with the new `state` parameter (Tauri's `state` is injected, not passed from the frontend).

- [ ] **Step 3: Run tests**

  Run: `cargo test -p rust-medical-assistant --lib`
  Expected: pass.

  Run: `cargo build -p rust-medical-assistant`
  Expected: clean.

- [ ] **Step 4: Smoke-test from the frontend (manual)**

  Open the app. Navigate to Settings → Audio (or wherever connection tests live). Click "Test connection" for each provider. Confirm:
  - Still works
  - Returns the same result as before (success message or specific error)

- [ ] **Step 5: Commit**

  ```bash
  git add src-tauri/src/state.rs src-tauri/src/commands/providers.rs src-tauri/src/commands/sharing/pairing.rs src-tauri/src/commands/sharing/lifecycle.rs
  git commit -m "refactor: share reqwest::Client across connection-test commands

  Five command sites built a fresh reqwest::Client per call, each
  allocating a new TLS context and connection pool. Park a pooled
  Arc<Client> on AppState (initialized once at boot) and reuse across:
  - test_ollama_connection
  - test_lmstudio_connection
  - test_stt_remote_connection
  - pair_with_server
  - lmstudio_running_port"
  ```

---

## Final verification

- [ ] **Workspace test sweep**

  ```bash
  cargo test --workspace --lib
  cargo test -p medical-security
  cargo test -p medical-rag
  cargo test -p medical-audio
  cargo test -p rust-medical-assistant
  ```

  Expected: all pass.

- [ ] **PHI-policy check**

  Run: `git diff master..HEAD -- '*.rs' | grep -E "^\+.*tracing::(info|warn|error|debug)!"`

  Confirm new log lines log structural info only (error displays, IDs, configuration values) — no PHI.

- [ ] **Security sanity check**

  Read the final diff of `crates/security/src/key_storage.rs`. Confirm:
  - No path remains that produces a key from the literal `"fallback"` string
  - The error variant clearly indicates "the user/operator needs to do something"
  - Existing test coverage on `KeyStorage::open`/`get_key`/`set_key` still passes
