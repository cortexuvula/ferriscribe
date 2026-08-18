# Deletion-Model Redesign — Design Spec

**Date:** 2026-08-17
**Status:** Awaiting user review
**Scope decision (taken on the user's behalf — confirm or override):** Core deletion model (restore-vs-tombstone LWW, chips/dictionary tombstone propagation, purge-time resurrection) **plus** the minimal Equal-tie mitigation in `build_sparse_fields`, because it lives in the same merge function this redesign rewrites. Clock-skew cursor pinning, the `take(10)` audio-upload cap, and full revision-coverage for all writers remain tracked separately in AGENTS.md.

## Problem

Three confirmed bugs share one root cause — deletions are represented only as mutable row state with no ordering between competing delete/restore/write operations, and tombstones do not reliably reach every replica:

1. **Restores never survive sync.** `merge_incoming`'s tombstone branch (`crates/db/src/content_sync.rs`) deletes a live local row whenever the remote carries any tombstone, with no timestamp comparison. A restore (which clears `deleted_at` and bumps `updated_at`) cannot land on a tombstoned peer — field updates stop at the `deleted_at IS NULL` guards — and the next pull re-tombstones the restored machine.
2. **Chips/dictionary deletions resurrect.** The server's sync endpoints return active items only and prune tombstones after 30 days, so clients never learn of a deletion; a stale client's next push re-inserts the item practice-wide. `increment_use` additionally resurrects a tombstone on a mere click. The dictionary client never merges server state locally at all.
3. **Purged recordings resurrect.** The server's 30-day purge hard-deletes rows and audio with no trace. A client that missed the tombstone pushes its stale live copy later, and the merge inserts it as brand-new — deleted PHI returns practice-wide.

Related (fixed as a rider because it is the same function): **Equal-tie silent divergence** — `build_sparse_fields` prefers field-revision timestamps over the row timestamp, so a writer that bumps only the row (transcription/generation completion) ties against a stale revision on the server and the Equal arm keeps the server's old value silently.

## Chosen approach

**Timestamped tombstone LWW + purge ledger**, inside the existing mutable-row model. Rejected alternatives: an append-only event log with per-replica clocks (correct but a wholesale rewrite of every sync path for a 2–10 machine practice — YAGNI), and server-authoritative deletes with restore-as-new-copy (breaks the existing undo feature and id-linked history, doesn't address chips/dict).

Key leverage point: server and clients run the **same** `merge_incoming` from `medical-db`. The merge-semantics fix lands once and applies to both directions.

## Design

### 1. Recordings merge — timestamped tombstone LWW

All timestamp comparisons **parse** timestamps (`parse_db_timestamp`) — never string-compare; legacy space-separated and `Z`-suffixed formats coexist in real data and sort wrongly as strings.

| Incoming (remote) | Local state | Resolution |
|---|---|---|
| tombstoned (`deleted_at = T_d`) | live, `updated_at = T_u` | `T_d > T_u` → propagate deletion; `T_u > T_d` → keep local (restored) row and treat the incoming tombstone as superseded; equal → tombstone wins |
| live (`deleted_at = None`), `updated_at = T_u` | tombstoned (`deleted_at = T_d`) | `T_u > T_d` → **restore locally**: clear `deleted_at`, apply fields, re-index FTS; `T_d ≥ T_u` → keep tombstone (fields stay guarded) |

A winning restore propagates with no extra machinery: the restored row's `updated_at` is newer than the peer's tombstone timestamp, so `changed_since` includes it in the next push and the peer's merge takes the "live incoming vs tombstoned local" row above. The loop closes through the existing cursors.

Implementation shape: an explicit pre-check in `merge_incoming` (read local `deleted_at` + `updated_at`, branch on the parsed comparison, then write) — no clever SQL guard changes. Field application to a tombstoned row remains blocked (the `deleted_at IS NULL` guards stay) **unless** the row-level comparison authorized a restore in the same merge step.

FTS discipline (the crate has hard-won trigger rules — follow `soft_delete`/`restore` exactly):
- sync-tombstone: de-index from `recordings_fts` before the UPDATE (also fixes the review finding that sync tombstones left rows indexed).
- sync-restore: re-insert into `recordings_fts` before clearing `deleted_at`.
- The row-level `updated_at` bump (`?1 > updated_at` guard) gains the same authorization: allowed on a tombstoned row only when `?1 > deleted_at`.

`retention_exempt` continues to ride in metadata; a restore re-stamps it as today (unchanged behavior).

### 2. Purge safety — `purged_recordings` ledger

New migration (next m0NN): `CREATE TABLE purged_recordings (id TEXT PRIMARY KEY, purged_at TEXT NOT NULL)`.

- `sweeps.rs` server purge: write ledger rows for every purged id **in the same transaction** as `purge_soft_deleted`.
- `merge_incoming` insert path (no local row): before `insert_remote_recording`, look up the ledger. If ledgered with `purged_at ≥ incoming.updated_at` → refuse the insert (drop, increment a refusal counter that is logged — counts only, no PHI). Rationale for the timestamp clause: a push carrying edits made *before* the purge is exactly the stale copy we must refuse; content genuinely re-created after a purge cannot exist because purged recordings are user-deleted.
- Ledger is id-only (no PHI), append-only, never pruned — bounded by practice-wide deletion volume; even a decade of deletions is kilobytes.

**Client convergence:** pull response gains an optional field `purged: Vec<PurgedRef>` where `PurgedRef = { id: String, purged_at: String }`, populated with ledger entries newer than the client's pull cursor (both client and server structs updated; serde defaults keep old⇄new binaries interoperable — no `deny_unknown_fields` anywhere in the sync types). On receipt, a client holding a **live** local copy for a purged id tombstones it locally with `deleted_at = purged_at` (its own 30-day sweeper finishes the cleanup); a client already tombstoned or without the row does nothing. Tombstoning (not hard delete) keeps the change FTS-safe through the same path as (1).

### 3. Chips & dictionary — tombstone propagation

- **Amendment (implementation review):** dictionary propagation uses a NEW `POST /v1/user-dictionary/sync-full` endpoint (`Vec<UserDictEntry>` incl. tombstones) — the existing endpoints speak `Vec<String>` and cannot carry tombstones without breaking old clients. New clients fall back to the legacy `/sync` on 404/405; the fallback synthesizes entries stamped BEFORE the request (a deletion landing mid-transit must outrun the synthesized stamp) and displays the server's active words (legacy responses cannot teach the local store about deletions). The dictionary client gains its missing local merge via the new endpoint.
- **Amendment (implementation review):** ledger refusal is id-only, not timestamp-compared. A machine offline across a deletion can *edit* its stale copy (fresh `updated_at`, same UUID) and pierce a timestamp check; genuinely re-created content always gets a new UUID, so same-UUID + any ledger hit is always a stale copy.
- Server sync/list endpoints for chips switch from `list_active` to full-list-with-tombstones. Both repos' `merge_incoming` implementations already handle tombstones (tie-break: tombstone wins; `use_count` MAX), so old chips clients are unaffected and the tie-break becomes reachable. Tombstones are small (a condition label / dictionary word + timestamps) — full lists remain tiny.
- Tombstone prune retention for chips/dict extends **30 days → 365 days** (code change in the sync handlers). These tables are tiny; a year of tombstones costs nothing and makes the stale-client resurrection window negligible without introducing a ledger for them.
- **Behavior change (implemented, spec sign-off given):** `increment_use` no longer resurrects a tombstoned chip — a click on a stale list currently undeletes the condition practice-wide once tombstones propagate. Explicit add still resurrects (existing documented behavior); the fresh-install self-healing path (no row at all → create with count 1) is preserved.

### 4. Equal-tie mitigation (rider)

`build_sparse_fields` (`src-tauri/src/commands/content_sync.rs`): for each field, use `max(field_revision.updated_at, row.updated_at)` (parsed comparison) as the wire timestamp. A stale revision can no longer mask a newer row-level write, so transcription/generation completions stop tying silently against pre-sync revisions. The Equal arm of field LWW (keep local, no conflict) then only fires on genuine ties. Full revision-coverage for every `RecordingsRepo::update` caller stays out of scope.

### 5. What does not change

- Wire formats otherwise unchanged (`SyncRecording` already carries `deleted_at`); one additive optional pull field.
- Client/server roles, cursors, `changed_since` semantics, the SSE layer, audio sync.
- Local soft-delete/restore/purge/undo UX.
- `use_count` MAX reconciliation.

## Error handling & edge cases

- **Mixed timestamp formats:** every comparison in the new logic parses via `parse_db_timestamp`; unparseable timestamps sort as "oldest" (they are legacy rows; losing to a parsed timestamp is the safe direction for deletions and restores alike).
- **Clock skew between machines:** LWW by wall clock can still let a skewed clock win a specific delete/restore duel — accepted (same exposure as every other field); the fleet-wide cursor-pinning failure mode is explicitly out of scope.
- **Simultaneous delete on A, restore on B (no causal order):** resolved by timestamp; tie → tombstone wins. A clinician who restores after seeing the deletion on their machine produces a strictly newer timestamp and wins — the common case.
- **Old server, new client:** `purged` field absent → default empty, client unaffected. **New server, old client:** unknown field ignored; old client's merge still has the old tombstone-always-wins behavior until upgraded (no worse than today).
- **Ledger refusal vs. first-time push of a legitimately new recording:** impossible collision — new recordings get fresh UUIDs; ledgered ids are purged ones.

## Testing

- `crates/db` integration tests (new file `deletion_model.rs`): each table row of §1; ledger refusal; ledger pass-through for non-ledgered ids; FTS index state after sync-tombstone and sync-restore (query `recordings_fts` directly); purged-ref client tombstoning (live → tombstoned, tombstoned → untouched); chips tombstone round-trip incl. tie-break and use_count MAX; dictionary client merge behavior; increment_use no-resurrection; Equal-tie rider (stale revision vs newer row).
- Server handler tests: pull response includes `purged` filtered by cursor; chips/dict endpoints return tombstones.
- All existing merge/sync/retention tests must pass unchanged except tests that assert the old always-delete behavior (updated to the new LWW contract).

## Migration & rollout

1. Migration adds `purged_recordings` (no data backfill needed — the ledger starts empty; recordings purged *before* this change are gone and their stale copies can still resurrect once, accepted as a one-time historical artifact).
2. Chips/dict retention change is code-only.
3. Single release carries server + client changes; mixed-version windows behave per the compatibility notes above.
