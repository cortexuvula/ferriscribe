# User Dictionary Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the client is paired with an office server, the per-user spellcheck dictionary syncs to the server over HTTP (server-canonical), mirroring how the vocabulary feature already works.

**Architecture:** Add three routes (`GET / POST / DELETE`) to the existing `sharing_vocab_api.rs` axum router on the same `vocab_port`. Add a thin HTTP client (`user_dict_remote.rs`) on the paired client. Branch the three existing dictionary Tauri commands in `commands/user_dictionary.rs` to call the remote when a paired connection with a `vocab_port` is present; otherwise unchanged local-DB path.

**Tech Stack:** Rust (Tauri 2, axum, reqwest, rusqlite via `medical_db::user_dictionary::UserDictionaryRepo`, `urlencoding`).

**Spec:** `docs/superpowers/specs/2026-05-20-dictionary-sync-design.md`

---

## File map

- Create `src-tauri/src/user_dict_remote.rs` — HTTP client (`list / add / remove`); parallel to `vocab_remote.rs`.
- Modify `src-tauri/src/lib.rs` — add `mod user_dict_remote;` next to the existing `mod vocab_remote;`.
- Modify `src-tauri/src/sharing_vocab_api.rs` — add three handlers and register routes on the existing router.
- Modify `src-tauri/src/commands/user_dictionary.rs` — add `paired_dict_target()` helper, branch each command on it.

No frontend changes. No migrations. No new ports.

## Conventions for this plan

- The version string used in the 404 error message is the version that ships this feature — use `v0.10.84` as the placeholder; if the release version differs at merge time, grep `0.10.84` and update.
- Workspace package for the Tauri app is `rust-medical-assistant` (NOT `medical-tauri`). Build with `cargo build -p rust-medical-assistant`; test with `cargo test --workspace --lib`.
- Vocab handlers in `sharing_vocab_api.rs` have NO unit tests today. We follow the same convention and verify with `cargo check` / `cargo build` / `cargo test --workspace --lib` plus a manual smoke at the end. Do NOT scaffold new axum / wiremock test infrastructure solely for this change.
- No PHI in logs — log lengths/counts, never word values.

---

## Task 1 — Server: add dictionary routes to the existing vocab API

**Files:**
- Modify: `src-tauri/src/sharing_vocab_api.rs`

### Step 1.1 — Add the three handlers

Open `src-tauri/src/sharing_vocab_api.rs`. At the very bottom of the file (after the templates section), append:

```rust
// ── User dictionary handlers ────────────────────────────────────────────
//
// Per-user spellcheck wordlist. Reads/writes hit
// `medical_db::user_dictionary::UserDictionaryRepo` against the office
// server's local SQLite DB. Same bearer auth + spawn_blocking pattern as
// the vocab handlers above. No PHI in logs.

#[derive(Deserialize)]
struct DictAddBody {
    word: String,
}

async fn dict_list_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<String>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let words = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        medical_db::user_dictionary::UserDictionaryRepo::list(&conn)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("dict_api list failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    debug!(count = words.len(), "dict_api: list");
    Ok(Json(words))
}

async fn dict_add_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(body): Json<DictAddBody>,
) -> Result<Json<bool>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let word = body.word;
    let word_len = word.len();
    let added = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        medical_db::user_dictionary::UserDictionaryRepo::add(&conn, &word)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("dict_api add failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    info!(word_len, added, "dict_api: add");
    Ok(Json(added))
}

async fn dict_remove_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Path(word): Path<String>,
) -> Result<Json<bool>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let word_len = word.len();
    let removed = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let conn = db.conn().map_err(|e| e.to_string())?;
        medical_db::user_dictionary::UserDictionaryRepo::remove(&conn, &word)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        warn!("dict_api remove failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    info!(word_len, removed, "dict_api: remove");
    Ok(Json(removed))
}
```

### Step 1.2 — Register the three routes on the existing router

Find the `Router::new()` call inside `pub async fn spawn(...)` (currently around lines 61–69). It registers the vocab + context-templates routes via `.route(...)` chained calls. Add three more `.route(...)` calls immediately after the context-templates routes (i.e., right before `.with_state(state);`):

