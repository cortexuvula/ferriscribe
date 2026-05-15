# Sharing Commands AppError Migration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Migrate 17 sharing-related functions from `Result<T, String>` to `AppResult<T>` per the recipe in `docs/superpowers/specs/2026-05-15-apperror-migration-design.md`.

**Worktree:** `.worktrees/apperror-migration` on branch `apperror-migration` (from master at `e38d774`).

**Baseline:** 14 cargo lib suites all `ok`, totaling identical counts to `e972654` (sharing-tier3 merge baseline).

## Universal recipe (applies in every task)

1. Add `use medical_core::error::{AppError, AppResult};` at the top of the file (just below existing `use` lines).
2. Change every target function's return type from `Result<T, String>` to `AppResult<T>` (i.e., replace `Result<X, String>` with `AppResult<X>`).
3. Inside each function body, rewrite per the mapping table in the spec:
   - `.map_err(|e| e.to_string())` on `std::io::Error` → `?` (auto-from)
   - `.map_err(|e| e.to_string())` on `serde_json::Error` → `?` (auto-from)
   - `.map_err(|e| e.to_string())` for anything else → `.map_err(AppError::from)` (uses existing `From<String>` impl)
   - `.ok_or_else(|| "msg".to_string())` → `.ok_or_else(|| AppError::Other("msg".into()))`
   - `Err("msg".to_string())` → `Err(AppError::Other("msg".into()))`
   - `Err(format!("..."))` → `Err(AppError::Other(format!("...")))`
4. After every file: run `cargo build -p rust-medical-assistant`. Fix any compile error before committing.
5. After every file: run `cargo test --workspace --lib`. Same suite counts as baseline.
6. Commit with the per-task message below.

**Critical:** the spec's `From<String> for AppError` impl exists — `.map_err(AppError::from)` and `error.into()` both work for `String` errors. Use whichever reads cleaner; prefer `AppError::from` for clarity.

---

## Task 1: Migrate `commands/sharing/mod.rs`

**File:** `src-tauri/src/commands/sharing/mod.rs`

**Targets:** 3 internal helpers (`paired_connection_path`, `server_config_path`, `write_server_config`).

**Apply the universal recipe.**

Specific guidance:
- `paired_connection_path` and `server_config_path` use `dirs::data_dir().ok_or_else(|| "no app data dir".to_string())` — convert the `String` to `AppError::Other("no app data dir".into())`.
- `paired_connection_path` and `server_config_path` use `std::fs::create_dir_all(...).map_err(|e| e.to_string())` — convert to `?` operator (io::Error auto-converts).
- `write_server_config` uses `serde_json::to_string(...).map_err(|e| e.to_string())` — convert to `?` (serde_json::Error auto-converts).
- `write_server_config` uses `std::fs::write(...).map_err(|e| e.to_string())` (the final expression) — convert to `?` and add a `;` and `Ok(())` at the end, OR simpler: keep it as `std::fs::write(...).map_err(AppError::from)` if that compiles. Use `Ok(std::fs::write(&path, json)?)` if the implicit return needs adjusting.

**Tests:** The existing `mod tests` calls `server_config_path()` and `write_server_config()` (lines 139–151 of mod.rs). The test uses `match server_config_path() { Ok(p) => p, Err(_) => return, }` and `.expect("write should succeed")`. Both work with `AppResult<_>` (since `AppError: Debug` is required by `.expect`, and the `match Ok/Err` pattern is type-agnostic). **No test changes needed**, but verify by running `cargo test -p rust-medical-assistant --lib commands::sharing` after the file changes.

### Steps
- [ ] Read the file.
- [ ] Apply the recipe.
- [ ] `cargo build -p rust-medical-assistant` clean.
- [ ] `cargo test -p rust-medical-assistant --lib commands::sharing` — same pass count as baseline.
- [ ] Commit:
  ```
  refactor(sharing): migrate mod.rs helpers to AppError

  paired_connection_path, server_config_path, write_server_config now
  return AppResult instead of Result<_, String>. Callers in lifecycle/
  and pairing/ can propagate via ? in the next tasks.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 2: Migrate `commands/sharing/discovery.rs`

**File:** `src-tauri/src/commands/sharing/discovery.rs`

**Targets:** 2 Tauri commands (`discover_servers`, `discover_via_tailscale`).

**Apply the universal recipe.**

Read the file first to identify all `.map_err`/`ok_or_else` sites. Sites likely include mDNS browse errors, Tailscale CLI invocation errors, JSON parse errors.

### Steps
- [ ] Read the file.
- [ ] Apply the recipe.
- [ ] `cargo build -p rust-medical-assistant` clean.
- [ ] `cargo test --workspace --lib` — all suites pass.
- [ ] Commit:
  ```
  refactor(sharing): migrate discovery commands to AppError

  discover_servers and discover_via_tailscale return AppResult.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 3: Migrate `commands/sharing/lifecycle.rs`

