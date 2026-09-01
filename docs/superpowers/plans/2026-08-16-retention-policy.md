# Recordings Retention Policy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Opt-in per-machine setting that auto-trashes recordings older than N days (Never/30/90/180/365), riding the existing soft-delete → sync → 30-day server purge pipeline, with a restore-once exemption.

**Architecture:** A `RecordingsRepo` retention sweep selects non-deleted, past-cutoff, non-exempt recordings and applies the existing `soft_delete`; `restore` stamps `metadata.retention_exempt = true`. The state.rs daily loop becomes unconditional and gains a retention phase. UI: Settings → Data Management dropdown. Spec: `docs/superpowers/specs/2026-08-16-retention-policy-design.md`.

**Execution:** worktree `.worktrees/retention`, branch `feat/retention-policy`.

---

### Task 1: Config field + DB retention sweep + restore exemption

**Files:**
- Modify: `crates/core/src/types/settings.rs` (AppConfig)
- Modify: `crates/db/src/recordings.rs` (restore stamp + new sweep fn)
- Test: `crates/db/tests/retention.rs` (new integration test file)

- [ ] **Step 1 (red):** Add `retention_days: Option<u32>` to `AppConfig` (near the Features block, with `#[serde(default)]` and a doc comment: "Per-machine recordings retention policy — soft-delete recordings older than this many days. None = keep forever (default)."). In `crates/core` settings tests, add a roundtrip test: a JSON config WITHOUT the field deserializes to `retention_days: None`; with `42` roundtrips. Run `cargo test -p medical-core --lib settings` — the without-field test fails (missing field) until the `#[serde(default)]` attr is right.
- [ ] **Step 2 (red):** Create `crates/db/tests/retention.rs` following the conventions of `crates/db/tests/recording_sync_merge.rs` (read its header first — in-memory `Database::open_in_memory`, insert recordings via `RecordingsRepo::insert`, etc.). Tests:

```rust
//! Retention sweep integration tests — candidate selection, exemption,
//! idempotency. (No PHI: fictional fixture data.)

use medical_core::types::recording::{ProcessingStatus, Recording};
use medical_db::recordings::RecordingsRepo;
use medical_db::Database;
use std::path::PathBuf;

fn recording_at(id: uuid::Uuid, created_at: chrono::DateTime<chrono::Utc>) -> Recording {
    let mut rec = Recording::new(format!("{id}.wav"), PathBuf::from(format!("/tmp/{id}.wav")));
    rec.id = id;
    rec.status = ProcessingStatus::Pending;
    rec.created_at = created_at;
    rec
}

fn days_ago(n: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::TimeDelta::days(n)
}

#[test]
fn retention_soft_deletes_only_old_visible_recordings() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let old = uuid::Uuid::new_v4();
    let fresh = uuid::Uuid::new_v4();
    RecordingsRepo::insert(&conn, &recording_at(old, days_ago(100))).expect("insert old");
    RecordingsRepo::insert(&conn, &recording_at(fresh, days_ago(10))).expect("insert fresh");

    let purged = RecordingsRepo::retention_soft_delete_older_than(&conn, 90, chrono::Utc::now())
        .expect("sweep");
    assert_eq!(purged, vec![old]);
    // The old one is hidden (soft-deleted), the fresh one remains.
    assert!(RecordingsRepo::get_by_id(&conn, &old).is_err());
    assert!(RecordingsRepo::get_by_id(&conn, &fresh).is_ok());
}

#[test]
fn retention_respects_restore_exemption() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = uuid::Uuid::new_v4();
    RecordingsRepo::insert(&conn, &recording_at(id, days_ago(400))).expect("insert");

    let first = RecordingsRepo::retention_soft_delete_older_than(&conn, 90, chrono::Utc::now())
        .expect("first sweep");
    assert_eq!(first, vec![id]);

    RecordingsRepo::restore(&conn, &id).expect("restore");
    // Restore stamps retention_exempt — the next sweep must skip it.
    let second = RecordingsRepo::retention_soft_delete_older_than(&conn, 90, chrono::Utc::now())
        .expect("second sweep");
    assert!(second.is_empty());
    assert!(RecordingsRepo::get_by_id(&conn, &id).is_ok());
}

#[test]
fn retention_is_idempotent_for_already_deleted() {
    let db = Database::open_in_memory().expect("db");
    let conn = db.conn().expect("conn");
    let id = uuid::Uuid::new_v4();
    RecordingsRepo::insert(&conn, &recording_at(id, days_ago(100))).expect("insert");
    RecordingsRepo::soft_delete(&conn, &id).expect("manual delete");

    let swept = RecordingsRepo::retention_soft_delete_older_than(&conn, 90, chrono::Utc::now())
        .expect("sweep");
    assert!(swept.is_empty()); // already in trash — never touched
}
```