Old block (matches exactly what's there):
```rust
        .route("/v1/context-templates/delete", axum::routing::post(templates_delete_handler))
        .with_state(state);
```

New block:
```rust
        .route("/v1/context-templates/delete", axum::routing::post(templates_delete_handler))
        .route("/v1/user-dictionary", get(dict_list_handler).post(dict_add_handler))
        .route("/v1/user-dictionary/{word}", axum::routing::delete(dict_remove_handler))
        .with_state(state);
```

(`get` is already imported at the top of the file; `axum::routing::delete` is used inline for parity with the inline `axum::routing::post` calls already in the router.)

### Step 1.3 — Build the workspace

Run:
```bash
cargo build -p rust-medical-assistant
```
Expected: clean compile (warnings OK; no errors).

If the compiler complains about `Path` being ambiguous (since `std::path::Path` is also in scope in some files): the existing file already imports `axum::extract::Path` at the top, so it should be fine. If a conflict arises, fully qualify with `axum::extract::Path<String>` in the new handler signature.

### Step 1.4 — Run the library test suite

Run:
```bash
cargo test --workspace --lib
```
Expected: all existing tests pass; no new test failures.

### Step 1.5 — Commit

```bash
git add src-tauri/src/sharing_vocab_api.rs
git commit -m "feat(sharing): server dictionary sync routes on vocab_port

Adds /v1/user-dictionary GET/POST/DELETE handlers to the existing
vocab API router. Reads/writes go through UserDictionaryRepo on the
office server's local DB. No PHI in logs.

Refs docs/superpowers/specs/2026-05-20-dictionary-sync-design.md"
```

---

## Task 2 — Client: new `user_dict_remote.rs` HTTP wrapper

**Files:**
- Create: `src-tauri/src/user_dict_remote.rs`
- Modify: `src-tauri/src/lib.rs`

### Step 2.1 — Create `src-tauri/src/user_dict_remote.rs`

```rust
//! HTTP client for the office server's user-dictionary CRUD API.
//!
//! Mirrors `vocab_remote.rs`: when a paired connection is present and the
//! office server advertised a `vocab_port`, the user-dictionary Tauri
//! commands route through here instead of the local SQLite repo so the
//! server stays the canonical source of truth.

use medical_core::error::{AppError, AppResult};
use medical_core::types::endpoint::http_url;
use serde::Serialize;

use crate::commands::sharing::PairedConnection;

pub struct UserDictRemote<'a> {
    pub conn: &'a PairedConnection,
    pub bearer: String,
    pub client: std::sync::Arc<reqwest::Client>,
}

impl<'a> UserDictRemote<'a> {
    /// Returns `Some(...)` when the paired connection has a `vocab_port`
    /// (the dictionary API rides on the same port as the vocab API) AND a
    /// bearer is available. Otherwise `None` — caller falls back to local DB.
    pub fn from(
        conn: &'a PairedConnection,
        bearer: Option<String>,
        client: std::sync::Arc<reqwest::Client>,
    ) -> Option<Self> {
        let bearer = bearer?;
        conn.ports.vocab?;
        Some(Self { conn, bearer, client })
    }

    fn base_url(&self) -> Option<String> {
        let port = self.conn.ports.vocab?;
        let host = self.conn.lan.as_deref().or(self.conn.tailscale.as_deref())?;
        Some(http_url(host, port))
    }

    pub async fn list(&self) -> AppResult<Vec<String>> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no dictionary address".into()))?;
        let url = format!("{base}/v1/user-dictionary");
        let resp = self.client
            .get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("dict list: {e}")))?;
        check_status(&resp).await?;
        resp.json::<Vec<String>>()
            .await
            .map_err(|e| AppError::Other(format!("dict list parse: {e}")))
    }

    pub async fn add(&self, word: &str) -> AppResult<bool> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no dictionary address".into()))?;
        let url = format!("{base}/v1/user-dictionary");
        let body = AddBody { word: word.to_string() };
        let resp = self.client
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("dict add: {e}")))?;
        check_status(&resp).await?;
        resp.json::<bool>()
            .await
            .map_err(|e| AppError::Other(format!("dict add parse: {e}")))
    }

    pub async fn remove(&self, word: &str) -> AppResult<bool> {
        let base = self
            .base_url()
            .ok_or_else(|| AppError::Other("paired server has no dictionary address".into()))?;
        let encoded = urlencoding::encode(word);
        let url = format!("{base}/v1/user-dictionary/{encoded}");
        let resp = self.client
            .delete(&url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("dict remove: {e}")))?;
        check_status(&resp).await?;
        resp.json::<bool>()
            .await
            .map_err(|e| AppError::Other(format!("dict remove parse: {e}")))
    }
}

#[derive(Debug, Serialize)]
struct AddBody {
    word: String,
}

async fn check_status(resp: &reqwest::Response) -> AppResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::Other(
            "Office server does not support dictionary sync (update it to v0.10.84 or later)."
                .to_string(),
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::Other(
            "Office server rejected the bearer token. Try unpair → re-pair from this client."
                .to_string(),
        ));
    }
    Err(AppError::Other(format!("dictionary API: HTTP {status}")))
}
```

### Step 2.2 — Declare the module in `src-tauri/src/lib.rs`

Find the top of `src-tauri/src/lib.rs`. There's an existing line `mod vocab_remote;` (around line 5). Add `mod user_dict_remote;` immediately after it:

Old block:
```rust
mod sharing_vocab_api;
mod vocab_remote;
```

New block:
```rust
mod sharing_vocab_api;
mod vocab_remote;
mod user_dict_remote;
```

### Step 2.3 — Build to confirm the new module compiles

Run:
```bash
cargo build -p rust-medical-assistant
```
Expected: clean compile. The new module isn't used yet, so you may see a `dead_code` warning for `UserDictRemote` — that's fine; Task 3 wires it in.

### Step 2.4 — Commit

```bash
git add src-tauri/src/user_dict_remote.rs src-tauri/src/lib.rs
git commit -m "feat(sharing): UserDictRemote HTTP client for dictionary sync

Parallel to vocab_remote.rs. Rides on the same vocab_port advertised
by the office server. list/add/remove with the same bearer + timeout
+ check_status conventions as VocabRemote.

Refs docs/superpowers/specs/2026-05-20-dictionary-sync-design.md"
```

---

## Task 3 — Client: route commands through `UserDictRemote` when paired

**Files:**
- Modify: `src-tauri/src/commands/user_dictionary.rs`

### Step 3.1 — Replace the file contents

Replace the entire body of `src-tauri/src/commands/user_dictionary.rs` with the following. This adds a `paired_dict_target()` helper and branches each of the three commands on it (paired → HTTP via `UserDictRemote`; otherwise → existing local-DB path).

```rust
//! Tauri commands for the per-user spellcheck dictionary.
//!
//! When this client is paired with an office server that advertised a
//! `vocab_port`, dictionary operations route through HTTP to that server
//! (which becomes the canonical source of truth). Otherwise they operate
//! on the local SQLite repo.
//!
//! No word values are emitted to logs — the dictionary may contain
//! patient-context-specific terms.

use medical_core::error::{AppError, AppResult};

use crate::state::{self, AppState};
use crate::user_dict_remote::UserDictRemote;

/// Returns `Some((conn, bearer))` when this client is paired with an office
/// server that advertised a vocab CRUD API (same port the dictionary API
/// rides on). Commands route through HTTP in that case; otherwise they
/// operate on the local SQLite repo.
fn paired_dict_target() -> Option<(crate::commands::sharing::PairedConnection, String)> {
    let conn = state::load_paired_connection()?;
    conn.ports.vocab?;
    let bearer = state::load_sharing_bearer()?;
    Some((conn, bearer))
}

#[tauri::command]
pub async fn user_dict_list(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<String>> {
    if let Some((conn, bearer)) = paired_dict_target() {
        let remote = UserDictRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired dict target unavailable".into()))?;
        return remote.list().await;
    }
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    medical_db::user_dictionary::UserDictionaryRepo::list(&conn)
        .map_err(|e| AppError::Database(e.to_string()))
}

#[tauri::command]
pub async fn user_dict_add(
    state: tauri::State<'_, AppState>,
    word: String,
) -> AppResult<bool> {
    if let Some((conn, bearer)) = paired_dict_target() {
        let remote = UserDictRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired dict target unavailable".into()))?;
        return remote.add(&word).await;
    }
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    medical_db::user_dictionary::UserDictionaryRepo::add(&conn, &word)
        .map_err(|e| AppError::Database(e.to_string()))
}

#[tauri::command]
pub async fn user_dict_remove(
    state: tauri::State<'_, AppState>,
    word: String,
) -> AppResult<bool> {
    if let Some((conn, bearer)) = paired_dict_target() {
        let remote = UserDictRemote::from(&conn, Some(bearer), state.http_client.clone())
            .ok_or_else(|| AppError::Other("paired dict target unavailable".into()))?;
        return remote.remove(&word).await;
    }
    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    medical_db::user_dictionary::UserDictionaryRepo::remove(&conn, &word)
        .map_err(|e| AppError::Database(e.to_string()))
}
```

### Step 3.2 — Build the workspace

Run:
```bash
cargo build -p rust-medical-assistant
```
Expected: clean compile; the `dead_code` warning on `UserDictRemote` from Task 2 should be gone now.

### Step 3.3 — Run the library test suite

Run:
```bash
cargo test --workspace --lib
```
Expected: all existing tests pass. Of particular interest, `UserDictionaryRepo` tests in `crates/db/src/user_dictionary.rs` still pass (no DB schema or repo logic changed).

### Step 3.4 — Run frontend type-check and tests

Run:
```bash
npm run check && npx vitest run src/lib/api/userDictionary.test.ts src/lib/components/rich_editor/spellcheck/spellchecker.test.ts
```
Expected: no svelte-check errors; both vitest files pass (the IPC contract is unchanged, so these tests should be untouched).

### Step 3.5 — Commit

```bash
git add src-tauri/src/commands/user_dictionary.rs
git commit -m "feat(sharing): route dictionary commands through UserDictRemote when paired

When a paired connection advertises a vocab_port, user_dict_list/add/
remove route through HTTP to the office server. Otherwise unchanged
local-DB path. No frontend changes — IPC contract is identical.

Refs docs/superpowers/specs/2026-05-20-dictionary-sync-design.md"
```

---

## Task 4 — Manual verification (smoke test)

**Files:** none (manual).

This step verifies end-to-end behavior in a way the test suite cannot, because the server- and client-side bits run in the same process tree but talk over HTTP. Skip this only if there is no way to run the app interactively (e.g., headless CI for the plan); in that case the build + lib-test passes from Tasks 1–3 are the strongest signal available.

- [ ] **Step 4.1 — Smoke: unpaired client still works locally**

  Build and launch the app:
  ```bash
  cargo tauri dev
  ```
  In Settings → Dictionary (or wherever DictionaryDialog opens), add a test word, e.g., `lisinopril`. Verify it appears in the list. Quit. Relaunch. Verify it persists (local DB write went through).

- [ ] **Step 4.2 — Smoke: paired client routes through the server**

  Set up two machines (or two profiles) — A as the office server, B as the paired client. Start sharing on A; pair B with A's QR.

  On B, open the dictionary, add `metformin`. On A (or a second paired client), open the dictionary and verify `metformin` appears. Remove `metformin` on B and confirm it disappears for A too.

  Negative checks:
  - On B's machine, the local `user_dictionary` table inside the SQLCipher DB should be untouched while paired (verify in app data dir if you want a hard check; not required).
  - Server logs should show `dict_api: list` / `dict_api: add` / `dict_api: remove` lines, with `word_len` only — no word values.

- [ ] **Step 4.3 — Negative path: stale server (no dict routes)**

  If you can get a build of the previous version (pre-this-PR) running as the office server while pairing a new client to it, attempting `user_dict_list` on the client should surface the 404 message: `"Office server does not support dictionary sync (update it to v0.10.84 or later)."`. If reproducing this requires too much setup, skip — the message is exercised by Task 2's `check_status` and the only failure mode is a wrong status code branch.

---

## Verification summary

After Task 3:
- `cargo build -p rust-medical-assistant` → clean.
- `cargo test --workspace --lib` → all tests pass.
- `npm run check` → no svelte-check errors.
- `npx vitest run` (the two named test files) → pass.

After Task 4:
- Unpaired client: add/remove/list works locally.
- Paired client: add/remove/list goes through the office server; multiple paired clients see the same dictionary.

---

## Out of scope (do NOT add to this plan)

- Bulk import/export, `delete_all`, or batch sync routes.
- Backfill of local words to the server on first pair.
- Pull-down of server words to local DB on unpair.
- Per-user scoping on the server.
- Any change to the mDNS / QR discovery payload (the existing `vocab` port already covers discovery).
- Any frontend code change.

If any of these come up during implementation, file a follow-up — do NOT expand this plan.
