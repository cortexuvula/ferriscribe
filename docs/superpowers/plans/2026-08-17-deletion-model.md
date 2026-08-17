# Deletion-Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make deletions, restores, and purges converge correctly across machines: timestamped tombstone LWW in the shared merge, a purge ledger that blocks resurrection, tombstone propagation for condition chips and the user dictionary, and the Equal-tie field-timestamp rider.

**Architecture:** Server and clients run the same `ContentSyncRepo::merge_incoming` (medical-db) — the LWW fix lands once for both directions. A new id-only `purged_recordings` ledger (m018) is written transactionally by the server purge and consulted before any merge-insert; purged ids reach clients via a new optional `purged` field on the pull response. Chips gain tombstone propagation by serving full lists (wire unchanged — `ConditionChip` already carries `deleted_at`); the dictionary needs a NEW `/v1/user-dictionary/sync-full` endpoint speaking `Vec<UserDictEntry>` because the existing endpoints speak `Vec<String>` and cannot carry tombstones (old clients must keep working).

**Tech Stack:** Rust (rusqlite, tokio, axum), existing migration engine, `cargo test -p medical-db` / `-p rust-medical-assistant`.

**Spec:** `docs/superpowers/specs/2026-08-17-deletion-model-design.md` (amended: dict propagation via new endpoint + legacy fallback, not a response-type change).

**Guards for every task:** PHI rule — log ids/counts only, never content. FTS discipline — `recordings_fts` is external-content; every tombstone de-indexes after UPDATE, every restore re-indexes before UPDATE (copy `soft_delete`/`restore` in `crates/db/src/recordings.rs` exactly). All timestamps compared via parsed datetimes, never strings.

---

### Task 1: m018 migration — `purged_recordings` ledger

**Files:**
- Create: `crates/db/src/migrations/m018_purge_ledger.rs`
- Modify: `crates/db/src/migrations/mod.rs`

- [ ] **Step 1: Write the migration**

```rust
//! m018: purge ledger. Records the ids of recordings permanently purged by
//! the office server's tombstone sweeper so `merge_incoming` can refuse
//! re-insertion of a stale live copy pushed by a machine that missed the
//! deletion. Id-only — no PHI.

use rusqlite::Connection;

use crate::DbResult;

pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS purged_recordings (
            id TEXT PRIMARY KEY,
            purged_at TEXT NOT NULL
        )",
        [],
    )?;
    Ok(())
}
```

In `mod.rs`: add `pub mod m018_purge_ledger;` after `m017_condition_chip_use_count;` and append to `all_migrations()`:

```rust
        Migration {
            version: 18,
            name: "purge_ledger",
            up: m018_purge_ledger::up,
        },
```

- [ ] **Step 2: Verify** — `cargo test -p medical-db --lib migrations` passes; then `cargo run --quiet --bin true 2>/dev/null || true` is unnecessary — instead run the in-memory check via existing tests: `cargo test -p medical-db --lib` (open_in_memory runs all migrations).

- [ ] **Step 3: Commit** — `git add crates/db/src/migrations/ && git commit -m "feat(db): m018 purge ledger table"`

---

### Task 2: LWW timestamp comparison helper

