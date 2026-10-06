# Reversible Delete All + Trash + Day-Scoped Restore (2026-10-06)

**Status:** Plan — awaiting review pass (turing/ada/codie) before zcode dispatch.
**Feature owner:** Andre. **UI design:** ui-consultant (consulted 2026-10-06, session `20261006_064548_bb8337`).
**Restore semantics ruling (Andre):** restore everything soft-deleted **on that exact calendar day** (local time). Not "on or after".

---

## 1. Goal

1. **Delete All becomes reversible**: "Move all to Trash" — all active recordings are soft-deleted (rows tombstoned, FTS de-indexed), with WAV files and RAG vectors preserved for recovery.
2. **30-day retention, then hard delete**: trashed recordings are permanently deleted 30 days after their `deleted_at` — on **every** machine (today the purge runs server-only).
3. **Trash view**: browse trashed recordings, restore per-row, restore all, and **restore by exact calendar day** of deletion.
4. Everything stays PHI-safe, sync-correct, and accessible.

## 2. Current behavior (verified at HEAD)

| Surface | Where | Today |
|---|---|---|
| Single delete | `src-tauri/src/commands/recordings.rs:77` `delete_recording` | Soft delete (`soft_delete`, FTS de-index, WAV+vectors preserved) + tombstone push to paired server (`recordings.rs:117-147`) |
| Undo | `src-tauri/src/commands/recordings.rs:155` `restore_recording` | Clears `deleted_at`, re-indexes FTS, stamps `retention_exempt` (repo `restore()` at `crates/db/src/recordings.rs:585`, exemption at :624). **No sync push — gap.** |
| Delete All | `src-tauri/src/commands/recordings.rs:194` `delete_all_recordings` | **HARD delete**: `DELETE FROM recordings WHERE deleted_at IS NULL` (`crates/db/src/recordings.rs:865`), deletes all RAG vectors, deletes WAV files from disk, resets content-sync cursors (both local pull cursor + `content_sync_push_cursor`). No undo. |
| 30-day purge | `src-tauri/src/sweeps.rs:241` `retention_sweep_tick` Phase 1 | **Server-only** (`is_server` gate at `sweeps.rs:247`; doc lines 228-231). Vectors cleaned, audio removed, `purge_soft_deleted_with_ledger` (`crates/db/src/recordings.rs:800`) writes `purged_recordings` resurrection-block ledger. Pinned client behavior: `retention_sweep_client_trashes_old_but_never_purges` (`sweeps.rs:404`). |
| Retention setting | `src/lib/components/settings/sections/RecordingRetention.svelte` | Options Never/30/90/180/365 (`retention_days`, `src/lib/types/index.ts:230`). Helper already says trashed recordings keep a 30-day undo window and restore exempts from future automatic cleanup. |
| UI | `src/lib/pages/RecordingsTab.svelte` | Delete All button (`:94-99`), confirm dialog claims "This will permanently delete… cannot be undone" (`:134-141`), single-delete confirm says "You can undo this for 8 seconds" (`:128`), Undo toast (`:35-48`). |
| Store | `src/lib/stores/recordings.svelte.ts` | `removeAll()` `:197` clears list; `remove()` `:163` captures `lastDeleted` for undo; `restore()` `:179` reloads. `lastDeleted` `:161` is a single-summary cache, not a batch mechanism. |
| API wrappers | `src/lib/api/recordings.ts:17,20,24` | delete / restore / deleteAll. |

No migration needed: `deleted_at`, `recordings_fts`, `purged_recordings` all exist.

## 3. Design decisions (resolved)

