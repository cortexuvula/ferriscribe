# Connected Client Renaming — Design

**Date:** 2026-05-08
**Status:** Approved (ready for implementation plan)

## Problem

When a client pairs with the office server today, it sends a label that the office-server admin sees in the "Connected clients" panel (`ServerStatus.svelte:81-95`). Two friction points:

1. If the client leaves the label input blank, `ClientPair.svelte:115` silently substitutes the literal string `"this laptop"`. Multiple clinicians paired this way end up indistinguishable in the list. The screenshot that motivated this spec shows exactly that — the only entry reads "this laptop", with no way to tell which clinician's machine it is.
2. There is no way to rename a connected client. Once the label is set at pair time, the only remediation is revoke + re-pair.

## Goals

- The office-server admin can rename any connected client in place, from the Connected clients panel.
- The client supplies a meaningful label at pair time even if they don't type anything — no more generic `"this laptop"` placeholder slipping through.
- The label round-trip stays inside the existing SQLCipher token store; no new secrets, no new endpoints.
- No PHI introduced. Hostnames are not PHI.

## Non-goals

- Audit history of label changes (no `original_label` column, no rename log).
- Server-driven rename push to the client. The client never sees its label after pair time; renames live entirely on the server.
- Length-limit user-facing errors. We silently truncate.
- Bulk operations or multi-select.
- A separate HTTP admin endpoint on the orchestrator. The frontend hits `TokenStore` via Tauri commands, same as the existing `revoke_client`.

## Decisions

| # | Decision |
|---|---|
| Q1 | Build both server-side rename and client-side pair-time strengthening. |
| Q2 | When the pair-time label input is empty, pre-populate from the client's OS hostname (stripped of trailing `.local`). User can overwrite. The `"this laptop"` fallback is removed. |
| Q3 | Server rename UX: inline edit in each row of the Connected clients list. Pencil icon → input → Enter saves, Esc cancels. |
| Q4 | No `original_label` column. `clients.label` is overwritten in place. |
| Q5 | Length cap is 80 chars post-trim, silently truncated at the store layer. |
| Q6 | Empty (post-trim) labels rejected at both Tauri command and `update_label` layers, surfaced as inline error in the row. |

## Architecture

### Data model

No schema migration. The existing `clients.label TEXT NOT NULL` column (`crates/sharing/src/token_store.rs:62-72`) holds both the initial pair-time value and any subsequent rename — overwritten in place.

### Backend additions

**`crates/sharing/src/token_store.rs`**
- New error variant: `TokenStoreError::EmptyLabel`.
- New method:
  ```rust
  pub fn update_label(&self, id: i64, new_label: &str) -> Result<()>
  ```
  Behavior: trim, reject empty with `EmptyLabel`, truncate to 80 chars, `UPDATE clients SET label = ? WHERE id = ? AND revoked_at IS NULL`. If `rows_affected == 0`, return a "not found or revoked" error.

**`crates/sharing/src/lib.rs` (or small new helper module)**
- New pure function:
  ```rust
  pub fn suggested_client_label() -> String
  ```
  Calls `hostname::get()`, lossy-converts to `String`, strips trailing `.local.` then `.local`. Falls back to `"laptop"` if the lookup errors. Already-present `hostname` dependency at `crates/sharing/Cargo.toml:30`.

**`src-tauri/src/commands/sharing/pairing.rs`**
- New Tauri command `rename_client(state, id: i64, label: String) -> Result<(), String>`. Mirrors the shape of `revoke_client` (`pairing.rs:49`); calls `svc.token_store().update_label(id, &label)`.
- New Tauri command `suggested_client_label() -> String`. Wraps the helper.
- Both registered in the invoke handler in `src-tauri/src/lib.rs` (alongside `revoke_client` at `lib.rs:252`).

### Server-side UI — `ServerStatus.svelte`

State additions:
```ts
let editingId: number | null = null;
let draftLabel = '';
let editError: string | null = null;
```