**Files:**
- Modify: `crates/db/src/content_sync.rs` (add near top, after imports; tests in existing `#[cfg(test)] mod` or new one)

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod lww_ts_tests {
    use super::cmp_lww_timestamps;
    use std::cmp::Ordering;

    #[test]
    fn rfc3339_offsets_compare_chronologically() {
        // String comparison would order these wrongly (Z vs +00:00).
        assert_eq!(
            cmp_lww_timestamps("2026-01-02T03:04:05Z", "2026-01-02T03:04:05+00:00"),
            Ordering::Equal
        );
        assert_eq!(
            cmp_lww_timestamps("2026-01-02T03:04:05.500Z", "2026-01-02T03:04:05Z"),
            Ordering::Greater
        );
    }

    #[test]
    fn legacy_space_format_compares_chronologically() {
        // ' ' (0x20) < 'T' (0x54): string comparison puts the LATER
        // space-format timestamp before the earlier RFC one on the same day.
        assert_eq!(
            cmp_lww_timestamps("2026-01-02 05:00:00", "2026-01-02T03:04:05Z"),
            Ordering::Greater
        );
        assert_eq!(
            cmp_lww_timestamps("2026-01-02 01:00:00", "2026-01-02T03:04:05Z"),
            Ordering::Less
        );
    }

    #[test]
    fn unparseable_sorts_oldest() {
        assert_eq!(cmp_lww_timestamps("garbage", "2026-01-02T03:04:05Z"), Ordering::Less);
        assert_eq!(cmp_lww_timestamps("2026-01-02T03:04:05Z", "garbage"), Ordering::Greater);
        assert_eq!(cmp_lww_timestamps("", ""), Ordering::Equal);
    }
}
```

- [ ] **Step 2: Run** — `cargo test -p medical-db --lib lww_ts` → compile FAIL (fn undefined).

- [ ] **Step 3: Implement**

```rust
/// Parse a stored timestamp for LWW decisions. Accepts both legitimate
/// stored formats: RFC 3339 (with any offset) and SQLite's space-separated
/// `datetime('now')` format. Returns `None` for anything else.
fn parse_lww_timestamp(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

/// Chronological comparison for LWW decisions. String comparison is wrong
/// across the two stored formats (`' '` < `T`, `Z` vs `+00:00`), so both
/// sides are parsed. Unparseable timestamps sort as the OLDEST value —
/// legacy or corrupt data must not win delete/restore decisions.
fn cmp_lww_timestamps(a: &str, b: &str) -> std::cmp::Ordering {
    match (parse_lww_timestamp(a), parse_lww_timestamp(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
    }
}
```

- [ ] **Step 4: Run** — `cargo test -p medical-db --lib lww_ts` → PASS. `cargo fmt --all`.

- [ ] **Step 5: Commit** — `git commit -am "feat(db): parsed LWW timestamp comparison for sync decisions"`

---

### Task 3: FTS-safe sync tombstone/restore helpers

**Files:**
- Modify: `crates/db/src/content_sync.rs` (impl ContentSyncRepo), tests appended

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod sync_tombstone_tests {
    use super::*;
    use crate::Database;
    use medical_db_test_helpers::seed_recording; // if a helpers module exists; otherwise inline-seed as in retention.rs tests

    fn fts_row_present(conn: &Connection, id: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM recordings_fts WHERE id = ?1",
            [id],
            |r| r.get::<_, i64>(0),
        ).unwrap() > 0
    }
}
```
(If no test-helpers module exists, seed inline: `let mut rec = medical_core::types::recording::Recording::new("x.wav", "/audio/x.wav".into()); RecordingsRepo::insert(&conn, &rec);` — see `crates/db/tests/retention.rs` for the pattern.)

Tests:
1. `sync_tombstone_hides_row_and_deindexes_fts` — seed live recording → `ContentSyncRepo::sync_tombstone(&conn, &rec.id, "2026-06-01T00:00:00Z")` → `deleted_at` set to the given ts, `updated_at` = ts, FTS row absent.
2. `sync_restore_revives_row_and_reindexes_fts` — seed, `RecordingsRepo::soft_delete`, then `ContentSyncRepo::sync_restore(&conn, &rec.id, "2026-06-02T00:00:00Z")` → `deleted_at` NULL, FTS row present.
3. `sync_tombstone_on_missing_row_is_noop` — no row → Ok(()), no error.
4. `sync_restore_on_live_row_is_noop` — live row → Ok(()), unchanged.

- [ ] **Step 2: Run** — FAIL (methods undefined).

- [ ] **Step 3: Implement** (in `impl ContentSyncRepo`, mirroring `soft_delete`/`restore` FTS discipline exactly — de-index AFTER the tombstone UPDATE, re-index BEFORE the restore UPDATE):

```rust
/// Tombstone a live local row from a sync peer's deletion, with a
/// caller-supplied timestamp (the deletion's own `deleted_at`). Mirrors
/// `RecordingsRepo::soft_delete`'s FTS discipline: UPDATE first, then
/// remove the FTS row with the *currently indexed* column values.
/// Missing row / already-tombstoned → clean no-op.
    pub fn sync_tombstone(conn: &Connection, id: &str, deleted_at: &str) -> DbResult<()> {
        conn.execute(
            "UPDATE recordings SET deleted_at = ?1, updated_at = ?1
             WHERE id = ?2 AND deleted_at IS NULL",
            params![deleted_at, id],
        )?;
        if let Err(e) = conn.execute(
            "INSERT INTO recordings_fts(recordings_fts, rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
             SELECT 'delete', rowid, id, filename, transcript, soap_note, referral, letter, patient_name
             FROM recordings WHERE id = ?1 AND deleted_at IS NOT NULL",
            [id],
        ) {
            tracing::warn!(error = %e, "sync_tombstone: failed to remove recording from FTS index");
        }
        Ok(())
    }

    /// Revive a tombstoned local row from a sync peer's newer restore, with
    /// the restore's `updated_at`. Mirrors `RecordingsRepo::restore`'s FTS
    /// discipline: re-index BEFORE the UPDATE (the update trigger fires a
    /// 'delete' for old values that must match indexed state). Does NOT
    /// stamp `retention_exempt` — the origin machine's restore stamped it
    /// and it travels in the synced `metadata` field. Missing / live row →
    /// clean no-op.
    pub fn sync_restore(conn: &Connection, id: &str, updated_at: &str) -> DbResult<()> {
        let tombstoned: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM recordings WHERE id = ?1 AND deleted_at IS NOT NULL)",
                [id],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !tombstoned {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO recordings_fts(rowid, id, filename, transcript, soap_note, referral, letter, patient_name)
             SELECT rowid, id, filename, transcript, soap_note, referral, letter, patient_name
             FROM recordings WHERE id = ?1",
            [id],
        )?;
        conn.execute(
            "UPDATE recordings SET deleted_at = NULL, updated_at = ?1
             WHERE id = ?2 AND deleted_at IS NOT NULL",
            params![updated_at, id],
        )?;
        Ok(())
    }
```

- [ ] **Step 4: Run** — PASS; `cargo test -p medical-db --lib` (no regressions).

- [ ] **Step 5: Commit** — `git commit -am "feat(db): FTS-safe sync_tombstone/sync_restore helpers"`

---

### Task 4: `merge_incoming` — tombstone LWW, restore path, ledger refusal

**Files:**
- Modify: `crates/db/src/content_sync.rs::merge_incoming` (deletion-handling block at ~line 427-486 and insert paths at ~488-515), tests

- [ ] **Step 1: Write failing tests** (in `crates/db/tests/deletion_model.rs`, new integration test file — use the seeding pattern from `retention.rs`):

1. `remote_tombstone_newer_than_local_live_deletes` — seed live rec with `updated_at` = T1; incoming `SyncRecording{id, deleted_at: Some(T2>T1), updated_at: T2, fields: {}}` → merge → row tombstoned, FTS absent.
2. `local_live_newer_than_remote_tombstone_wins` — seed live with `updated_at` = T2; incoming tombstone `deleted_at = T1 < T2` → merge → row stays live.
3. `remote_live_newer_than_local_tombstone_restores` — seed, `soft_delete` at T1; incoming live row `updated_at = T2 > T1` with a `soap_note` field @T2 → merge → row live, soap applied, FTS present.
4. `remote_live_older_than_local_tombstone_stays_deleted` — same but `updated_at = T0 < T1` → row stays tombstoned, soap NOT applied.
5. `tie_tombstone_wins` — live local `updated_at` == tombstone `deleted_at` → deleted.
6. `purged_recording_refused_on_insert` — insert ledger row `(id, purged_at=T2)` via SQL; incoming live `SyncRecording{id, updated_at: T1 < T2, fields:{}}` (no local row) → merge → NO row inserted.
7. `non_ledgered_insert_unchanged` — same shape, no ledger row → row inserted (existing behavior).

Building a `SyncRecording` by hand: see the struct in `crates/db/src/content_sync.rs` (`id: String, filename: String, created_at, updated_at, deleted_at: Option<String>, patient_name, duration_seconds, file_size_bytes, stt_provider, ai_provider, fields: HashMap<String, SyncFieldValue>, tags/metadata as per struct`). Copy field names exactly from the struct definition.

- [ ] **Step 2: Run** — `cargo test -p medical-db --test deletion_model` → FAIL.

- [ ] **Step 3: Implement.** In the deletion-handling block replace the `None => { propagate deletion ... }` arm (currently unconditional) with timestamped LWW, using `sync_tombstone`:

```rust
match local_deleted {
    None => {
        // Local live vs remote tombstone → newer wins; tie → tombstone
        // (consistent with chips/dict tie-break).
        let local_ts = local_updated.unwrap_or_default();
        if cmp_lww_timestamps(remote_deleted, &local_ts) != std::cmp::Ordering::Less {
            Self::sync_tombstone(conn, id_str, remote_deleted)?;
            changed.push(id_str.clone());
            tracing::info!(
                recording_id = %id_str,
                "sync: propagated remote deletion"
            );
        } else {
            // Local row is newer — a restore happened here after the remote
            // deletion. Keep the local live row; the tombstone is superseded
            // and peers converge when this row pushes.
            tracing::debug!(
                recording_id = %id_str,
                "sync: local row newer than remote tombstone; kept live"
            );
        }
    }
    Some(local_ts) => { /* existing both-deleted arm unchanged */ }
}
```
(The `local_deleted` read currently selects only `deleted_at`; extend the SELECT to `deleted_at, updated_at`.)

Insert-path guard — add before BOTH `insert_remote_recording` call sites (tombstone-for-unknown and new-recording):

```rust
if Self::purge_ledger_refuses(conn, id_str, &remote.updated_at) {
    tracing::warn!(
        recording_id = %id_str,
        "sync: refused re-insert of purged recording (stale copy from a machine that missed the deletion)"
    );
    continue;
}
```

```rust
/// True when the purge ledger records this id with a `purged_at` at or
    /// after the incoming row's `updated_at` — i.e. the push is a stale copy
    /// of a recording the practice deleted and the server purged.
    fn purge_ledger_refuses(conn: &Connection, id: &str, incoming_updated_at: &str) -> bool {
        conn.query_row(
            "SELECT purged_at FROM purged_recordings WHERE id = ?1",
            [id],
            |r| r.get::<_, String>(0),
        )
        .map(|purged_at| {
            cmp_lww_timestamps(&purged_at, incoming_updated_at) != std::cmp::Ordering::Less
        })
        .unwrap_or(false)
    }
```

Restore path — in the "Existing recording → per-field LWW" section, BEFORE `let local_revisions = ...`:

```rust
// Local tombstone check: a live incoming row newer than the local
// tombstone is a restore — clear the tombstone (FTS-safe) so the field
// merge below is unblocked. A live incoming row that is NOT newer loses
// to the tombstone: skip field merge entirely (the apply guards would
// no-op it anyway; skipping avoids the 0-row warn spam).
let local_deleted_at: Option<String> = conn
    .query_row(
        "SELECT deleted_at FROM recordings WHERE id = ?1",
        [id_str],
        |r| r.get(0),
    )
    .unwrap_or(None);
if let Some(local_del) = local_deleted_at {
    if remote.deleted_at.is_none()
        && cmp_lww_timestamps(&remote.updated_at, &local_del) == std::cmp::Ordering::Greater
    {
        Self::sync_restore(conn, id_str, &remote.updated_at)?;
        changed.push(id_str.clone());
        tracing::info!(
            recording_id = %id_str,
            "sync: restored recording (remote row newer than local tombstone)"
        );
    } else {
        // Tombstone stands — fields stay guarded.
        continue;
    }
}
```

- [ ] **Step 4: Run** — new tests PASS; `cargo test -p medical-db` (ALL suites — lib + integration) PASS. Update any pre-existing test that asserted always-delete (check `content_sync_edge_cases.rs`).

- [ ] **Step 5: Commit** — `git commit -am "feat(db): timestamped tombstone LWW + restore + purge-ledger refusal in merge_incoming"`

---

### Task 5: Purge writes the ledger

**Files:**
- Modify: `crates/db/src/recordings.rs` (`purge_soft_deleted` — or add sibling), `src-tauri/src/sweeps.rs`, tests

- [ ] **Step 1: Failing test** (in `crates/db/tests/deletion_model.rs`):

`purge_records_ledger_entries` — seed + soft_delete + age tombstone 40d (raw UPDATE, single statement — see sweeps tests) → `RecordingsRepo::purge_soft_deleted_with_ledger(&conn, &[rec.id])` → row gone AND `purged_recordings` has `(id, purged_at NOT NULL)`.

- [ ] **Step 2: Run** → FAIL.

- [ ] **Step 3: Implement** — add to `RecordingsRepo` (keep existing `purge_soft_deleted` intact for its other assertions; delegate when safe):

```rust
/// [`purge_soft_deleted`] + purge-ledger write in ONE transaction, so a
/// crash between row deletion and ledger recording cannot re-open the
/// resurrection window. Used by the server-side tombstone sweeper.
pub fn purge_soft_deleted_with_ledger(
    conn: &Connection,
    ids: &[Uuid],
) -> DbResult<Vec<Uuid>> {
    let purged_at = chrono::Utc::now().to_rfc3339();
    let tx = conn.unchecked_transaction()?;
    // Delete rows via the same FTS-safe statement sequence as
    // purge_soft_deleted (re-insert index rows, then DELETE).
    // — replicate the body of purge_soft_deleted against &tx here —
    // (copy the existing body; it is a straightforward statement list)
    for id in ids {
        tx.execute(
            "INSERT INTO purged_recordings (id, purged_at) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET purged_at = excluded.purged_at",
            rusqlite::params![id.to_string(), purged_at],
        )?;
    }
    tx.commit()?;
    Ok(ids.to_vec())
}
```
(When implementing: read `purge_soft_deleted`'s body and reuse its statements against `&tx`; return the purged ids the same way. If the body is small, have `purge_soft_deleted` call this with a no-ledger flag — implementer's choice, but the ledger write MUST be in the same transaction as the DELETE.)

In `src-tauri/src/sweeps.rs` phase 1: replace `RecordingsRepo::purge_soft_deleted(&conn, &ids)` with `RecordingsRepo::purge_soft_deleted_with_ledger(&conn, &ids)` and update the log to include `ledger_count = ids.len()`.

- [ ] **Step 4: Run** — `cargo test -p medical-db --test deletion_model` + `cargo test -p rust-medical-assistant --lib sweeps` PASS.

- [ ] **Step 5: Commit** — `git commit -am "feat(db): purge writes the resurrection-blocking ledger transactionally"`

---

### Task 6: `purged` field on the pull response + client application

**Files:**
- Modify: `crates/db/src/content_sync.rs` (PurgedRef + `purged_since`), `src-tauri/src/sharing_vocab_api/content_sync.rs` (handler + response struct), `src-tauri/src/content_remote.rs` (PullResponse), `src-tauri/src/commands/content_sync.rs` (run_sync application), tests

- [ ] **Step 1: Failing tests:**
  - db: `purged_since_filters_by_cutoff` — insert 2 ledger rows (T1, T2) → `ContentSyncRepo::purged_since(&conn, "T1.5")` → only T2 row.
  - db/client application: `apply_purged_refs_tombstones_live_copies` — seed live row, `ContentSyncRepo::apply_purged_refs(&conn, &[PurgedRef{id, purged_at}])` → row tombstoned (FTS absent); tombstoned input row → untouched; unknown id → no-op.

- [ ] **Step 2: Run** → FAIL.

- [ ] **Step 3: Implement.** In `crates/db/src/content_sync.rs` (wire-adjacent types live here):

```rust
/// A purge notification travelling on the pull response: the server
/// permanently deleted this recording; clients holding a stale live copy
/// tombstone it locally.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PurgedRef {
    pub id: String,
    pub purged_at: String,
}

impl ContentSyncRepo {
    /// Ledger entries with `purged_at > since` (all entries when `since`
    /// is None — a fresh client's first pull). Ordered by purged_at.
    pub fn purged_since(conn: &Connection, since: Option<&str>) -> DbResult<Vec<PurgedRef>> {
        let mut out = Vec::new();
        if let Some(since) = since {
            let mut stmt = conn.prepare(
                "SELECT id, purged_at FROM purged_recordings WHERE purged_at > ?1 ORDER BY purged_at",
            )?;
            let rows = stmt.query_map([since], |r| {
                Ok(PurgedRef { id: r.get(0)?, purged_at: r.get(1)? })
            })?;
            for row in rows { out.push(row?); }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, purged_at FROM purged_recordings ORDER BY purged_at",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(PurgedRef { id: r.get(0)?, purged_at: r.get(1)? })
            })?;
            for row in rows { out.push(row?); }
        }
        Ok(out)
    }

    /// Apply purge notifications: tombstone any LOCAL LIVE copy (FTS-safe)
    /// so a machine that missed the deletion converges. Already-tombstoned
    /// or unknown ids are no-ops (the local 30-day sweeper finishes them).
    pub fn apply_purged_refs(conn: &Connection, purged: &[PurgedRef]) -> DbResult<()> {
        for p in purged {
            let live: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM recordings WHERE id = ?1 AND deleted_at IS NULL)",
                [&p.id],
                |r| r.get(0),
            ).unwrap_or(false);
            if live {
                Self::sync_tombstone(conn, &p.id, &p.purged_at)?;
                tracing::info!(recording_id = %p.id, "sync: tombstoned local copy of purged recording");
            }
        }
        Ok(())
    }
}
```

Server (`sharing_vocab_api/content_sync.rs`): add to `ContentPullResponse`:

```rust
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    purged: Vec<medical_db::content_sync::PurgedRef>,
```
In `content_sync_pull_handler`'s spawn_blocking, also compute `let purged = ContentSyncRepo::purged_since(&conn, since.as_deref()).unwrap_or_default();` and return it; include in the response.

Client (`content_remote.rs`): add to `PullResponse`:

```rust
    #[serde(default)]
    pub purged: Vec<medical_db::content_sync::PurgedRef>,
```

`run_sync` (commands/content_sync.rs), inside the merge spawn_blocking (same conn, before/after merge — after is fine):

```rust
if !batch.purged.is_empty() {
    if let Err(e) = ContentSyncRepo::apply_purged_refs(&conn, &batch.purged) {
        tracing::warn!(error = %e, count = batch.purged.len(), "sync: applying purge refs failed");
    }
}
```
(Move `batch` fields appropriately — the merge closure currently moves `batch.recordings`; restructure to destructure `let batch_recordings = batch.recordings; let batch_purged = batch.purged;` before the closure.)

- [ ] **Step 4: Run** — tests PASS; `cargo test -p medical-db && cargo test -p rust-medical-assistant --lib content_sync`.

- [ ] **Step 5: Commit** — `git commit -am "feat(sync): purged ids travel on the pull response; clients tombstone stale copies"`

---

### Task 7: Chips — full-list sync + 365-day tombstone retention

**Files:**
- Modify: `src-tauri/src/sharing_vocab_api/condition_chips.rs` (GET + sync handlers: `list_active` → `list_all`; prune 30 → 365 days), `src-tauri/src/commands/conditions.rs` (list merge-back path — verify it merges the full list)

- [ ] **Step 1: Failing test** — server handler level is heavy; test at repo/command seam instead. In `crates/db/tests/deletion_model.rs`: `chips_merge_applies_remote_tombstone` — insert active chip locally; `ConditionChipsRepo::merge_incoming(&conn, &[tombstoned_version])` → local chip tombstoned (this already works — it pins the behavior the server change relies on). Then in `src-tauri` a unit test if the handler extracts a helper `full_chip_list(conn)`; otherwise verify by grep + compile. **Minimum bar:** repo-level tombstone-application test exists and passes.

- [ ] **Step 2/3:** In `condition_chips.rs` GET handler and sync handler: replace `list_active(&conn)` with `list_all(&conn)`; change `chrono::Duration::days(30)` → `days(365)` (both prune sites; update the adjacent comments). In `commands/conditions.rs` `list_condition_chips`: the remote path returns `Vec<ConditionChip>` — ensure the post-pull merge-back calls `merge_incoming` with that list (it does at ~line 276 on the sync path; if the list path doesn't merge, add the same merge-back so tombstones arrive on plain list — read lines 69-110 and mirror the sync path's merge call).

- [ ] **Step 4:** `cargo test -p medical-db --test deletion_model && cargo check -p rust-medical-assistant`.

- [ ] **Step 5: Commit** — `git commit -am "feat(sync): condition-chip tombstones travel; 365-day retention"`

---

### Task 8: Dictionary — `/sync-full` endpoint + client merge + legacy fallback

**Files:**
- Modify: `src-tauri/src/sharing_vocab_api/user_dictionary.rs` (new handler + route), `src-tauri/src/sharing_vocab_api/mod.rs` (route), `src-tauri/src/user_dict_remote.rs` (`sync_full`), `src-tauri/src/commands/user_dictionary.rs` (merge locally), tests

- [ ] **Step 1: Failing test** — `crates/db/tests/deletion_model.rs`: `dict_merge_applies_remote_tombstone` — active local word; merge tombstoned entry → local word tombstoned (`list` excludes it).

- [ ] **Step 2:** Run → should PASS already (repo merge handles tombstones) — this test PINS the behavior the client path will rely on. If it fails, the repo merge needs fixing first (escalate).

- [ ] **Step 3: Implement.** Server — new handler in `user_dictionary.rs` (mirror `dict_sync_handler` but full-fidelity):

```rust
/// POST /v1/user-dictionary/sync-full — bidirectional sync carrying
/// tombstones (`Vec<UserDictEntry>`), so deletions propagate to clients.
/// The legacy `/sync` (active words only) is kept for old clients.
pub(super) async fn dict_sync_full_handler(
    AxumState(state): AxumState<ApiState>,
    headers: HeaderMap,
    Json(incoming): Json<Vec<medical_core::types::user_dict_entry::UserDictEntry>>,
) -> Result<Json<Vec<medical_core::types::user_dict_entry::UserDictEntry>>, StatusCode> {
    let _ = authorize(&state, &headers)?;
    let db = Arc::clone(&state.db);
    let cutoff_iso = (chrono::Utc::now() - chrono::Duration::days(365))
        .format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let merged = tokio::task::spawn_blocking(
        move || -> Result<Vec<_>, medical_core::error::AppError> {
            let conn = db.conn()?;
            medical_db::user_dictionary::UserDictionaryRepo::merge_incoming(&conn, &incoming)
                .map_err(medical_core::error::AppError::from)?;
            let _ = medical_db::user_dictionary::UserDictionaryRepo::prune_tombstones(&conn, &cutoff_iso);
            medical_db::user_dictionary::UserDictionaryRepo::list_all(&conn)
                .map_err(medical_core::error::AppError::from)
        },
    ).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
      .map_err(|e| { warn!("dict_api sync-full failed: {e}"); StatusCode::INTERNAL_SERVER_ERROR })?;
    let _ = state.dict_changed_tx.send(());
    info!(incoming_count = incoming.len(), result_count = merged.len(), "dict_api: sync-full");
    Ok(Json(merged))
}
```
Add `list_all` to `UserDictionaryRepo` if missing (chips has one; dict has `list_all` already — verify; it exists per the merge code using it). Register `.route("/v1/user-dictionary/sync-full", post(dict_sync_full_handler))` in `mod.rs` next to the existing dict routes.

Client `user_dict_remote.rs`:

```rust
/// Full-fidelity sync (entries incl. tombstones). Falls back to the legacy
/// word-only `/sync` when the server predates the endpoint (404).
pub async fn sync_full(&self, local_entries: Vec<UserDictEntry>) -> AppResult<Vec<UserDictEntry>> {
    let base = self.base_url()
        .ok_or_else(|| AppError::Other("paired server has no dictionary address".into()))?;
    let url = format!("{base}/v1/user-dictionary/sync-full");
    let resp = self.client.post(&url)
        .timeout(std::time::Duration::from_secs(15))
        .bearer_auth(&self.bearer)
        .json(&local_entries)
        .send().await
        .map_err(|e| AppError::Other(format!("dict sync-full: {e}")))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        // Old server: fall back to legacy sync (active words only, no
        // tombstones — deletions then rely on the 30s poll until it upgrades).
        let words = self.sync(local_entries).await?;
        return Ok(words.into_iter().map(|w| UserDictEntry {
            id: crate::commands::user_dictionary::deterministic_dict_id(&w),
            word: w,
            updated_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            deleted_at: None,
        }).collect());
    }
    check_status(&resp).await?;
    resp.json::<Vec<UserDictEntry>>().await
        .map_err(|e| AppError::Other(format!("dict sync-full parse: {e}")))
}
```
(The fallback synthesizes entries from words so the client merge has a uniform shape; if `deterministic_dict_id` lives elsewhere/inaccessible, replicate the normalization the repo's `add` uses — read `UserDictEntry` + repo `add` for the exact id derivation and match it. If matching is nontrivial, escalate rather than guess — a mismatched synthetic id would corrupt the local merge.)

`commands/user_dictionary.rs`: in `user_dict_list` and `sync_user_dictionary_cmd`, replace the `remote.sync(...)`-style call with `remote.sync_full(local_all)` and merge the result locally (mirror the chips path):

```rust
let merged_entries = remote.sync_full(local_all.clone()).await?;
let db = std::sync::Arc::clone(&state.db);
let active = tokio::task::spawn_blocking(move || -> AppResult<Vec<String>> {
    let conn = db.conn()?;
    medical_db::user_dictionary::UserDictionaryRepo::merge_incoming(&conn, &merged_entries)
        .map_err(AppError::from)
}).await.map_err(crate::commands::join_err)??;
Ok(active)
```
Also update the legacy `/sync` handler prune 30→365 in `dict_sync_handler` for consistency.

- [ ] **Step 4:** `cargo test -p medical-db --test deletion_model && cargo check -p rust-medical-assistant && cargo test -p rust-medical-assistant --lib user_dict`.

- [ ] **Step 5: Commit** — `git commit -am "feat(sync): dictionary full-fidelity sync with tombstones + legacy fallback"`

---

### Task 9: `increment_use` no longer resurrects tombstones

**Files:**
- Modify: `crates/db/src/condition_chips.rs::increment_use` + its tests

- [ ] **Step 1: Failing test:**

```rust
#[test]
fn increment_use_does_not_resurrect_tombstone() {
    let db = crate::Database::open_in_memory().unwrap();
    let conn = db.conn().unwrap();
    let chip = ConditionChip { id: deterministic_id("htn"), text: "htn".into(),
        updated_at: "2026-06-01T00:00:00.000Z".into(), deleted_at: None,
        sort_order: 0, use_count: 5 };
    ConditionChipsRepo::upsert(&conn, &chip).unwrap();
    ConditionChipsRepo::soft_delete(&conn, &chip.id).unwrap(); // if exists; else set deleted_at via SQL
    let active = ConditionChipsRepo::increment_use(&conn, "htn", "2026-06-02T00:00:00.000Z").unwrap();
    assert!(!active.iter().any(|c| c.id == chip.id), "tombstoned chip must stay deleted");
    // And the create-arm self-heal is preserved for missing rows:
    let fresh = ConditionChipsRepo::increment_use(&conn, "brand-new", "2026-06-02T00:00:00.000Z").unwrap();
    assert!(fresh.iter().any(|c| c.text == "Brand New" || c.text == "brand new"), "missing chip still self-creates");
}
```
(Adapt to the actual `soft_delete`/text normalization in the repo; read the file first.)

- [ ] **Step 2:** Run → first assertion FAILS (current behavior resurrects).

- [ ] **Step 3:** Change the UPDATE to `... WHERE id = ?2 AND deleted_at IS NULL`, and in the `changed == 0` create-arm, first check `SELECT EXISTS(SELECT 1 FROM condition_chips WHERE id = ?1)` — if a (tombstoned) row exists, return the active list unchanged; only create when no row at all. Update the doc comment's "resurrected" paragraph to describe the new rule (explicit add resurrects; click does not).

- [ ] **Step 4:** `cargo test -p medical-db --lib condition_chips` — fix the doc-comment-derived test expectations if any assert resurrection.

- [ ] **Step 5: Commit** — `git commit -am "fix(db): condition-chip click no longer resurrects a tombstone"`

---

### Task 10: Equal-tie rider — `build_sparse_fields` uses max(revision, row) timestamps

**Files:**
- Modify: `src-tauri/src/commands/content_sync.rs::build_sparse_fields` + tests (the file's `mod tests`)

- [ ] **Step 1: Failing test:**

```rust
#[test]
fn build_sparse_fields_row_timestamp_wins_over_stale_revision() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let mut rec = medical_core::types::recording::Recording::new("v.wav", "/audio/v.wav".into());
    rec.soap_note = Some("regenerated soap".into());
    rec.updated_at = Some(chrono::Utc::now());
    medical_db::recordings::RecordingsRepo::insert(&conn, &rec).unwrap();
    // Stale revision from a pre-regeneration sync round-trip.
    medical_db::content_sync::ContentSyncRepo::upsert_revision(
        &conn, &rec.id, "soap_note", "2020-01-01T00:00:00Z", None).unwrap();

    let sync = build_sync_recording(&conn, &rec.id.to_string()).unwrap();
    let soap = &sync.fields["soap_note"];
    assert_ne!(soap.updated_at, "2020-01-01T00:00:00Z",
        "stale revision must not mask the newer row-level write");
    assert!(soap.updated_at.starts_with(&rec.updated_at.unwrap().to_rfc3339()[..10]));
}
```
(Adapt to actual `Recording` field types — `updated_at` may be `Option<DateTime<Utc>>`; `Recording::new` defaults it.)

- [ ] **Step 2:** Run → FAIL (revision wins today).

- [ ] **Step 3:** In `build_sparse_fields`, both `push_text` and `push_json` replace their timestamp selection with:

```rust
let rev_ts = rev_map.get(name).map(|r| r.updated_at.as_str());
let ts = match rev_ts {
    // Newer of the field revision and the row write — a stale revision
    // (from a pre-edit sync round-trip) must not mask a newer row-level
    // write, or the server's Equal-tie arm silently drops it.
    Some(r) => {
        let (rt, rod) = (r.to_string(), rev_map.get(name).and_then(|r| r.origin_device.clone()));
        if medical_db::content_sync::cmp_revision_vs_row(&rt, &row_ts) == std::cmp::Ordering::Less {
            (row_ts.clone(), None)
        } else {
            (rt, rod)
        }
    }
    None => (row_ts.clone(), None),
};
```
Expose the comparison from medical-db: make `cmp_lww_timestamps` `pub` (rename export as `pub fn cmp_lww_timestamps`) and call it directly instead of adding a wrapper — simplest: `if medical_db::content_sync::cmp_lww_timestamps(r, &row_ts) == std::cmp::Ordering::Less { (row_ts.clone(), None) } else { (r.to_string(), origin) }`. Implementer: write it cleanly in one place, used by both closures (extract a small `fn field_ts(name, rev_map, row_ts) -> (String, Option<String>)` above the closures).

- [ ] **Step 4:** Run → PASS; `cargo test -p rust-medical-assistant --lib commands::content_sync`.

- [ ] **Step 5: Commit** — `git commit -am "fix(sync): field timestamps are max(revision, row) — stale revisions can't mask newer writes"`

---

### Task 11: Docs + full gates

**Files:**
- Modify: `docs/superpowers/specs/2026-08-17-deletion-model-design.md` (amendment: dict via new endpoint), `AGENTS.md` (deferred-debt update)

- [ ] **Step 1:** Amend the spec §3 with: "Dictionary propagation uses a NEW `/v1/user-dictionary/sync-full` endpoint (`Vec<UserDictEntry>` incl. tombstones) because the existing endpoints speak `Vec<String>` and cannot carry tombstones without breaking old clients; new clients fall back to legacy `/sync` on 404."
- [ ] **Step 2:** AGENTS.md — remove the now-fixed items from the deferred-debt sync bullet (restore LWW, chips/dict propagation, purge resurrection, Equal-tie rider), keep the unfixed ones (cursor pinning, take(10)/upload queue, full revision coverage).
- [ ] **Step 3:** Full gates: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --lib && cargo test -p medical-db && cargo test -p rust-medical-assistant --lib && npm run check && npm run lint && npx vitest run`. All green.
- [ ] **Step 4:** Commit — `git commit -am "docs: deletion-model shipped; update spec amendment and debt tracking"`