1. **Delete All = soft delete; cursor reset is KILLED.** The current cursor-reset exists only because hard-delete needed a blunt re-sync. With tombstones, the correct mechanism is: soft-delete all locally, push tombstones (batched) to the paired server, let normal sync converge partners. Cursor reset would actually **resurrect** everything from a partner on next pull — it must be removed. (Rationale comment in the command must be rewritten; the "permanently break sync" reasoning no longer applies.)
2. **30-day purge runs on every machine — purge-first ordering (Codie gate 1).** Remove the `is_server` gate on Phase 1, AND reorder the phase so the purge transactions run BEFORE artifact cleanup: list candidates → `purge_soft_deleted_with_ledger` (transaction, returns confirmed purged ids) → delete RAG vectors + audio only for the CONFIRMED purged ids (missing files tolerated). Today vectors/audio are deleted before the purge transaction from an unguarded listing; a restore landing between artifact deletion and the purge transaction leaves a live row with its audio + vectors already gone. Under D7 that window would open daily on every machine. Required in standalone setups and safe in paired setups (ledger refusal on `merge_incoming` is shared code). Update the module doc + the client-never-purges test (invert its assertions: row gone, audio removed, ledger entry written).
3. **Restore-by-date = exact local calendar day** (Andre's ruling; overrides the consultant's "on or after" draft). UI labels it "Deleted on <date>". The frontend computes the local-day interval and sends explicit UTC boundaries; the backend filters `datetime(deleted_at) >= datetime(start) AND datetime(deleted_at) < datetime(end)` (half-open, matching the retention sweep's `datetime()` comparison convention).
4. **Bulk Undo (Delete-All toast) = exact captured ID set.** The toast captures the full set of IDs before invoking; Undo restores exactly those IDs. Never "restore everything deleted since <timestamp>" (consultant flag — could restore unrelated deletes).
5. **All restore paths push a revive to the paired server.** Single restore currently lacks this; add it, plus bulk variants. The revive is a sync recording with `deleted_at = null` (server-side restore-vs-tombstone LWW is already resolved per AGENTS.md).
6. **No new settings, no new migration.** Trash retention is fixed at 30 days (matches the existing sweeper literal). `retention_days` (auto-trash of old active recordings) is unchanged. "Empty Trash" and per-row permanent-delete are explicitly **out of scope** (consultant).
7. **retention_exempt preserved for every restore path.** All bulk/date restores reuse the per-row `restore()` logic (metadata stamp + FTS membership probe + re-index + clear `deleted_at`) inside one transaction, so the exemption contract (already documented in RecordingRetention's hint) holds.
8. **A11y:** ConfirmDialog gets a configurable `role` (`dialog` vs `alertdialog`) + `aria-describedby`; default stays `alertdialog` so existing callers are untouched (`ConfirmDialog.svelte:73`). View switch is accessible tabs. One polite `role="status"` region for results; `role="alert"` only for actionable failures. No PHI in toasts/errors; sanitized backend errors.
9. **Copy:** 8-second-undo wording removed everywhere (toast visibility ≠ retention window). "Move all to Trash" replaces "Delete All".

## 4. Backend work

### 4.1 Repo — `crates/db/src/recordings.rs`
- **`soft_delete_all(conn) -> DbResult<Vec<Uuid>>`** (new): one transaction; per active row, the exact statement pair from `soft_delete` (:512-548) — `UPDATE recordings SET deleted_at = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL` then the guarded FTS `'delete'` via `INSERT INTO recordings_fts(recordings_fts, rowid, …) SELECT 'delete', …`. Returns the ids (for the tombstone push).
- **`list_trashed(conn, limit, offset) -> DbResult<(Vec<TrashedSummary>, u32)>`** (new): `WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC`; summary carries id, filename, patient_name, duration_seconds, created_at, deleted_at; separate `COUNT(*)` for total. Never returns transcript/SOAP content.
- **`restore_all(conn) -> DbResult<Vec<Uuid>>`** (new): one transaction, per-row logic identical to `restore()` (:585-671) — metadata read, `retention_exempt: true` stamp, FTS membership probe (`fts_row_indexed` :565) + guarded re-index, `UPDATE … SET deleted_at = NULL`. Returns restored ids (for the revive push).
- **`restore_deleted_between(conn, start: &str, end: &str) -> DbResult<Vec<Uuid>>`** (new): same per-row restore loop restricted to `WHERE deleted_at IS NOT NULL AND datetime(deleted_at) >= datetime(?start) AND datetime(deleted_at) < datetime(?end)`.
- `delete_all` (:865) becomes unused by the command; keep or remove per lint (prefer remove + delete the misleading doc comment; check other call sites first).

### 4.2 Commands — `src-tauri/src/commands/recordings.rs`
- **`delete_all_recordings`** (:194): replace body — transaction: `soft_delete_all`; **no** vector deletion, **no** audio deletion, **no** cursor reset (`set_cursor` + `content_sync_push_cursor` UPDATE removed). Return count. After commit: fire-and-forget batched tombstone push to the paired server (same shape as single-delete push :117-147, looping `build_sync_recording` + `deleted_at`, one `remote.push`).
- **`restore_recording`** (:155): add the revive push (deleted_at null) mirroring the delete push.
- **`restore_all_trashed`** (new): `restore_all` → count; revive push for all ids.
- **`restore_recordings_deleted_between(start_iso, end_iso)`** (new): validate both parse as RFC3339 and `start < end`; call repo; count; revive push.
- **`count_recordings_deleted_between(start_iso, end_iso)`** (new): same RFC3339 + `start < end` validation and the same half-open `datetime()` interval; returns `u32`. The count-preview wire surface — never derive the preview from paginated trash pages (Codie gate 2).
- **`list_trashed_recordings(limit, offset)`** (new): page + total.
- **`count_trashed_recordings()`** (new, trivial) for the trash badge — or fold total into `list_trashed_recordings` (prefer the fold; fewer commands).
- **`count_recordings()`** (new, trivial: `RecordingsRepo::count` :886) for the "Move all N recordings to Trash?" authoritative count (store `list.length` is paginated — consultant flag).
- Register all in `src-tauri/src/lib.rs` `invoke_handler`.

### 4.3 Sweeper — `src-tauri/src/sweeps.rs`
- **Phase 1 purge runs unconditionally AND purge-first**: drop the `is_server` block gate (statement at :247); reorder so `list_soft_deleted_older_than` → `purge_soft_deleted_with_ledger` (returns confirmed ids) → THEN vector + audio cleanup for confirmed ids only (today artifacts are cleaned before the purge from an unguarded listing — the restore-vs-purge race window, now daily on every machine). Update the module/function doc (lines 225-234).
- `is_server` parameter: remove it (call sites: `retention_sweep_tick(&db, is_server)` :345 and tests). `spawn_retention_sweeper` :339 no longer needs `load_server_config`.
- Tests: rewrite `retention_sweep_client_trashes_old_but_never_purges` (:404) → client **purges** aged tombstones, asserting row gone + audio removed + ledger written (invert, don't just rename); keep the server test (:436) green; add the race test: a restore landing between listing and purge leaves the row ACTIVE with its audio file intact on disk.

### 4.4 Register commands (`src-tauri/src/lib.rs:414-416` area) — add new ones alongside existing recordings commands.

## 5. Frontend work

### 5.1 API — `src/lib/api/recordings.ts`
Add wrappers: `listTrashedRecordings(limit, offset)`, `restoreAllTrashed()`, `restoreRecordingsDeletedBetween(startIso, endIso)`, `countRecordings()`. Keep existing three. Tests in `src/lib/api/recordings.test.ts`.

### 5.2 Store — `src/lib/stores/recordings.svelte.ts`
- Trash state: `trashedList`, `trashedTotal`, `trashedLoading`, `trashedError`, `trashedRequestId` (monotonic, same discipline as `listRequestId` :144), search state **separate** from Active. `loadTrashed()`, `loadMoreTrashed()`, `searchTrashed(q)`.
- `removeAll()` (:197): capture the **full id set** before invoking (the authoritative active list — capture ids from the loaded pages plus a backend count; simplest robust approach: backend `delete_all_recordings` returns the count and the UI's Undo calls `restoreAllTrashed()` **only when no other deletion can interleave**… see decision 4: prefer capturing exact ids via a `list_active_ids` — hmm, cheaper: batch Undo restores the snapshot ids returned by a new lightweight `delete_all_recordings` behavior — but the command currently returns count only. **Resolution:** have `delete_all_recordings` return the ids (or a `count` and let Undo use `restore_all_trashed` guarded by a version token). Simplest correct contract: `delete_all_recordings -> { count, ids }`? Returning the full id list is fine for realistic library sizes (<10k) and makes Undo exact. **Chosen: command returns `DeletedAll { count: u32, ids: string[] }`; Undo restores exactly those ids via `restoreRecordings(ids)` (new batch command).** Add `restoreRecordings(ids)` command + wrapper for the batch-undue path specifically.)
  - Also: `removeAll` clears Active list + selection (existing), then sets a `lastDeletedAllIds` for the toast.
- Restore paths invalidate: after any restore, refresh Active list + trash totals + `lastDeleted`-style state. Monotonic request-ids guard stale trash loads during mutations.
- `restoreByDate(day: Date)`: compute local-day boundaries (`start = new Date(y,m,d,0,0,0)`; `end = new Date(y,m,d+1,0,0,0)`), send `.toISOString()`, invoke `restoreRecordingsDeletedBetween`.

### 5.3 Components
- **`RecordingViewSwitch.svelte`** (new): Active | Trash (N) — accessible tabs (arrow-key nav, `role="tablist"/"tab"/"tabpanel"`), visible even when Active is empty (empty-state must not hide the route to recovery).
- **`TrashPanel.svelte`** (new): retention explanation line (consultant copy), toolbar — "N recordings in Trash", "Restore by deletion date…", "Restore all…"; search-empty, loading, error states; note bulk actions cover ALL trashed records, not the loaded page/search ("N matches · M in Trash" when filtered). Sorts newest deletion first.
- **`TrashRecordingRow.svelte`** (new): compact, **not** the RecordingCard whole-row-click pattern; name, Recorded <date/time> · <duration>, Moved to Trash <date/time>, <N days remaining> · Permanent deletion after <datetime>; neutral text, restrained amber near expiry, "Less than 1 day remaining" under a day, **"Pending permanent deletion"** once eligible (never negative days); no transcript excerpts; Restore always visible (not hover-only); no processing/generation controls.
- **`RestoreByDateDialog.svelte`** (new): one dialog — native date input (`<input type="date">`, labelled; reject invalid/future dates), count preview after valid date, confirm. Copy (day-exact adaptation of consultant draft):
  - Title "Restore by deletion date"; field "Deleted on"; helper "Uses the date recordings were moved to Trash, not the date they were recorded. Searches all of Trash, regardless of the current search. Times use <this machine's timezone>."
  - Preview "Restore N recordings moved to Trash on <long-form date>?" / "They will return to Active with their saved audio and documents." Buttons "Cancel" / "Restore N recordings".
  - Zero: "No recordings currently in Trash match this date." Restore disabled.
  - Counting… / Restoring… states, duplicate-submit guard; success stays in Trash, counts refresh, announce "N recordings restored to Active."; failure retains date + dialog, "Couldn't restore recordings. Try again."
  - **Preview and execution must reference the same candidate set**: report actual restored/skipped counts vs preview total (sweeper may purge in between — race, consultant flag).
- **`ConfirmDialog.svelte`** (modify): configurable `role` prop (`dialog` default preserved as `alertdialog` for existing callers — verify each existing caller's intent) + `aria-describedby` pointing at the body; preserve overlay stack, focus trap, Escape, safe initial focus. If the invoking row disappears, move focus to the next Restore button or the Trash heading — never document.body.
- **`RecordingsTab.svelte`** (modify): mount `RecordingViewSwitch` above `SearchBar`; per-view SearchBar; Move-all-to-Trash confirm (see copy); post-move empty state with "View Trash" action; single-delete confirm + toast copy updates (see copy).
- **`ToastContainer`** (modify): announcement semantics — one polite `role="status"` for results; `role="alert"` for actionable failures; pause actionable-toast dismissal on focus/hover; **one toast per bulk operation**; pending/duplicate guards; avoid double-announcing an inline status + toast for the same event.
- **`RecordingRetention.svelte`** (modify): option 0 copy "Never (keep forever)" → **"Never automatically move to Trash"** (consultant; avoids implying immunity from manual deletion or trash expiry).

### 5.4 Copy (consultant, day-exact adaptation)
- Toolbar: "Move all to Trash".
- Dialog title: "Move all N recordings to Trash?" (N from `countRecordings` — authoritative, not `list.length`).
- Body: "This moves all active recordings to Trash, including recordings outside the current search. Their audio, transcripts, SOAP notes, and generated documents are kept for 30 days. You can restore them from Trash during that time. After 30 days, they are permanently deleted." Buttons "Cancel" / "Move N recordings to Trash". **Restrained styling** — not the irreversible-warning treatment.
- Toast: "N recordings moved to Trash. Available to restore for 30 days." Action "Undo" → success "N recordings restored to Active."
- Single delete: title "Move recording to Trash?"; body "You can restore this recording from Trash for 30 days. After that, it and its saved audio and documents are permanently deleted." Action "Move to Trash". Toast "Recording moved to Trash." / "Undo".
- Persistent trash explanation: "Recordings stay in Trash for 30 days. After that, their audio, transcripts, SOAP notes, and generated documents are permanently deleted."
- Trash empty: "Trash is empty." / "Recordings moved to Trash appear here for 30 days before permanent deletion."
- Search empty: "No recordings in Trash match your search." + "Clear search".

## 6. Acceptance criteria (numbered deliverables for zcode)

- **D1 Move-all-to-Trash**: soft-deletes every active row (FTS de-indexed, WAV + vectors preserved); authoritative count; confirm dialog copy; exact-id batch Undo restores ONLY that set; tombstones pushed to paired server; sync cursors untouched.
- **D2 Trash view**: browse (paginated, newest first), total badge, states (empty/loading/error/search-empty), days-remaining rules (incl. "Pending permanent deletion"), per-row restore.
- **D3 Restore all**: confirmation + count, retention_exempt stamped, revives pushed.
- **D4 Restore by exact day**: date input validation, count preview, actual-vs-preview reporting, revives pushed; a recording deleted on another day is NOT restored.
- **D5 30-day purge everywhere**: sweeper Phase 1 unconditional on client AND server; audio + vectors removed; `purged_recordings` ledger written; `merge_incoming` still refuses stale copies (existing tests stay green).
- **D6 Settings copy**: "Never automatically move to Trash"; hint text retained.
- **D7 A11y**: ConfirmDialog role configurable + described-by; tablist semantics; single status region; no PHI in toasts/errors (sanitized only).

## 7. Verification gates (run individually)

1. `cargo test --workspace --lib` (repo baseline ~15 lib binaries, ~70s)
2. `cargo test -p medical-db` (content-sync/tombstone merge tests)
3. `npx vitest run` (incl. new component/store/api tests; coverage floors apply — new components under `src/lib/components/**` / `src/lib/pages/**` per-glob floors)
4. `npm run check` (svelte-check)
5. `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings`
6. Manual/boot: `npm run tauri dev` — move-all→trash→undo→restore-by-day round trip; verify WAVs still on disk during trash; verify purge after back-dating a tombstone (`datetime(deleted_at)`); paired-server scenario if available.

## 8. Out of scope (explicit)

- "Empty Trash" button and per-row permanent-delete (consultant).
- Configurable trash retention (fixed 30 days; matches existing sweeper literal).
- Sort/group options beyond newest-deletion-first.
- Changing `retention_days` semantics.
- Date-range restore (Andre chose exact single day).

## 9. References

- UI consultation: ui-consultant reply in profile state.db, session `20261006_064548_bb8337` (recovered copy: `~/.hermes/briefs/ui-consultant-trash-restore-reply.txt`).
- Zcode prompt: `docs/zcode/trash-restore.md` (this feature's build brief).
- Deletion-model sync design (resolved items): `AGENTS.md` "Sync/merge remaining items … deletion-model redesign shipped on feat/deletion-model".