(Adapt fixture construction to what `recording_sync_merge.rs` actually does — read it first; `Recording::new` + field assignment pattern comes from `src-tauri/.../test_helpers.rs`. `get_by_id` on a soft-deleted row errors only if that's the repo's actual behavior — VERIFY by reading `get_by_id`; if it returns the row regardless, assert on a `list_all`/`deleted_at` check instead. Also verify `insert` signature — adjust tests to compile against the real API.)

Run `cargo test -p medical-db --test retention` → COMPILE ERROR (no such repo fn) — the red phase.

- [ ] **Step 3 (green):** In `crates/db/src/recordings.rs`:

(a) `restore` — stamp the exemption. Read the row's metadata first, stamp, then run the existing UPDATE (keep the FTS re-insert touch as-is):

```rust
    pub fn restore(conn: &Connection, id: &Uuid) -> DbResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        // Stamp the retention exemption BEFORE clearing deleted_at: restoring
        // is an explicit user override — the retention sweep must never
        // re-trash this recording.
        let metadata: String = conn.query_row(
            "SELECT metadata FROM recordings WHERE id = ?1",
            [&id.to_string()],
            |row| row.get(0),
        )?;
        let mut meta: serde_json::Value = serde_json::from_str(&metadata).unwrap_or_default();
        if !meta.is_object() {
            meta = serde_json::json!({});
        }
        if let Some(obj) = meta.as_object_mut() {
            obj.insert("retention_exempt".to_string(), serde_json::json!(true));
        }
        conn.execute(
            "UPDATE recordings SET deleted_at = NULL, updated_at = ?1, metadata = ?2 WHERE id = ?3 AND deleted_at IS NOT NULL",
            rusqlite::params![now, meta.to_string(), id.to_string()],
        )?;
        // ... existing FTS re-insert touch unchanged
```

(Adapt to the real column types — check how `update` writes metadata; keep behavior for missing/NULL metadata robust.)

(b) The sweep fn (place near `soft_delete`):

```rust
    /// Retention sweep: soft-delete every visible recording older than the
    /// cutoff whose metadata does not carry `retention_exempt`. Returns the
    /// ids that were trashed (for count-only logging). Idempotent — rows
    /// already in trash are never touched.
    pub fn retention_soft_delete_older_than(
        conn: &Connection,
        days: u32,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DbResult<Vec<Uuid>> {
        let cutoff = (now - chrono::TimeDelta::days(days as i64)).to_rfc3339();
        let mut stmt = conn.prepare(
            "SELECT id, metadata FROM recordings
              WHERE deleted_at IS NULL AND created_at < ?1",
        )?;
        let rows: Vec<(String, String)> = stmt
            .query_map(rusqlite::params![cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<_, _>>()?;

        let mut trashed = Vec::new();
        for (id_str, metadata) in rows {
            let Ok(id) = uuid::Uuid::parse_str(&id_str) else { continue };
            let exempt = serde_json::from_str::<serde_json::Value>(&metadata)
                .ok()
                .and_then(|m| m.get("retention_exempt").and_then(|v| v.as_bool()))
                .unwrap_or(false);
            if exempt {
                continue;
            }
            Self::soft_delete(conn, &id)?;
            trashed.push(id);
        }
        Ok(trashed)
    }
```

(Verify the `created_at` comparison semantics against how the tombstone sweeper queries dates — `datetime(created_at) < datetime(?)` style; match the existing convention so cutoff math is consistent.)

- [ ] **Step 4:** `cargo test -p medical-db --test retention` PASS; `cargo test -p medical-core --lib settings` PASS; `cargo test -p medical-db` (all db integration tests still green); clippy both crates; fmt.
- [ ] **Step 5: Commit** `feat(db): retention sweep + restore exemption + retention_days setting`

---

### Task 2: Sweeper loop restructure

**Files:**
- Modify: `src-tauri/src/state.rs` (tombstone sweeper block, ~lines 722-800)

