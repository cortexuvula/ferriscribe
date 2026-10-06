# Zcode Prompt — Reversible Delete All + Trash + Day-Scoped Restore (FerriScribe)

Feature brief for the FerriScribe desktop repo at `/Users/cortexuvula/Development/rustMedicalAssistant` (GitHub `cortexuvula/ferriscribe`). Implement on an isolated git worktree under `.worktrees/` — never `master` (repo rule). This brief was planned 2026-10-06: implementation plan at `docs/superpowers/plans/2026-10-06-reversible-trash-restore.md` (read it; this prompt is the binding build contract), UI design reviewed by the ui-consultant agent.

## Summary

Delete All is currently a permanent hard delete. Change it to a reversible "Move all to Trash": every active recording is soft-deleted (kept 30 days with audio and RAG vectors intact, restorable), a Trash view lists soft-deleted recordings with a days-remaining display, and the user can restore per-row, restore everything, or restore everything deleted on one exact calendar day. The existing 30-day tombstone-purge sweeper currently runs server-only — make it run on every machine. All restore paths must push the revive to a paired office server (today only delete does).

## Hard facts (verified; reuse, do not reinvent)

### Backend repo (`crates/db/src/recordings.rs`)
- `soft_delete` (line 512): single-transaction `UPDATE recordings SET deleted_at = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL` + guarded FTS `'delete'` via `INSERT INTO recordings_fts(recordings_fts, rowid, id, filename, transcript, soap_note, referral, letter, patient_name) SELECT 'delete', …` — the exact statement pair any new bulk soft-delete must reuse per row.
- `restore` (line 585): reads metadata, stamps `retention_exempt: true` in metadata (:624), FTS membership probe via `fts_row_indexed` (:565), guarded re-index (`INSERT INTO recordings_fts(rowid, id, …) SELECT … FROM recordings WHERE id = ?1`), then `UPDATE recordings SET deleted_at = NULL, updated_at = ?, metadata = ? WHERE id = ? AND deleted_at IS NOT NULL`; all in one transaction.
- `list_all` (:175) filters `deleted_at IS NULL` — a trashed listing needs a new query (`deleted_at IS NOT NULL ORDER BY deleted_at DESC` + `COUNT(*)` total). Never return transcript/SOAP content in trash summaries.
- `delete_all` (:865) is the current HARD delete (`DELETE FROM recordings WHERE deleted_at IS NULL`) — replace its use, then remove it (check call sites first).
- `purge_soft_deleted_with_ledger` (:800) + `purged_recordings` ledger already exist — the 30-day purge path must keep using the ledger variant so `merge_incoming` refuses stale copies.
- `list_soft_deleted_older_than` (:723) and `retention_soft_delete_older_than` (:677) already exist.
- `count` (:886) returns the active count.

### Commands (`src-tauri/src/commands/recordings.rs`)
- `delete_recording` (:77): soft delete + fire-and-forget tombstone push to paired server (:117-147) — pattern: `content_sync_target_parts` resolve, `spawn_blocking` builds `sync_rec` via `build_sync_recording`, sets `sync_rec.deleted_at`, `remote.push(recordings)`. MIRROR THIS for bulk.
- `restore_recording` (:155): clears `deleted_at` only — NO sync push today. This is a gap: add the revive push (delete push shape with `deleted_at = None`).
- `delete_all_recordings` (:194): TODAY hard-deletes + `DELETE FROM vectors` + removes WAVs + resets BOTH content-sync cursors (`ContentSyncRepo::set_cursor(&conn, None)` and `UPDATE sync_state SET value = NULL WHERE key = 'content_sync_push_cursor'`). The cursor reset must be REMOVED (with soft deletes it would resurrect everything from a partner on next pull; tombstones are the correct propagation).
- Register every new command in `src-tauri/src/lib.rs` `invoke_handler` (git grep `generate_handler!`).

### Command pattern (Ada finding 2)
`Database` is NOT `Clone` — `AppState.db` is `Arc<Database>`. Every new/spawn-blocking command must follow the established shape: `let db = state.db.clone(); tokio::task::spawn_blocking(move || { let conn = db.conn()?; ... })` (see `delete_recording` etc.). Never move `state` itself across the spawn boundary.

### Sweeper (`src-tauri/src/sweeps.rs`)
- `retention_sweep_tick(db, is_server)` (:241): Phase 1 tombstone purge is gated `if is_server` (:247) — vectors cleaned, audio removed, `purge_soft_deleted_with_ledger`; Phase 2 retention sweep runs everywhere. Doc comment (:225-234) says durable deletion is server policy. **Change: run Phase 1 unconditionally; drop the `is_server` parameter** (update `spawn_retention_sweeper` :339 and both tests — `retention_sweep_client_trashes_old_but_never_purges` :404 must become client-purger assertions; `retention_sweep_server_purges_old_tombstones_and_audio` :436 stays).