Per-row layout (replacing `ServerStatus.svelte:88-94`):
- When `editingId !== c.id`: show label text + pencil icon button + Revoke button.
- When `editingId === c.id`: show input bound to `draftLabel`, ✓ commit button, ✕ cancel button, Revoke button. On focus, select all.
- Inline `editError` shown under the row when set.

Interactions:
- Pencil click: `editingId = c.id; draftLabel = c.label; editError = null;`. Pause `setInterval` poll (`ServerStatus.svelte:43`) while editing — otherwise refresh-driven `clients = …` reassignment can clobber the input mid-typing.
- Enter / ✓ click: trim `draftLabel`. If empty → set `editError`. Else `await invoke('rename_client', { id: editingId, label: trimmed })`; on success clear edit state and call existing `refresh()`. On thrown error, surface as `editError` and stay in edit mode.
- Esc / ✕ click: clear edit state without touching the server.
- Resume the poll on commit/cancel.

### Client-side UI — `ClientPair.svelte`

- On mount (`ClientPair.svelte:208`), call `invoke<string>('suggested_client_label')` and assign to `label` if the field is currently empty. Failure of the invoke is non-fatal — leave the field blank.
- Rewrite the fallback at `ClientPair.svelte:115`:
  ```ts
  const trimmed = label.trim();
  const tokenLabel = trimmed
    || (await invoke<string>('suggested_client_label').catch(() => ''))
    || '';
  if (!tokenLabel) {
    error = 'Please enter a label for this computer.';
    busy = false;
    return;
  }
  ```
  No more `"this laptop"`. Helper hint text (`ClientPair.svelte:249`) stays as is.

### Error handling

| Path | Behavior |
|---|---|
| Empty label on rename | `update_label` returns `EmptyLabel`; Tauri command surfaces as string; row stays in edit mode with red inline message. |
| Label > 80 chars on rename | Truncated silently at the store layer. User sees the saved value on next refresh. |
| Rename a revoked / nonexistent id | `update_label` returns "client not found or revoked"; surfaced inline. |
| Rename + revoke race | Revoke wins. Subsequent rename returns the not-found error above. |
| Hostname lookup fails on client | `suggested_client_label` returns `"laptop"`; pair form pre-fills with that. |
| Pair-form `suggested_client_label` invoke fails | Caught and ignored at the call site. Field remains empty; user must type something or pairing fails with the empty-label error path. |

## Tests

**`crates/sharing/tests/token_store.rs`** — new cases:
- `rename_round_trips`: issue → update_label → list shows new value.
- `rename_rejects_empty`: `update_label(id, "   ")` returns `EmptyLabel`.
- `rename_truncates_at_80`: 200-char input → stored value is exactly 80 chars.
- `rename_rejects_revoked`: revoke then rename → not-found error.
- `rename_rejects_unknown_id`: rename a row that was never issued → not-found error.

**`crates/sharing/tests/`** (new file `suggested_label.rs` or inline in `pairing.rs` test module):
- `suggested_label_strips_local_suffix`: helper tested by injecting hostname through a thin private fn that takes `Result<OsString, _>`.
- `suggested_label_falls_back_when_hostname_errors`.

**Frontend Vitest** (`src/lib/components/settings/sharing/__tests__/ClientPair.test.ts`, new):
- Pre-fills `label` from a mocked `suggested_client_label` invoke on mount.
- Does not overwrite a non-empty `label` already typed by the user.
- Empty-after-trim label with empty suggestion → shows error, does not call `pair_with_server`.

**Frontend Vitest** (`ServerStatus.test.ts`, new):
- Pencil click enters edit mode and selects all input text.
- Enter commits with trimmed value, calls `rename_client`, refreshes.
- Esc cancels without invoking the backend.
- Backend error from `rename_client` keeps the row in edit mode with the error visible.

No new e2e tests — the existing `orchestrator_e2e` suite covers the auth-proxy paths and isn't affected by label storage.

## Out of scope (explicitly)

- Showing rename history, "renamed by admin" indicator, or `original_label`.
- Notifying the client when its label changes.
- Validating the label against a regex or character set. UTF-8 with whitespace trim is sufficient.
- A separate `/pair/rename/:id` HTTP admin route on the orchestrator.
- Localized error strings.