**File:** `src-tauri/src/commands/sharing/lifecycle.rs`

**Targets:** 5 functions:
- `start_sharing` (Tauri command, line 16)
- `start_sharing_inner` (helper, line 32 — re-exported via mod.rs)
- `stop_sharing` (Tauri command, line 143)
- `sharing_status` (Tauri command, line 205)
- `build_sharing_config` (helper, line 221)

**Apply the universal recipe.**

**Caller verification:** `start_sharing_inner` is called from:
1. `src-tauri/src/commands/sharing/lifecycle.rs:20` — inside `start_sharing(...)`; uses `?` propagation; works fine after migration.
2. `src-tauri/src/lib.rs:160` — uses `if let Err(e) = ...`. The `e` binding will become `AppError` instead of `String`. Check the body of that `if let` block (in `src-tauri/src/lib.rs` around lines 160–170). If it does `tracing::error!("...{}", e)` or similar, that still works (AppError implements Display via thiserror). If it does `.to_string()` on `e`, that still works. If it does anything String-specific (e.g., `.contains("...")`), update it accordingly.

After migrating `lifecycle.rs`, build the entire app (`cargo build -p rust-medical-assistant`) so any caller-side breakage surfaces immediately. If `src-tauri/src/lib.rs` requires a touch-up, include that change in the same commit.

### Steps
- [ ] Read `lifecycle.rs` and the `lib.rs` start_sharing_inner caller block (lines ~155–175).
- [ ] Apply the recipe to lifecycle.rs.
- [ ] Adjust `lib.rs:160` block if needed.
- [ ] `cargo build -p rust-medical-assistant` clean.
- [ ] `cargo test --workspace --lib` — same totals.
- [ ] Commit:
  ```
  refactor(sharing): migrate lifecycle commands to AppError

  start_sharing, start_sharing_inner, stop_sharing, sharing_status,
  build_sharing_config now return AppResult. Touches src-tauri/src/lib.rs
  if the start_sharing_inner caller required adjustment.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 4: Migrate `commands/sharing/pairing.rs`

**File:** `src-tauri/src/commands/sharing/pairing.rs`

**Targets:** 7 Tauri commands:
- `pairing_qr` (line 12)
- `list_paired_clients` (line 35)
- `revoke_client` (line 49)
- `rename_client` (line 57)
- `pair_with_server` (line 87)
- `paired_endpoint` (line 267)
- `unpair` (line 281)

**Apply the universal recipe.**

`paired_endpoint` reads + deserializes `paired_connection.json`. Both `std::fs::read_to_string` (io::Error) and `serde_json::from_str` (serde_json::Error) auto-convert via `?`.

`unpair` does `std::fs::remove_file(...)` and returns `Ok(())` on missing file. The mapping retains: use `?` for io errors but **be careful** if the original code intentionally swallowed `NotFound` errors — preserve that behavior. Read the existing body before editing.

`pair_with_server` does an HTTP POST via reqwest — `reqwest::Error::to_string()` works the same after migration (`.map_err(AppError::from)` produces `AppError::Other(error_text)`).

### Steps
- [ ] Read the file.
- [ ] Apply the recipe.
- [ ] `cargo build -p rust-medical-assistant` clean.
- [ ] `cargo test --workspace --lib` — same totals.
- [ ] Commit:
  ```
  refactor(sharing): migrate pairing commands to AppError

  All 7 pairing Tauri commands now return AppResult.

  Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
  ```

---

## Task 5: Final verification

### Steps
- [ ] `git grep -nE 'Result<.*, String>' src-tauri/src/commands/sharing/` — should match ONLY comments / doc strings, never live function signatures.
- [ ] `cargo build -p rust-medical-assistant` clean.
- [ ] `cargo test --workspace --lib` — 14 lib suites all `ok`, same totals as baseline.
- [ ] `cargo clippy -p rust-medical-assistant --no-deps -- -D warnings` — clean. (If pre-existing warnings on master exist, only verify NO NEW warnings introduced.)
- [ ] `npm run check` — clean (sanity check; types unchanged).
- [ ] Dispatch final whole-branch reviewer subagent.
- [ ] Present merge options menu after review.

After all tasks: use superpowers:finishing-a-development-branch.