### Frontend
- `src/lib/pages/RecordingsTab.svelte`: Delete All button (:94-99), hard-delete ConfirmDialog (:134-141), single-delete ConfirmDialog (:125-132) with "You can undo this for 8 seconds" (:128) — DELETE that wording, single-delete Undo toast (:35-48), toolbar count uses `recordings.list.length` (:93, paginated subset — NOT authoritative).
- `src/lib/stores/recordings.svelte.ts`: `lastDeleted` (:161) is a single-summary undo cache; `remove` (:163), `restore` (:179, reloads after), `removeAll` (:197). Monotonic request-id discipline pattern at `listRequestId` (:144) — reuse for trash loads.
- `src/lib/api/recordings.ts`: `deleteRecording` (:16 export line), `restoreRecording` (:20), `deleteAllRecordings` (:24) — add wrappers here; mirror tests in `src/lib/api/recordings.test.ts`.
- `src/lib/types/index.ts:225-230` — `retention_days: number | null` frontend mirror.
- `src/lib/components/settings/sections/RecordingRetention.svelte`: options Never/30/90/180/365 (`<option value={0}>Never (keep forever)</option>` :18); change that label. Helper (:24-25) already documents the 30-day trash window + restore exemption — keep.
- `src/lib/components/ConfirmDialog.svelte:73` hardcodes `role="alertdialog"` — make the role configurable (default `alertdialog`, existing callers unchanged) + `aria-describedby` to the body element.
- The app is Svelte 5 + Vite (NOT SvelteKit); `npm run check` runs svelte-check.

## Design decisions (resolved — do not re-litigate)

1. Delete All = soft delete; cursor reset removed; tombstones pushed in bulk to the paired server.
2. 30-day purge runs on every machine (client + server), ledger variant, `is_server` parameter deleted.
3. Restore-by-date = EXACT local calendar day (Andre's ruling; NOT "on or after"). Frontend sends explicit UTC interval `[local-midnight, next-local-midnight)` as two ISO-8601 strings; backend compares `datetime(deleted_at) >= datetime(?1) AND datetime(deleted_at) < datetime(?2)` (half-open; matches the retention sweep's `datetime()` convention).
4. Bulk Undo (Move-all toast) restores an EXACT captured id set — never "everything deleted since <<timestamp>>". `delete_all_recordings` returns `{ count, ids }`; a new `restore_recordings(ids: string[])` command handles the batch Undo.
5. Every restore path (single, all, by-date, batch-undo) pushes the revived recording to the paired server. **Tombstone backstop (Codie suggestion 3):** the fire-and-forget push may swallow failures — that is acceptable ONLY because the periodic sync re-travels tombstones and revives: `push_batch` selects by `updated_at` with NO `deleted_at` filter and `build_sync_recording` carries `deleted_at` from the row; restore bumps `updated_at` so revives re-travel too. NEVER add a `deleted_at IS NULL` filter to the periodic push path — that would silently break deletion propagation in the privacy-critical direction. Pin with a test (D1 acceptance).
6. Trash retention fixed at 30 days — no new setting, no DB migration (`deleted_at`, `recordings_fts`, `purged_recordings` all exist).
7. `retention_exempt` must be stamped by every restore path (reuse per-row `restore()` logic inside one transaction).
8. Out of scope: "Empty Trash" button, per-row permanent-delete, configurable trash retention, date-range restore (single day only), changing `retention_days` semantics.

## Non-negotiable constraints (PHI / HIPAA — Same as AGENTS.md)

- NO PHI in logs: transcripts/SOAP/meds/conditions never in `tracing::*`, `println!`, `console.log`; logs carry counts/ids/lengths only.
- No new hosted/remote calls; the only outbound endpoints are user-configured local AI providers + the paired office server over Tailscale (existing content-sync push — reuse `content_remote::ContentRemote` / `content_sync_target_parts`, do not invent a second transport).
- Keep the SQLCipher + FE1 encrypted-audio-at-rest properties intact (this feature never touches encryption; it only stops deleting files earlier than 30 days).
- Frontend toasts/errors must not contain patient names or content — sanitize backend error strings before display.
- Strict CSP unchanged.

## Deliverables (numbered — each with its acceptance criterion)

