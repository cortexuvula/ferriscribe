# Sharing Commands `Result<_, String>` → `AppError` Migration — Design

**Date:** 2026-05-15
**Branch:** `apperror-migration`
**Predecessor:** Component splits (`e38d774`)

## Goal

Migrate 17 sharing-related Tauri-boundary functions from `Result<T, String>` to `AppResult<T>` (i.e., `Result<T, AppError>`) so the frontend receives the standard structured error shape (`{kind, message, ...}`) for these commands as it already does for the rest of the surface. Mechanical refactor — no logic, dialog, or test-shape changes.

## Scope

**17 functions across 4 files:**

`src-tauri/src/commands/sharing/mod.rs` — 3 internal helpers (called by the Tauri commands; migrating them lets callers use `?`):
- `paired_connection_path()`
- `server_config_path()`
- `write_server_config(cfg)`

`src-tauri/src/commands/sharing/discovery.rs` — 2 Tauri commands:
- `discover_servers(timeout_ms)`
- `discover_via_tailscale(...)`

`src-tauri/src/commands/sharing/lifecycle.rs` — 5 functions (2 Tauri commands + 1 inner + 1 helper... call it 5 sites):
- `start_sharing(...)` (Tauri command)
- `start_sharing_inner(...)` (re-exported helper, called from `start_sharing` and from `state::init`)
- `stop_sharing(state)` (Tauri command)
- `sharing_status(state)` (Tauri command)
- `build_sharing_config(...)` (internal helper)

`src-tauri/src/commands/sharing/pairing.rs` — 7 Tauri commands:
- `pairing_qr(state)`
- `list_paired_clients(state)`
- `revoke_client(state, id)`
- `rename_client(state, id, label)`
- `pair_with_server(state, ...)`
- `paired_endpoint()`
- `unpair(state)`

`suggested_client_label()` is `pub fn -> String` already (no Result) — untouched.

## Out of scope

- `sharing_vocab_api.rs` — these are axum HTTP handlers, not Tauri commands. They serialize to HTTP status codes + JSON bodies and the abstraction is different. `Result<_, String>` is fine there.
- `corpus_export/mod.rs` — internal `pub fn export(...)` called from `training_corpus_export.rs`; not crossing a serialization boundary.
- `training_corpus_export.rs` line 43 — local `Result<_, String>` inside a `spawn_blocking` closure; the outer command already returns `AppResult`.
- Adding new `AppError` variants. Stays in scope of "mechanical type swap".

## Mapping recipe

Per the existing `AppError` enum (`crates/core/src/error.rs`):

| Source pattern | Target |
|----------------|--------|
| `.map_err(|e| e.to_string())` where `e: std::io::Error` | `?` (auto-from via `#[from]`) or `.map_err(AppError::from)` if wrapped in a non-`?` context |
| `.map_err(|e| e.to_string())` where `e: serde_json::Error` | `?` (auto-from) |
| `.map_err(|e| e.to_string())` for everything else | `.map_err(AppError::from)` (uses the existing `From<String>` → `AppError::Other`) |
| `.ok_or_else(|| "msg".to_string())` | `.ok_or_else(|| AppError::Other("msg".into()))` |
| `Err("msg".to_string())` | `Err(AppError::Other("msg".into()))` |
| `Err(format!("..."))` | `Err(AppError::Other(format!("...")))` |
| `dirs::data_dir().ok_or_else(|| "no app data dir".to_string())` | `dirs::data_dir().ok_or_else(|| AppError::Other("no app data dir".into()))` — could be `Config`, but `Other` is fine and avoids overloading `Config` with non-user-facing semantics |

**Imports added at the top of each migrated file:**
```rust
use medical_core::error::{AppError, AppResult};
```
(`medical_core` is the workspace crate that owns the error module — verify by reading existing AppError-using files like `commands/vocabulary.rs` to confirm the import path.)

## Frontend contract

The frontend's `formatError(err)` in `src/lib/types/errors.ts` already handles both raw strings and `AppError`-shaped `{kind, message}` objects (and routes specific kinds like `EndpointOffline` and `InvalidEndpoint` to dedicated dialogs). After migration:

- Sharing commands return `{kind: "Other", message: "<text>"}` for catch-all errors (previously plain `"<text>"`).
- File-read errors auto-converted via `?` return `{kind: "Io", message: "..."}`.
- The "Sharing" UI components (Settings → Sharing) catch errors with `try { ... } catch (err) { toasts.error(formatError(err)) }` — no change needed in the frontend.

The shape change is strictly additive: any caller doing `String(err)` or relying on `.message` gets the same human-readable text from `formatError`.

## Test impact

Backend tests:
- `commands/sharing/mod.rs` test `write_then_delete_server_config_is_idempotent` calls `server_config_path()` and `write_server_config()`. The migration changes return types from `Result<_, String>` to `AppResult<_>`. Test assertions use `Ok(p)` pattern matching and `.expect("write should succeed")` — both compile fine against `AppResult` (since `AppError: Debug`). No assertion changes needed.
- No other tests touch the sharing command functions directly.

Frontend:
- No changes. `formatError()` already handles both shapes.

## Acceptance criteria

- `cargo build -p rust-medical-assistant` — clean.
- `cargo test --workspace --lib` — all 14 lib suites green, same totals as baseline.
- `cargo clippy -p rust-medical-assistant --no-deps -- -D warnings` — clean (no new warnings introduced).
- `git grep -nE 'Result<.*, String>' src-tauri/src/commands/sharing/` — only the comment "Result" mentions remain; no live signatures.
- `npm run check` — clean (frontend types unchanged).
- 5 commits: one per file + one final verification commit if cleanup needed.

## Risk register

- **Caller-site type drift:** `start_sharing_inner` is called from `state::init` (verify path) — that caller must compile against the new return type. Mitigation: search for callers in `state/mod.rs` or wherever `start_sharing_inner` is re-imported.
- **`?` propagation gotchas:** Some `.map_err(|e| e.to_string())` sites wrap errors from inside iterator closures or `async` blocks. The implementer must verify each `?` actually compiles (the From impls are present for `std::io::Error` and `serde_json::Error`).
- **Test signature drift:** `mod.rs`'s test must still compile after `write_server_config` returns `AppResult<()>`. Mitigation: implementer checks `cargo test -p rust-medical-assistant --lib commands::sharing::mod::tests` after the mod.rs commit.

## Why no new `Sharing` variant

Adding `AppError::Sharing(String)` would let the frontend pattern-match on sharing-specific errors. Today no dialog or handler does that. YAGNI — skip the variant; revisit if/when sharing gets its own offline dialog like `EndpointOffline`.