- [ ] **Step 1:** Restructure the block: spawn the daily loop UNCONDITIONALLY (remove the `if server_config.is_some()` gate around the spawn — keep `load_server_config()` semantics inside the loop if that's how the server check is done per tick, or re-check each tick via the DB if the current code captures it once — READ the full block first and preserve the server-purge behavior EXACTLY: same SQL, same RAG/audio cleanup, same logging). Add a retention phase after the purge phase:

```rust
                        // ── Retention sweep (any machine with a policy) ──
                        // Soft-delete recordings older than the configured
                        // window. Runs wherever the user enabled the policy;
                        // soft-deletes then sync + are purged server-side 30
                        // days later by the tombstone phase above.
                        if let Ok(conn) = db_clone.conn() {
                            match medical_db::settings::SettingsRepo::load_config(&conn) {
                                Ok(config) => {
                                    if let Some(days) = config.retention_days {
                                        match RecordingsRepo::retention_soft_delete_older_than(
                                            &conn, days, chrono::Utc::now(),
                                        ) {
                                            Ok(trashed) if !trashed.is_empty() => {
                                                tracing::info!(
                                                    count = trashed.len(),
                                                    "retention sweep: moved recordings to trash"
                                                );
                                            }
                                            Ok(_) => {}
                                            Err(e) => tracing::warn!(
                                                error = %e,
                                                "retention sweep failed"
                                            ),
                                        }
                                    }
                                }
                                Err(e) => tracing::warn!(error = %e, "retention sweep: config load failed"),
                            }
                        }
```

(Adapt to the actual loop body structure — it may be inside `spawn_blocking` with `db_clone` moved in; the existing tombstone code shows the exact pattern for conn access + purge. Match it. Import `RecordingsRepo` if not in scope. `retention_days` clamps: if `days == 0` treat as off — add `if let Some(days) if days > 0`.)

- [ ] **Step 2:** Verify the existing sweeper test coverage: `grep -n "tombstone\|sweeper" src-tauri/src/state.rs` test mod (if any) — run whatever exists; the retention phase itself is covered by Task 1's db tests, the loop wiring is compile-checked. Run `cargo test -p rust-medical-assistant --lib state` if such tests exist, else `--lib --no-run`; clippy; fmt.
- [ ] **Step 3: Commit** `feat(tauri): daily retention sweep in the sweeper loop`

---

### Task 3: DataManagement UI

**Files:**
- Modify: `src/lib/types/index.ts` (AppConfig gains `retention_days: number | null`)
- Modify: `src/lib/components/settings/sections/DataManagement.svelte`
- Test: colocated per existing conventions (check whether DataManagement has a test; if not, add `DataManagement.test.ts` following the settings-section test pattern found in the repo — `ls src/lib/components/settings/**/*.test.* 2>/dev/null || grep -rln "settings/sections" src --include="*.test.ts" | head -2`)

- [ ] **Step 1:** TS: add `retention_days: number | null;` to `AppConfig` (mirror note comment if the interface has one about fields mirroring Rust).
- [ ] **Step 2:** In DataManagement.svelte (follow the file's existing form-group/label conventions and the `<select>` pattern from `GeneralBasics.svelte:63`), add a "Recording retention" group:

```svelte
<div class="form-group">
  <h3>Recording retention</h3>
  <label class="form-label" for="retention-select">Automatically move recordings to trash when older than</label>
  <select
    id="retention-select"
    value={settings.state.retention_days ?? 0}
    onchange={(e) => {
      const days = Number((e.currentTarget as HTMLSelectElement).value);
      settings.updateField('retention_days', days > 0 ? days : null);
    }}
  >
    <option value={0}>Never (keep forever)</option>
    <option value={30}>30 days</option>
    <option value={90}>90 days</option>
    <option value={180}>180 days</option>
    <option value={365}>365 days</option>
  </select>
  <span class="form-hint">
    Trashed recordings keep a 30-day undo window before permanent deletion.
    Restoring a recording exempts it from future automatic cleanup.
  </span>
</div>
```

(Adapt class names to the file's real ones — read it first; check how `settings.updateField` types the value — `retention_days: number | null` must be assignable. If updateField rejects null for some reason, follow the store's nullable-field precedent.)
- [ ] **Step 3:** Test (jsdom if mounting; if the section is too heavy to mount like RecordTab, mirror the mapping logic in a pure test): renders Never by default; selecting 90 calls updateField with 90; selecting Never calls updateField with null.
- [ ] **Step 4:** `npx vitest run` + `npm run check` green.
- [ ] **Step 5: Commit** `feat(web): retention policy setting in Data Management`

---

### Task 4: Gates + final review

- [ ] `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --lib` (wiremock-flake fallback allowed); `cargo test -p medical-db`; `npx vitest run`; `npm run check`; `npm run lint`.
- [ ] Final whole-branch reviewer subagent.