- **D1 Move all to Trash.** Command `delete_all_recordings` soft-deletes every active row (per-row `soft_delete` statement pair, one transaction; FTS de-indexed; WAV + RAG vectors preserved; cursors untouched) and returns `{ count: u32, ids: string[] }`; after commit, bulk tombstone push to the paired server (fire-and-forget, one batch). Toolbar label "Move all to Trash"; confirm dialog copy per the Copy section; restrained styling (not the current irreversible warning). ACCEPT: after the move, `crates` rows have `deleted_at` set, count matches, WAV files still exist on disk, `vectors` table unchanged, sync-state cursor rows unchanged. **Tombstone backstop test (Codie suggestion 3):** with the fire-and-forget push forced to fail, the tombstone still reaches a paired server via the periodic sync — and the same holds for a revive after restore.
- **D2 Batch Undo.** Move-all toast (one toast, not one per recording) with an Undo action that calls `restore_recordings(ids)` with the exact captured set; success toast "N recordings restored to Active."; Active + Trash counts refresh; selection cleared. ACCEPT: undoing a move-all does not restore a recording that was deleted AFTER the move; duplicate submission is guarded while in flight.
- **D3 Restore recordings by ids.** `restore_recordings(ids: string[])` command + repo bulk restore (per-row `restore()` logic, one transaction, `retention_exempt` stamped) + revive push for the returned ids. ACCEPT: rows revived, FTS index consistent (no double index — membership probe), exemption stamped, server receives the revive.
- **D4 Trash view.** New repo `list_trashed(limit, offset) -> (items, total)` (newest deletion first, no content fields) + command `list_trashed_recordings`. New components: `RecordingViewSwitch.svelte` (Active | Trash(N) accessible tabs, visible even when Active is empty), `TrashPanel.svelte`, `TrashRecordingRow.svelte`. Row: display name (**patient name if present, else filename — one label, not both**), Recorded <date/time> · <duration>, Moved to Trash <date/time>, "<N days remaining> · Permanent deletion after <datetime>" — neutral text; restrained amber near expiry; "Less than 1 day remaining" under a day; **"Pending permanent deletion"** once past 30 days (never negative days, never a promise of an exact deletion time); Restore always visible, not hover-only; no transcript excerpts or processing controls. States: empty / loading ("Loading Trash…") / error ("Couldn't load Trash. Try again." + Retry — never render a load failure as an empty trash) / search-empty ("No recordings in Trash match your search." + "Clear search"). Separate search state from Active; entering Trash starts with an empty query. Bulk actions apply to ALL trashed records, not the loaded page or search (say so in dialogs; when filtered show "N matches · M in Trash"). ACCEPT: paginated browsing, correct total badge, all four states render, per-row restore works from any pagination offset.
- **D5 Restore all.** "Restore all…" with confirmation ("Restore all N recordings from Trash?" / "They will return to Active with their saved audio and documents." / Cancel / "Restore all N"), repo `restore_all`, revive push. Same in-flight duplicate-submit guard as the by-date dialog (Codie suggestion 4) — a double-click is one restore, not two racing transactions. ACCEPT: all trashed rows revived with exemption; count reported; duplicate submission produces one restore.
- **D6 Restore by exact day.** `RestoreByDateDialog.svelte`: ONE dialog — native labelled date input (typed or picked; reject invalid/future dates), helper text ('Uses the date recordings were moved to Trash, not the date they were recorded. Searches all of Trash, regardless of the current search. Times use <this machine's timezone>.'), count preview after a valid date ("Restore N recordings moved to Trash on <long-form date>?" / "They will return to Active with their saved audio and documents."), Cancel / "Restore N recordings"; zero matches → "No recordings currently in Trash match this date." and Restore disabled; "Counting…"/"Restoring…" states; duplicate-submit guard; on success stay in Trash, refresh counts, announce "N recordings restored to Active."; on failure keep date + dialog, "Couldn't restore recordings. Try again.". The preview count comes from the dedicated `count_recordings_deleted_between(start_iso, end_iso)` command — same RFC3339 + start<end validation and the same half-open `datetime()` interval as the restore command, NEVER derived from paginated trash pages (Codie gate 2). Backend `restore_recordings_deleted_between(start_iso, end_iso)` (validate RFC3339 + start < end) → repo `restore_deleted_between` (half-open `datetime()` interval, per-row restore logic) → revive push. Report ACTUAL restored count vs preview (a purge or concurrent restore may change the candidate set between preview and execution). ACCEPT: deleting one recording on day A, another on day B, then restoring day A restores exactly the day-A recording; boundaries are local-calendar-day exact.
- **D7 Purge everywhere.** `retention_sweep_tick` Phase 1 runs unconditionally AND **purge-first** (Codie gate 1): `list_soft_deleted_older_than` → `purge_soft_deleted_with_ledger` (transaction, confirmed purged ids) → THEN delete RAG vectors + audio ONLY for the confirmed ids (missing files tolerated). Today artifacts are deleted before the purge transaction from an unguarded listing — under D7 that race (restore lands between artifact deletion and purge → live row with audio/vectors already gone) would open daily on every machine. `is_server` parameter removed (update module docs and `spawn_retention_sweeper`); tests: client test INVERTED — asserts client purges aged tombstones with row gone + audio removed + ledger entry written (not just renamed; Ada finding 1); server test unchanged; NEW race test: a restore landing between listing and purge leaves the row ACTIVE with its audio file intact on disk. ACCEPT: a >30-day-old `deleted_at` row is purged on a non-server machine; `purged_recordings` carries the id; RAG vectors removed; `merge_incoming` stale-copy refusal still green (`cargo test -p medical-db`); the race test passes.
- **D8 Store + API wiring.** `src/lib/api/recordings.ts` wrappers (with tests): `listTrashedRecordings`, `restoreRecordings(ids)`, `restoreAllTrashed`, `restoreRecordingsDeletedBetween(startIso, endIso)`, `countRecordingsDeletedBetween(startIso, endIso)` (the D6 preview surface), `countRecordings` (authoritative active count for the confirm-dialog N — never `list.length`). Store: trash state + monotonic request-id invalidation; after ANY restore/delete/sync/purge, refresh both Active and Trash totals; clear stale selected-recording fetches during mutations. ACCEPT: `npx vitest run` green incl. new tests; authoritative N in dialogs; the preview count comes from the count command, never from paginated pages.
- **D9 A11y + copy.** ConfirmDialog role configurable + `aria-describedby` (existing callers untouched). View switch = real tabs (arrow keys). ONE polite `role="status"` region announces previews/completions/results; `role="alert"` only for actionable failures; static retention copy is not a live region; don't double-announce inline status + toast. Pause actionable-toast dismissal on focus/hover. Focus: if the invoking row disappears, move focus to next Restore button or the Trash heading — never `document.body`. Settings label change (D10). Copy per the Copy section — exact strings. ACCEPT: role/describedby assertions in `confirm-dialog` tests; announcements verified via jsdom `role="status"` presence and single-instance rule; copy grep matches the Copy section.
- **D10 Settings copy.** `RecordingRetention.svelte` option 0: "Never (keep forever)" → **"Never automatically move to Trash"**. Helper text retained. Update `DataManagement.test.ts` / `RecordingRetention`-related assertions to the new label. ACCEPT: label + tests match.

## Copy (exact strings)

- Toolbar: "Move all to Trash".
- Dialog title: "Move all N recordings to Trash?" (N = authoritative count).
- Body: "This moves all active recordings to Trash, including recordings outside the current search. Their audio, transcripts, SOAP notes, and generated documents are kept for 30 days. You can restore them from Trash during that time. After 30 days, they are permanently deleted." Buttons: "Cancel" / "Move N recordings to Trash".
- Toast: "N recordings moved to Trash. Available to restore for 30 days." Action: "Undo". Undo success: "N recordings restored to Active."
- Single delete title: "Move recording to Trash?" Body: "You can restore this recording from Trash for 30 days. After that, it and its saved audio and documents are permanently deleted." Action: "Move to Trash". Toast: "Recording moved to Trash." / "Undo". REMOVE "You can undo this for 8 seconds".
- Trash persistent explanation: "Recordings stay in Trash for 30 days. After that, their audio, transcripts, SOAP notes, and generated documents are permanently deleted."
- Trash empty: "Trash is empty." + "Recordings moved to Trash appear here for 30 days before permanent deletion."
- Search empty: "No recordings in Trash match your search." + "Clear search".
- Loading: "Loading Trash…" Error: "Couldn't load Trash. Try again." / "Retry".

## Verification gates (run individually; report each result)

1. `cargo test --workspace --lib` — expect green across ~15 lib binaries.
2. `cargo test -p medical-db` — tombstones/content-sync/merge tests must stay green (ledger refusal, restore-vs-tombstone).
3. `npx vitest run` — all frontend suites incl. new component/store/api tests; per-glob coverage floors for `src/lib/components/**` and `src/lib/pages/**` apply.
4. `npm run check` (svelte-check).
5. `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` — both must be clean BEFORE you claim green.
6. Boot smoke: `npm run tauri dev` boots with the "FerriScribe starting" banner and no panic (repository release-gate rule ~/AGENTS.md).

## Report back

- Worktree path + branch + HEAD sha.
- Per deliverable: what changed (files) and the verification-gate results for that item.
- Any deviation from this brief, with the reason (verify against source before deviating; a faithful consequence of the brief's own data is acceptable, a violation of a decision is not).
- The exact Undo-contract decision you implemented (ids snapshot vs backend token) and why.
- What you did NOT change (per Out of scope).
