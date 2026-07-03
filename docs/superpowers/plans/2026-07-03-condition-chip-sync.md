# Condition Chip Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in setting that synchronizes "known condition" chip presets between the office server and remote clients via two-way merge with per-item last-write-wins and tombstones.

**Architecture:** New `condition_chips` table with deterministic UUID-v5 IDs, a `ConditionChipsRepo` with an idempotent `merge_incoming` function, server HTTP endpoints on the existing `vocab_port`, a `ConditionsRemote` HTTP client, and Tauri command dispatchers that route to the server when sync is enabled + paired. The opt-in `sync_condition_chips` setting (defaults false) gates all sync behavior.

**Tech Stack:** Rust (rusqlite, axum, reqwest, uuid, chrono), Svelte 5 runes, Tauri v2 commands, Vitest.

**Spec:** `docs/superpowers/specs/2026-07-03-condition-chip-sync-design.md`

---

## File Structure

### New files

| File | Responsibility |
|------|----------------|
| `crates/core/src/types/condition_chip.rs` | `ConditionChip` struct + `deterministic_id()` helper |
| `crates/db/src/condition_chips.rs` | `ConditionChipsRepo` — CRUD + `merge_incoming` + `prune_tombstones` |
| `crates/db/src/migrations/m010_condition_chips.rs` | Migration: create table + seed from `custom_conditions` |
| `src-tauri/src/conditions_remote.rs` | HTTP client mirroring `templates_remote.rs` |
| `src-tauri/src/commands/conditions.rs` | Tauri commands: `list_condition_chips`, `add_condition_chip`, `remove_condition_chip`, `sync_condition_chips` |
| `src/lib/api/conditions.ts` | Frontend typed wrappers for the 4 Tauri commands |
| `src/lib/components/ConditionChips.test.ts` | Frontend test for the rewired chip component |

### Modified files

| File | Change |
|------|--------|
| `crates/core/src/types/mod.rs` | Register `pub mod condition_chip;` + re-export |
| `crates/db/src/lib.rs` | Register `pub mod condition_chips;` + re-export |
| `crates/db/src/migrations/mod.rs` | Register `pub mod m010_condition_chips;` + add to `all_migrations()` |
| `crates/core/src/types/settings.rs` | Add `sync_condition_chips: bool` field + backward-compat test |
| `src-tauri/src/sharing_vocab_api.rs` | Add `/v1/condition-chips` GET + `/v1/condition-chips/sync` POST routes + handlers |
| `src-tauri/src/commands/mod.rs` | Register `pub mod conditions;` |
| `src-tauri/src/lib.rs` | Register `mod conditions_remote;` + add conditions commands to `generate_handler!` |
| `src/lib/types/index.ts` | Add `sync_condition_chips: boolean` to `AppConfig` interface |
| `src/lib/stores/settings.svelte.ts` | Add `sync_condition_chips: false` to defaults |
| `src/lib/components/ConditionChips.svelte` | Rewire from `settings.state.custom_conditions` to `invoke` calls |
| `src/lib/components/settings/Sharing.svelte` | Add opt-in toggle for condition chip sync |

---

## Task 1: `ConditionChip` struct + `deterministic_id`

**Files:**
- Create: `crates/core/src/types/condition_chip.rs`
- Modify: `crates/core/src/types/mod.rs` (lines 21-43)

- [ ] **Step 1: Create the type module**

Create `crates/core/src/types/condition_chip.rs`:

```rust
//! Condition chip type used by the condition-chip sync feature.
//!
//! A condition chip is a practice-wide quick-add preset shown under "Known
//! conditions" (e.g. "Hypertension"). Each chip has a deterministic ID derived
//! from its normalized text so that two machines independently adding the same
//! condition produce the same row — enabling per-item last-write-wins merge.

use serde::{Deserialize, Serialize};

/// Fixed namespace for UUID v5 generation of condition chip IDs.
/// Generated once and hardcoded — must never change (would break ID stability).
const CONDITION_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x4a, 0x3e, 0xc1, 0x07, 0x9b, 0x2d, 0x4f, 0x6a,
    0xa1, 0x10, 0xd8, 0x4f, 0xa2, 0xb3, 0xc5, 0xe7,
]);

/// A condition chip entry with sync metadata.
///
/// - `id`: deterministic UUID v5 from `normalize_for_id(&text)`. Two machines
///   adding "Hypertension" produce the same id.
/// - `updated_at`: ISO 8601 UTC string — the last-write-wins clock.
/// - `deleted_at`: tombstone timestamp. `None` means active.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConditionChip {
    pub id: String,
    pub text: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// Normalize condition text for deterministic ID generation.
///
/// Lowercases and trims so "Hypertension", "hypertension ", and
/// "HYPERTENSION" all produce the same id.
pub fn normalize_for_id(text: &str) -> String {
    text.trim().to_lowercase()
}

/// Generate a deterministic UUID v5 from normalized condition text.
///
/// Same text always produces the same UUID, across machines and restarts.
pub fn deterministic_id(text: &str) -> String {
    uuid::Uuid::new_v5(&CONDITION_NAMESPACE, normalize_for_id(text).as_bytes())
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_id_is_stable() {
        assert_eq!(deterministic_id("Hypertension"), deterministic_id("Hypertension"));
    }

    #[test]
    fn deterministic_id_is_case_insensitive() {
        assert_eq!(deterministic_id("Hypertension"), deterministic_id("hypertension"));
    }

    #[test]
    fn deterministic_id_ignores_whitespace() {
        assert_eq!(deterministic_id("Hypertension"), deterministic_id(" Hypertension "));
    }

    #[test]
    fn different_conditions_have_different_ids() {
        assert_ne!(deterministic_id("Hypertension"), deterministic_id("Diabetes"));
    }
}
```

- [ ] **Step 2: Check that `uuid` is available in `medical-core`**

Run: `grep 'uuid' crates/core/Cargo.toml`
Expected: a line like `uuid = { version = "1", features = ["v5", "serde"] }` or similar.

If `uuid` is NOT in `Cargo.toml`, add it. Check existing usage first:
Run: `grep -r 'use uuid' crates/core/src/ | head -3`
If uuid is already used in the crate, it's already a dependency.

- [ ] **Step 3: Register the module**

In `crates/core/src/types/mod.rs`, add after line 23 (`pub mod endpoint;`):

```rust
pub mod condition_chip;
```

And in the re-export block (after line 36, `pub use letter_audience::LetterAudience;`), add:

```rust
pub use condition_chip::*;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p medical-core --lib condition_chip`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/types/condition_chip.rs crates/core/src/types/mod.rs
git commit -m "feat(core): add ConditionChip type with deterministic UUID v5 id"
```

---

## Task 2: `ConditionChipsRepo` with merge logic

**Files:**
- Create: `crates/db/src/condition_chips.rs`
- Modify: `crates/db/src/lib.rs` (lines 26-42)

- [ ] **Step 1: Write the failing tests first (TDD)**

Create `crates/db/src/condition_chips.rs` with tests at the bottom. Start with the test module to define behavior:

```rust
//! Repository for the `condition_chips` table — condition chip CRUD and
//! the last-write-wins merge used by condition chip sync.

use rusqlite::{Connection, Row};

use medical_core::types::condition_chip::{
    ConditionChip, deterministic_id, normalize_for_id,
};

use crate::{DbError, DbResult};

/// Repository for the `condition_chips` table.
///
/// All methods are associated functions that take a `&Connection`.
pub struct ConditionChipsRepo;

impl ConditionChipsRepo {
    /// Map a SQL row to a `ConditionChip`.
    fn row_to_chip(row: &Row) -> rusqlite::Result<ConditionChip> {
        Ok(ConditionChip {
            id: row.get(0)?,
            text: row.get(1)?,
            updated_at: row.get(2)?,
            deleted_at: row.get(3)?,
        })
    }

    /// Return all active (non-deleted) chips, ordered alphabetically by text.
    pub fn list_active(conn: &Connection) -> DbResult<Vec<ConditionChip>> {
        let mut stmt = conn.prepare(
            "SELECT id, text, updated_at, deleted_at
             FROM condition_chips
             WHERE deleted_at IS NULL
             ORDER BY text COLLATE NOCASE",
        )?;
        let chips = stmt
            .query_map([], Self::row_to_chip)?
            .filter_map(|r| {
                r.map_err(|e| tracing::warn!(error = %e, "dropping unreadable chip row"))
                    .ok()
            })
            .collect();
        Ok(chips)
    }

    /// Return ALL chips including tombstones (for sync).
    pub fn list_all(conn: &Connection) -> DbResult<Vec<ConditionChip>> {
        let mut stmt = conn.prepare(
            "SELECT id, text, updated_at, deleted_at
             FROM condition_chips",
        )?;
        let chips = stmt
            .query_map([], Self::row_to_chip)?
            .filter_map(|r| {
                r.map_err(|e| tracing::warn!(error = %e, "dropping unreadable chip row"))
                    .ok()
            })
            .collect();
        Ok(chips)
    }

    /// Insert or replace a chip by id.
    pub fn upsert(conn: &Connection, chip: &ConditionChip) -> DbResult<()> {
        conn.execute(
            "INSERT INTO condition_chips (id, text, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                text = excluded.text,
                updated_at = excluded.updated_at,
                deleted_at = excluded.deleted_at",
            rusqlite::params![chip.id, chip.text, chip.updated_at, chip.deleted_at],
        )?;
        Ok(())
    }

    /// Soft-delete a chip by id: set deleted_at and bump updated_at.
    pub fn soft_delete(conn: &Connection, id: &str, now_iso: &str) -> DbResult<()> {
        conn.execute(
            "UPDATE condition_chips
             SET deleted_at = ?1, updated_at = ?1
             WHERE id = ?2",
            rusqlite::params![now_iso, id],
        )?;
        Ok(())
    }

    /// Core merge: reconcile incoming remote chips with local state using
    /// per-item last-write-wins.
    ///
    /// For each remote chip:
    /// - If no local chip with the same id: insert it as-is.
    /// - If local exists: compare `updated_at`. Newer wins. On exact tie,
    ///   deleted wins (conservative — avoids ghost reappearance).
    ///
    /// Local-only chips (not in remote list) are left untouched — they'll
    /// propagate to the other side on its next pull.
    ///
    /// Returns the active (non-deleted) chips after merge.
    pub fn merge_incoming(
        conn: &Connection,
        remote_chips: &[ConditionChip],
    ) -> DbResult<Vec<ConditionChip>> {
        for remote in remote_chips {
            let local: Option<ConditionChip> = conn
                .query_row(
                    "SELECT id, text, updated_at, deleted_at
                     FROM condition_chips WHERE id = ?1",
                    [&remote.id],
                    Self::row_to_chip,
                )
                .map(Some)
                .unwrap_or(None);

            match local {
                None => {
                    // New chip from remote — insert as-is.
                    Self::upsert(conn, remote)?;
                }
                Some(local) => {
                    let remote_wins = remote.updated_at.cmp(&local.updated_at)
                        .is_gt();
                    let tie_and_remote_deleted = remote.updated_at == local.updated_at
                        && remote.deleted_at.is_some();
                    if remote_wins || tie_and_remote_deleted {
                        Self::upsert(conn, remote)?;
                    }
                    // else: local wins, do nothing.
                }
            }
        }
        Self::list_active(conn)
    }

    /// Remove tombstones older than the given duration. Active chips are
    /// never removed. If a pruned tombstone's condition is re-added later,
    /// it creates a fresh active row.
    pub fn prune_tombstones(conn: &Connection, cutoff_iso: &str) -> DbResult<usize> {
        let affected = conn.execute(
            "DELETE FROM condition_chips
             WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
            [&cutoff_iso],
        )?;
        Ok(affected)
    }

    /// Add a condition chip (used by the local add command).
    /// Returns the full active list after insertion.
    pub fn add(conn: &Connection, text: &str, now_iso: &str) -> DbResult<Vec<ConditionChip>> {
        let chip = ConditionChip {
            id: deterministic_id(text),
            text: text.trim().to_string(),
            updated_at: now_iso.to_string(),
            deleted_at: None,
        };
        Self::upsert(conn, &chip)?;
        Self::list_active(conn)
    }

    /// Remove a condition chip by its text (used by the local remove command).
    /// Returns the full active list after removal.
    pub fn remove_by_text(conn: &Connection, text: &str, now_iso: &str) -> DbResult<Vec<ConditionChip>> {
        let id = deterministic_id(text);
        // Only soft-delete if the chip exists and is active.
        conn.execute(
            "UPDATE condition_chips
             SET deleted_at = ?1, updated_at = ?1
             WHERE id = ?2 AND deleted_at IS NULL",
            rusqlite::params![now_iso, id],
        )?;
        Self::list_active(conn)
    }
}
```

Now add the tests at the bottom of the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    fn now(offset_secs: i64) -> String {
        // Build ISO 8601 timestamps relative to a base, for deterministic LWW tests.
        let base = chrono::DateTime::parse_from_rfc3339("2026-07-03T10:00:00Z").unwrap();
        let t = base + chrono::Duration::seconds(offset_secs);
        t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    fn chip(text: &str, updated_offset: i64, deleted: bool) -> ConditionChip {
        ConditionChip {
            id: deterministic_id(text),
            text: text.to_string(),
            updated_at: now(updated_offset),
            deleted_at: if deleted { Some(now(updated_offset)) } else { None },
        }
    }

    #[test]
    fn merge_inserts_new_remote_chip() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        // Table doesn't exist yet — this test will fail until Task 3 migration runs.
        // For now we create it manually so the repo logic can be tested independently.
        conn.execute_batch(
            "CREATE TABLE condition_chips (
                id TEXT PRIMARY KEY, text TEXT NOT NULL,
                updated_at TEXT NOT NULL, deleted_at TEXT
            );"
        ).unwrap();

        let result = ConditionChipsRepo::merge_incoming(&conn, &[chip("Asthma", 0, false)]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "Asthma");
    }

    #[test]
    fn merge_remote_newer_wins() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "CREATE TABLE condition_chips (
                id TEXT PRIMARY KEY, text TEXT NOT NULL,
                updated_at TEXT NOT NULL, deleted_at TEXT
            );"
        ).unwrap();

        // Local has "Diabetes" at t=0.
        ConditionChipsRepo::upsert(&conn, &chip("Diabetes", 0, false)).unwrap();
        // Remote has "Diabetes" at t=300 (newer).
        let result = ConditionChipsRepo::merge_incoming(&conn, &[chip("Diabetes", 300, false)]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].updated_at, now(300));
    }

    #[test]
    fn merge_local_newer_wins() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "CREATE TABLE condition_chips (
                id TEXT PRIMARY KEY, text TEXT NOT NULL,
                updated_at TEXT NOT NULL, deleted_at TEXT
            );"
        ).unwrap();

        // Local has "Diabetes" at t=300.
        ConditionChipsRepo::upsert(&conn, &chip("Diabetes", 300, false)).unwrap();
        // Remote has "Diabetes" at t=0 (older).
        let result = ConditionChipsRepo::merge_incoming(&conn, &[chip("Diabetes", 0, false)]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].updated_at, now(300));
    }

    #[test]
    fn merge_tombstone_wins_over_older_active() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "CREATE TABLE condition_chips (
                id TEXT PRIMARY KEY, text TEXT NOT NULL,
                updated_at TEXT NOT NULL, deleted_at TEXT
            );"
        ).unwrap();

        // Local has "COPD" active at t=0.
        ConditionChipsRepo::upsert(&conn, &chip("COPD", 0, false)).unwrap();
        // Remote has "COPD" tombstone at t=600 (newer).
        let result = ConditionChipsRepo::merge_incoming(&conn, &[chip("COPD", 600, true)]).unwrap();
        assert!(result.is_empty(), "COPD should be tombstoned");
    }

    #[test]
    fn merge_re_add_after_tombstone() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "CREATE TABLE condition_chips (
                id TEXT PRIMARY KEY, text TEXT NOT NULL,
                updated_at TEXT NOT NULL, deleted_at TEXT
            );"
        ).unwrap();

        // Local has "COPD" tombstone at t=600.
        ConditionChipsRepo::upsert(&conn, &chip("COPD", 600, true)).unwrap();
        // Remote has "COPD" active at t=1200 (newer — re-added).
        let result = ConditionChipsRepo::merge_incoming(&conn, &[chip("COPD", 1200, false)]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "COPD");
        assert!(result[0].deleted_at.is_none());
    }

    #[test]
    fn merge_tie_deleted_wins() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "CREATE TABLE condition_chips (
                id TEXT PRIMARY KEY, text TEXT NOT NULL,
                updated_at TEXT NOT NULL, deleted_at TEXT
            );"
        ).unwrap();

        // Local active, remote tombstone, same timestamp.
        let local = chip("Asthma", 100, false);
        let remote_tombstone = chip("Asthma", 100, true);
        ConditionChipsRepo::upsert(&conn, &local).unwrap();
        let result = ConditionChipsRepo::merge_incoming(&conn, &[remote_tombstone]).unwrap();
        assert!(result.is_empty(), "on tie, deleted wins");
    }

    #[test]
    fn merge_is_idempotent() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "CREATE TABLE condition_chips (
                id TEXT PRIMARY KEY, text TEXT NOT NULL,
                updated_at TEXT NOT NULL, deleted_at TEXT
            );"
        ).unwrap();

        let remote = vec![chip("Asthma", 0, false), chip("Diabetes", 0, false)];
        let r1 = ConditionChipsRepo::merge_incoming(&conn, &remote).unwrap();
        let r2 = ConditionChipsRepo::merge_incoming(&conn, &remote).unwrap();
        assert_eq!(r1, r2);
    }

    #[test]
    fn prune_tombstones_removes_old_only() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "CREATE TABLE condition_chips (
                id TEXT PRIMARY KEY, text TEXT NOT NULL,
                updated_at TEXT NOT NULL, deleted_at TEXT
            );"
        ).unwrap();

        // Old tombstone (t=-3000000, way in the past).
        ConditionChipsRepo::upsert(&conn, &chip("OldGone", -3000000, true)).unwrap();
        // Recent tombstone (t=0).
        ConditionChipsRepo::upsert(&conn, &chip("RecentGone", 0, true)).unwrap();
        // Active chip.
        ConditionChipsRepo::upsert(&conn, &chip("Active", 0, false)).unwrap();

        // Prune tombstones older than t=-100.
        let pruned = ConditionChipsRepo::prune_tombstones(&conn, &now(-100)).unwrap();
        assert_eq!(pruned, 1, "only the old tombstone should be pruned");

        // Recent tombstone still exists, active chip untouched.
        let all = ConditionChipsRepo::list_all(&conn).unwrap();
        assert_eq!(all.len(), 2); // RecentGone + Active
    }

    #[test]
    fn add_and_remove_by_text() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();
        conn.execute_batch(
            "CREATE TABLE condition_chips (
                id TEXT PRIMARY KEY, text TEXT NOT NULL,
                updated_at TEXT NOT NULL, deleted_at TEXT
            );"
        ).unwrap();

        let after_add = ConditionChipsRepo::add(&conn, "Hypertension", &now(0)).unwrap();
        assert_eq!(after_add.len(), 1);

        let after_remove = ConditionChipsRepo::remove_by_text(&conn, "Hypertension", &now(100)).unwrap();
        assert!(after_remove.is_empty(), "should be removed");

        // Verify it's a tombstone, not hard-deleted:
        let all = ConditionChipsRepo::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].deleted_at.is_some());
    }
}
```

- [ ] **Step 2: Register the module in `crates/db/src/lib.rs`**

Add after line 28 (`pub mod letter_audiences;`):

```rust
pub mod condition_chips;
```

And after line 42 (`pub use user_dictionary::UserDictionaryRepo;`):

```rust
pub use condition_chips::ConditionChipsRepo;
```

- [ ] **Step 3: Run tests to verify they fail (table doesn't exist yet — but we create it manually in tests)**

Run: `cargo test -p medical-db --lib condition_chips`
Expected: All tests pass (because each test creates the table manually). The repo logic is fully tested independently of the migration.

- [ ] **Step 4: Commit**

```bash
git add crates/db/src/condition_chips.rs crates/db/src/lib.rs
git commit -m "feat(db): add ConditionChipsRepo with LWW merge + tombstones"
```

---

## Task 3: Migration `m010_condition_chips`

**Files:**
- Create: `crates/db/src/migrations/m010_condition_chips.rs`
- Modify: `crates/db/src/migrations/mod.rs` (lines 15, 49-84)

- [ ] **Step 1: Create the migration file**

Create `crates/db/src/migrations/m010_condition_chips.rs`:

```rust
use rusqlite::Connection;

use crate::DbResult;

/// Create the `condition_chips` table and seed it from existing
/// `AppConfig.custom_conditions` values (if any).
///
/// Each existing condition becomes an active row with `updated_at = now()`.
/// The old `custom_conditions` field in the settings blob is left intact
/// (inert) for rollback safety.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS condition_chips (
            id          TEXT PRIMARY KEY,
            text        TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            deleted_at  TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_condition_chips_active
            ON condition_chips(text) WHERE deleted_at IS NULL;",
    )?;

    // Seed from existing custom_conditions in the settings blob.
    seed_from_custom_conditions(conn)?;

    Ok(())
}

/// Read `custom_conditions` from the `settings` table (key "app_config")
/// and insert each as an active condition chip.
fn seed_from_custom_conditions(conn: &Connection) -> DbResult<()> {
    use medical_core::types::condition_chip::{ConditionChip, deterministic_id};
    use medical_core::types::settings::AppConfig;

    let json: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'app_config'",
            [],
            |row| row.get(0),
        )
        .ok();

    let Some(json) = json else { return Ok(()); };
    let config: AppConfig = match serde_json::from_str(&json) {
        Ok(c) => c,
        Err(_) => return Ok(()), // unparseable config — skip seeding
    };

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    for text in &config.custom_conditions {
        let chip = ConditionChip {
            id: deterministic_id(text),
            text: text.clone(),
            updated_at: now.clone(),
            deleted_at: None,
        };
        let _ = conn.execute(
            "INSERT OR IGNORE INTO condition_chips (id, text, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, NULL)",
            rusqlite::params![chip.id, chip.text, chip.updated_at],
        );
    }

    tracing::info!(count = config.custom_conditions.len(), "Seeded condition chips from custom_conditions");
    Ok(())
}
```

- [ ] **Step 2: Register the migration in `crates/db/src/migrations/mod.rs`**

Add after line 15 (`pub mod m009_soft_delete;`):

```rust
pub mod m010_condition_chips;
```

In the `all_migrations()` array, after the m009 entry (before the closing `]`), add:

```rust
        Migration { version: 10, name: "condition_chips", up: m010_condition_chips::up },
```

- [ ] **Step 3: Run the full migration test suite**

Run: `cargo test -p medical-db --lib migrations`
Expected: all migration tests pass including the new m010.

- [ ] **Step 4: Run the condition_chips repo tests again (now with real migration)**

Run: `cargo test -p medical-db --lib condition_chips`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/db/src/migrations/m010_condition_chips.rs crates/db/src/migrations/mod.rs
git commit -m "feat(db): m010 migration — condition_chips table + seed from custom_conditions"
```

---

## Task 4: Add `sync_condition_chips` setting

**Files:**
- Modify: `crates/core/src/types/settings.rs` (before line 491)
- Modify: `src/lib/types/index.ts` (lines 77-134)
- Modify: `src/lib/stores/settings.svelte.ts` (lines 4-53)

- [ ] **Step 1: Add the Rust field**

In `crates/core/src/types/settings.rs`, add before line 491 (just before the closing `}` of `AppConfig`), after the `capture_for_training` field:

```rust
    // Condition chip sync
    /// When true, condition chip presets sync two-way between this machine
    /// and the paired server via the vocab API. Defaults to false — each
    /// machine keeps its own list unless the user opts in.
    #[serde(default)]
    pub sync_condition_chips: bool,
```

- [ ] **Step 2: Add the backward-compat test**

In `crates/core/src/types/settings.rs`, in the `#[cfg(test)]` module, add:

```rust
    #[test]
    fn sync_condition_chips_defaults_to_false_in_older_configs() {
        let old_json = r#"{"ai_provider":"ollama","stt_mode":"local"}"#;
        let cfg: AppConfig = serde_json::from_str(old_json).expect("should parse with serde defaults");
        assert!(!cfg.sync_condition_chips, "default must be false");
    }
```

- [ ] **Step 3: Run Rust test**

Run: `cargo test -p medical-core --lib sync_condition_chips`
Expected: 1 test passes.

- [ ] **Step 4: Add the TS type**

In `src/lib/types/index.ts`, inside the `AppConfig` interface, after the `capture_for_training` field (around line 125), add:

```typescript
  /** When true, condition chip presets sync two-way with the paired server. Defaults to false. */
  sync_condition_chips: boolean;
```

- [ ] **Step 5: Add the frontend default**

In `src/lib/stores/settings.svelte.ts`, in the `defaults` object (lines 4-53), after `custom_conditions: []`, add:

```typescript
  sync_condition_chips: false,
```

- [ ] **Step 6: Run type check**

Run: `npm run check`
Expected: 0 errors, 0 warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/types/settings.rs src/lib/types/index.ts src/lib/stores/settings.svelte.ts
git commit -m "feat(settings): add sync_condition_chips opt-in (defaults false)"
```

---

## Task 5: Server API handlers

**Files:**
- Modify: `src-tauri/src/sharing_vocab_api.rs`

- [ ] **Step 1: Read the current router setup**

Read `src-tauri/src/sharing_vocab_api.rs` lines 68-121 to see the exact Router construction. Find the `.route()` chain.

- [ ] **Step 2: Add the two new routes**

In the Router builder chain (after the user-dictionary routes, before `.with_state`), add:

```rust
        .route("/v1/condition-chips", get(condition_chips_list_handler))
        .route("/v1/condition-chips/sync", post(condition_chips_sync_handler))
```

Add `use axum::routing::{get, post};` if not already imported (check the existing imports — templates use `post` already, so it may be imported).

- [ ] **Step 3: Add the handler functions**

At the end of `sharing_vocab_api.rs` (before the closing of the module, or in the same file as the other handlers), add:

```rust
// ──────────────────────────────────────────────────────────────────────────
// Condition chips handlers
// ──────────────────────────────────────────────────────────────────────────

/// GET /v1/condition-chips — return all active condition chips.
async fn condition_chips_list_handler(
    State(state): State<ApiState>,
) -> Result<Json<Vec<medical_core::types::condition_chip::ConditionChip>>, AppError> {
    let _client_id = authorize(&state).await?;
    let db = Arc::clone(&state.db);
    let chips = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::list_active(&conn)
            .map_err(AppError::from)
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;
    tracing::debug!(count = chips.len(), "condition chips listed");
    Ok(Json(chips))
}

/// POST /v1/condition-chips/sync — two-way merge.
/// Body: the client's full chip list (active + tombstones).
/// Returns: the merged active chip list.
async fn condition_chips_sync_handler(
    State(state): State<ApiState>,
    Json(incoming): Json<Vec<medical_core::types::condition_chip::ConditionChip>>,
) -> Result<Json<Vec<medical_core::types::condition_chip::ConditionChip>>, AppError> {
    let _client_id = authorize(&state).await?;
    let db = Arc::clone(&state.db);

    // Prune old tombstones opportunistically (30 days).
    let cutoff = chrono::Utc::now()
        - chrono::Duration::days(30);
    let cutoff_iso = cutoff.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let merged = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        let result =
            medical_db::condition_chips::ConditionChipsRepo::merge_incoming(&conn, &incoming)
                .map_err(AppError::from)?;
        // Best-effort prune — don't fail the sync if pruning errors.
        let _ = medical_db::condition_chips::ConditionChipsRepo::prune_tombstones(&conn, &cutoff_iso);
        Ok::<_, AppError>(result)
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    tracing::debug!(
        incoming = incoming.len(),
        result = merged.len(),
        "condition chips synced"
    );
    Ok(Json(merged))
}
```

- [ ] **Step 4: Build to verify compilation**

Run: `cargo build -p rust-medical-assistant 2>&1 | tail -20`
Expected: compiles without errors. If `Json` or `State` are not imported, add them from the existing imports section.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/sharing_vocab_api.rs
git commit -m "feat(sharing): add /v1/condition-chips GET + /sync POST server handlers"
```

---

## Task 6: `ConditionsRemote` HTTP client

**Files:**
- Create: `src-tauri/src/conditions_remote.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod conditions_remote;`)

- [ ] **Step 1: Create the remote client**

Create `src-tauri/src/conditions_remote.rs`:

```rust
//! HTTP client for condition chip sync with a paired server.
//!
//! Mirrors `templates_remote.rs` — same `from()` constructor gating on
//! `conn.ports.vocab`, same bearer auth, same graceful None fallback for
//! old servers that don't have the `/v1/condition-chips` routes.

use std::sync::Arc;
use std::time::Duration;

use medical_core::types::condition_chip::ConditionChip;

use crate::commands::sharing::PairedConnection;
use medical_core::error::{AppError, AppResult};

/// HTTP client for the condition-chips sync API on a paired server.
pub struct ConditionsRemote<'a> {
    conn: &'a PairedConnection,
    bearer: String,
    client: Arc<reqwest::Client>,
}

impl<'a> ConditionsRemote<'a> {
    /// Construct from a paired connection. Returns `None` if there's no
    /// bearer token or no vocab port (old server without condition-chips
    /// routes) — callers fall back to local DB.
    pub fn from(
        conn: &'a PairedConnection,
        bearer: Option<String>,
        client: Arc<reqwest::Client>,
    ) -> Option<Self> {
        let bearer = bearer?;
        conn.ports.vocab?;
        Some(Self { conn, bearer, client })
    }

    /// Build the base URL for the vocab API, preferring LAN then Tailscale.
    fn base_url(&self) -> Option<String> {
        let port = self.conn.ports.vocab?;
        let host = self.conn.lan.as_ref().or(self.conn.tailscale.as_ref())?;
        Some(format!("http://{}:{}", host, port))
    }

    /// GET /v1/condition-chips — pull all active chips from the server.
    pub async fn list(&self) -> AppResult<Vec<ConditionChip>> {
        let url = format!("{}/v1/condition-chips", self.base_url().ok_or_else(|| {
            AppError::Other("no vocab base URL for conditions remote".into())
        })?);
        let resp = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("conditions remote list: {e}")))?;
        check_status(&resp).await?;
        resp.json::<Vec<ConditionChip>>()
            .await
            .map_err(|e| AppError::Other(format!("conditions remote parse: {e}")))
    }

    /// POST /v1/condition-chips/sync — push local list, get merged list back.
    pub async fn sync(&self, local_chips: Vec<ConditionChip>) -> AppResult<Vec<ConditionChip>> {
        let url = format!("{}/v1/condition-chips/sync", self.base_url().ok_or_else(|| {
            AppError::Other("no vocab base URL for conditions remote".into())
        })?);
        let resp = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(15))
            .bearer_auth(&self.bearer)
            .json(&local_chips)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("conditions remote sync: {e}")))?;
        check_status(&resp).await?;
        resp.json::<Vec<ConditionChip>>()
            .await
            .map_err(|e| AppError::Other(format!("conditions remote parse: {e}")))
    }
}

/// Check HTTP status and map common error codes to user-facing messages.
async fn check_status(resp: &reqwest::Response) -> AppResult<()> {
    if resp.status().is_success() {
        return Ok(());
    }
    let status = resp.status();
    match status {
        reqwest::StatusCode::NOT_FOUND => Err(AppError::Other(
            "Server does not support condition chip sync (update required)".into(),
        )),
        reqwest::StatusCode::UNAUTHORIZED => Err(AppError::Other(
            "Authentication failed — re-pair with the server".into(),
        )),
        _ => Err(AppError::Other(format!("Server error: {status}"))),
    }
}
```

- [ ] **Step 2: Register the module**

In `src-tauri/src/lib.rs`, near the other `mod *_remote;` declarations (search for `mod templates_remote`), add:

```rust
mod conditions_remote;
```

- [ ] **Step 3: Build to verify**

Run: `cargo build -p rust-medical-assistant 2>&1 | tail -20`
Expected: compiles without errors.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/conditions_remote.rs src-tauri/src/lib.rs
git commit -m "feat(sharing): add ConditionsRemote HTTP client for chip sync"
```

---

## Task 7: Tauri commands + dispatch

**Files:**
- Create: `src-tauri/src/commands/conditions.rs`
- Modify: `src-tauri/src/commands/mod.rs` (line ~5, alphabetical)
- Modify: `src-tauri/src/lib.rs` (add to `generate_handler!`)

- [ ] **Step 1: Create the commands file**

Create `src-tauri/src/commands/conditions.rs`:

```rust
//! Tauri commands for condition chips — list, add, remove, sync.
//!
//! Each command checks whether sync is enabled (`sync_condition_chips` setting)
//! and whether this machine is paired. If both are true, operations route to
//! the paired server via `ConditionsRemote`. Otherwise, they operate on the
//! local DB. Sync failures never block the UI — a failed push leaves the
//! local state correct, and the next successful sync reconciles.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::types::condition_chip::ConditionChip;

use crate::state::AppState;

/// Check if condition chip sync is on AND we're paired.
/// Returns the paired target if sync should route to the server.
fn paired_conditions_target(
    state: &AppState,
) -> Option<(crate::commands::sharing::PairedConnection, String)> {
    // Gate on the opt-in setting.
    let config = crate::commands::settings::load_config_sync(&state.db).ok()?;
    if !config.sync_condition_chips {
        return None;
    }
    // Gate on pairing.
    let conn = crate::state::load_paired_connection()?;
    conn.ports.vocab?;
    let bearer = crate::state::load_sharing_bearer()?;
    Some((conn, bearer))
}

/// List all active condition chips.
/// If sync is on + paired, pulls from the server (which may trigger a merge
/// on the next sync). For the initial pull-on-connect, this returns the
/// server's list directly.
#[tauri::command]
#[instrument(skip(state), name = "conditions::list")]
pub async fn list_condition_chips(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<ConditionChip>> {
    if let Some((conn, bearer)) = paired_conditions_target(&state) {
        if let Some(remote) = crate::conditions_remote::ConditionsRemote::from(
            &conn,
            Some(bearer),
            state.http_client.clone(),
        ) {
            // Pull from server, merge locally, return result.
            let server_chips = remote.list().await.map_err(|e| {
                tracing::warn!(error = %e, "conditions remote list failed, using local");
                e
            })?;
            let db = Arc::clone(&state.db);
            return tokio::task::spawn_blocking(move || {
                let conn = db.conn()?;
                // Merge server chips into local to stay in sync.
                let merged = medical_db::condition_chips::ConditionChipsRepo::merge_incoming(
                    &conn, &server_chips,
                )
                .map_err(AppError::from)?;
                Ok(merged)
            })
            .await
            .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;
        }
    }
    // Local fallback.
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::list_active(&conn)
            .map_err(AppError::from)
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}

/// Add a condition chip. Updates local DB immediately, then pushes to server
/// if sync is enabled.
#[tauri::command]
#[instrument(skip(state), name = "conditions::add")]
pub async fn add_condition_chip(
    state: tauri::State<'_, AppState>,
    text: String,
) -> AppResult<Vec<ConditionChip>> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    // 1. Update local DB immediately (instant UI).
    let db = Arc::clone(&state.db);
    let local_list = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::add(&conn, &text, &now)
            .map_err(AppError::from)
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    // 2. Best-effort background sync (non-blocking — local state is already correct).
    if let Some((conn, bearer)) = paired_conditions_target(&state) {
        if let Some(remote) = crate::conditions_remote::ConditionsRemote::from(
            &conn,
            Some(bearer),
            state.http_client.clone(),
        ) {
            let chips_to_push = local_list.clone();
            // Fire and forget — failures log but don't surface.
            tokio::spawn(async move {
                match remote.sync(chips_to_push).await {
                    Ok(_) => tracing::debug!("condition chip sync push succeeded"),
                    Err(e) => tracing::warn!(error = %e, "condition chip sync push failed (will retry on next pull)"),
                }
            });
        }
    }

    Ok(local_list)
}

/// Remove a condition chip by text. Soft-deletes locally, then pushes.
#[tauri::command]
#[instrument(skip(state), name = "conditions::remove")]
pub async fn remove_condition_chip(
    state: tauri::State<'_, AppState>,
    text: String,
) -> AppResult<Vec<ConditionChip>> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    // 1. Soft-delete locally.
    let db = Arc::clone(&state.db);
    let local_list = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::remove_by_text(&conn, &text, &now)
            .map_err(AppError::from)
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    // 2. Best-effort background sync.
    if let Some((conn, bearer)) = paired_conditions_target(&state) {
        if let Some(remote) = crate::conditions_remote::ConditionsRemote::from(
            &conn,
            Some(bearer),
            state.http_client.clone(),
        ) {
            // For sync, we need the full list INCLUDING the tombstone we just created.
            let db2 = Arc::clone(&state.db);
            let all_chips = tokio::task::spawn_blocking(move || {
                let conn = db2.conn()?;
                medical_db::condition_chips::ConditionChipsRepo::list_all(&conn)
                    .map_err(AppError::from)
            })
            .await
            .map_err(|e| AppError::Other(format!("Task join error: {e}")))?;

            if let Ok(all_chips) = all_chips {
                tokio::spawn(async move {
                    match remote.sync(all_chips).await {
                        Ok(_) => tracing::debug!("condition chip sync push (remove) succeeded"),
                        Err(e) => tracing::warn!(error = %e, "condition chip sync push (remove) failed"),
                    }
                });
            }
        }
    }

    Ok(local_list)
}

/// Manual sync trigger — push local full list, merge with server, return result.
/// Used when toggling sync on or reconnecting.
#[tauri::command]
#[instrument(skip(state), name = "conditions::sync")]
pub async fn sync_condition_chips_cmd(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<ConditionChip>> {
    // Load local full list (including tombstones).
    let db = Arc::clone(&state.db);
    let local_all = tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::list_all(&conn)
            .map_err(AppError::from)
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    if let Some((conn, bearer)) = paired_conditions_target(&state) {
        if let Some(remote) = crate::conditions_remote::ConditionsRemote::from(
            &conn,
            Some(bearer),
            state.http_client.clone(),
        ) {
            let merged = remote.sync(local_all).await?;
            // Merge the server result back into local.
            let db = Arc::clone(&state.db);
            return tokio::task::spawn_blocking(move || {
                let conn = db.conn()?;
                medical_db::condition_chips::ConditionChipsRepo::merge_incoming(&conn, &merged)
                    .map_err(AppError::from)
            })
            .await
            .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;
        }
    }

    // No pairing — just return active local list.
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let conn = db.conn()?;
        medical_db::condition_chips::ConditionChipsRepo::list_active(&conn)
            .map_err(AppError::from)
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
}
```

- [ ] **Step 2: Register the module in `commands/mod.rs`**

In `src-tauri/src/commands/mod.rs`, add after `pub mod audio;` (line ~2):

```rust
pub mod conditions;
```

- [ ] **Step 3: Register commands in `generate_handler!`**

In `src-tauri/src/lib.rs`, in the `tauri::generate_handler![...]` macro (search for `commands::user_dictionary`), add after the user_dictionary entries:

```rust
        commands::conditions::list_condition_chips,
        commands::conditions::add_condition_chip,
        commands::conditions::remove_condition_chip,
        commands::conditions::sync_condition_chips_cmd,
```

- [ ] **Step 4: Check that `load_config_sync` exists**

Run: `grep -n 'fn load_config_sync' src-tauri/src/commands/settings.rs`

If it does NOT exist, add a synchronous config loader. If it exists, skip. The function should be:

```rust
/// Synchronously load the app config (for use in non-async dispatch helpers).
pub fn load_config_sync(db: &Arc<medical_db::Database>) -> Result<medical_core::types::settings::AppConfig, AppError> {
    let conn = db.conn().map_err(AppError::from)?;
    medical_db::settings::SettingsRepo::load_config(&conn)
        .map(|mut c| { c.migrate(); c })
        .map_err(AppError::from)
}
```

Add this to `src-tauri/src/commands/settings.rs` if missing.

- [ ] **Step 5: Build to verify**

Run: `cargo build -p rust-medical-assistant 2>&1 | tail -30`
Expected: compiles without errors. Fix any missing imports.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/commands/conditions.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs src-tauri/src/commands/settings.rs
git commit -m "feat(commands): condition chip Tauri commands with sync dispatch"
```

---

## Task 8: Frontend — API helpers + rewire `ConditionChips.svelte`

**Files:**
- Create: `src/lib/api/conditions.ts`
- Modify: `src/lib/components/ConditionChips.svelte`

- [ ] **Step 1: Create the API helpers**

Create `src/lib/api/conditions.ts`:

```typescript
import { invoke } from '@tauri-apps/api/core';

export interface ConditionChip {
  id: string;
  text: string;
  updated_at: string;
  deleted_at: string | null;
}

export async function listConditionChips(): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('list_condition_chips');
}

export async function addConditionChip(text: string): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('add_condition_chip', { text });
}

export async function removeConditionChip(text: string): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('remove_condition_chip', { text });
}

export async function syncConditionChips(): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('sync_condition_chips_cmd');
}
```

- [ ] **Step 2: Read the current `ConditionChips.svelte`**

Read `src/lib/components/ConditionChips.svelte` fully to understand current structure before modifying.

- [ ] **Step 3: Rewire the chip component**

The key changes to `ConditionChips.svelte`:
1. Remove the `settings` import and `settings.state.custom_conditions` reads.
2. Add a `chips` local state populated by `listConditionChips()` on mount.
3. Change `addNewCondition` to call `addConditionChip(text)` and update local state from the response.
4. Change `removeCondition` to call `removeConditionChip(text)` and update local state from the response.

Replace the script section (lines ~1-65) with:

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { addConditionChip, listConditionChips, removeConditionChip } from '../api/conditions';

  let { onAdd }: { onAdd: (condition: string) => void } = $props();

  const DEFAULT_CONDITIONS = [
    'Hypertension', 'Type 2 diabetes', 'Hyperlipidemia', 'Asthma', 'COPD',
    'Hypothyroidism', 'Atrial fibrillation', 'Coronary artery disease',
    'CKD (chronic kidney disease)', 'GERD', 'Anxiety', 'Depression',
    'Osteoarthritis', 'Obesity', 'Sleep apnea',
  ];

  let chips = $state<string[]>([]);
  let loaded = $state(false);
  let adding = $state(false);
  let newCondition = $state('');

  // Display defaults until the backend list loads (or if it's empty).
  let conditions = $derived(loaded && chips.length > 0 ? chips : DEFAULT_CONDITIONS);

  onMount(async () => {
    try {
      chips = (await listConditionChips()).map((c) => c.text);
    } catch (e) {
      console.error('Failed to load condition chips:', e);
    }
    loaded = true;
  });

  async function addNewCondition() {
    const trimmed = newCondition.trim();
    if (!trimmed) return;
    // Dedup check (case-insensitive).
    if (conditions.some((c) => c.toLowerCase() === trimmed.toLowerCase())) {
      newCondition = '';
      adding = false;
      return;
    }
    try {
      const updated = await addConditionChip(trimmed);
      chips = updated.map((c) => c.text);
    } catch (e) {
      console.error('Failed to add condition chip:', e);
    }
    newCondition = '';
    adding = false;
  }

  async function removeCondition(condition: string) {
    try {
      const updated = await removeConditionChip(condition);
      chips = updated.map((c) => c.text);
    } catch (e) {
      console.error('Failed to remove condition chip:', e);
    }
  }
</script>
```

Keep the markup section (lines ~78-120) unchanged — it already iterates `conditions` and calls `addNewCondition`/`removeCondition`.

- [ ] **Step 4: Run type check**

Run: `npm run check`
Expected: 0 errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/api/conditions.ts src/lib/components/ConditionChips.svelte
git commit -m "feat(frontend): rewire ConditionChips to Tauri commands (prep for sync)"
```

---

## Task 9: Frontend — settings toggle

**Files:**
- Modify: `src/lib/components/settings/Sharing.svelte`

- [ ] **Step 1: Read the current `Sharing.svelte`**

Read `src/lib/components/settings/Sharing.svelte` fully.

- [ ] **Step 2: Add the settings import + toggle**

Add at the top of the script section:

```typescript
  import { settings } from '../../stores/settings.svelte';
  import { syncConditionChips } from '../../api/conditions';
```

After the mode selector section (after line ~53, before the conditional sub-component rendering), add the toggle. This should render whenever sharing is active (server or client mode):

```svelte
  {#if sharingOn}
    <label class="checkbox-row" style="margin-top: 1rem;">
      <input
        type="checkbox"
        checked={settings.state.sync_condition_chips ?? false}
        onchange={async (e) => {
          const checked = (e.target as HTMLInputElement).checked;
          settings.updateField('sync_condition_chips', checked);
          if (checked) {
            try {
              await syncConditionChips();
            } catch (err) {
              console.error('Initial condition chip sync failed:', err);
            }
          }
        }}
      />
      <span>
        Sync known condition chips with the server
        <p class="hint">
          When enabled, your condition chip presets sync two-way between this
          machine and the server. Other clients' changes appear on reconnect.
          Off by default — each machine keeps its own list.
        </p>
      </span>
    </label>
  {/if}
```

- [ ] **Step 3: Run type check**

Run: `npm run check`
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/settings/Sharing.svelte
git commit -m "feat(frontend): add condition chip sync toggle in Sharing settings"
```

---

## Task 10: Frontend test

**Files:**
- Create: `src/lib/components/ConditionChips.test.ts`

- [ ] **Step 1: Write the test**

Create `src/lib/components/ConditionChips.test.ts`:

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

// Mock the conditions API before importing the component.
const mockListConditionChips = vi.fn();
const mockAddConditionChip = vi.fn();
const mockRemoveConditionChip = vi.fn();

vi.mock('../api/conditions', () => ({
  listConditionChips: mockListConditionChips,
  addConditionChip: mockAddConditionChip,
  removeConditionChip: mockRemoveConditionChip,
}));

import ConditionChips from './ConditionChips.svelte';

beforeEach(() => {
  vi.clearAllMocks();
  mockListConditionChips.mockResolvedValue([]);
  mockAddConditionChip.mockResolvedValue([]);
  mockRemoveConditionChip.mockResolvedValue([]);
});

describe('ConditionChips', () => {
  it('renders default conditions while loading', async () => {
    mockListConditionChips.mockReturnValue(new Promise(() => {})); // never resolves
    render(ConditionChips, { props: { onAdd: () => {} } });
    // Default conditions should show immediately.
    expect(screen.getByText('Hypertension')).toBeTruthy();
    expect(screen.getByText('Asthma')).toBeTruthy();
  });

  it('loads chips from backend on mount', async () => {
    mockListConditionChips.mockResolvedValue([
      { id: '1', text: 'Custom Condition', updated_at: '', deleted_at: null },
    ]);
    render(ConditionChips, { props: { onAdd: () => {} } });
    await waitFor(() => {
      expect(screen.getByText('Custom Condition')).toBeTruthy();
    });
  });

  it('calls addConditionChip when adding a new chip', async () => {
    mockAddConditionChip.mockResolvedValue([
      { id: '1', text: 'NewCond', updated_at: '', deleted_at: null },
    ]);
    render(ConditionChips, { props: { onAdd: () => {} } });
    // Click the + button to reveal the input.
    const addButton = screen.getByText('+');
    await fireEvent.click(addButton);
    // Type and submit.
    const input = screen.getByPlaceholderText(/add/i) as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'NewCond' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => {
      expect(mockAddConditionChip).toHaveBeenCalledWith('NewCond');
    });
  });

  it('falls back gracefully when list fails', async () => {
    mockListConditionChips.mockRejectedValue(new Error('network'));
    render(ConditionChips, { props: { onAdd: () => {} } });
    // Should still show defaults, no crash.
    await waitFor(() => {
      expect(screen.getByText('Hypertension')).toBeTruthy();
    });
  });
});
```

- [ ] **Step 2: Check if `@testing-library/svelte` is available**

Run: `grep '@testing-library/svelte' package.json`

If NOT present, install it:
Run: `npm install -D @testing-library/svelte`

- [ ] **Step 3: Run the test**

Run: `npx vitest run src/lib/components/ConditionChips.test.ts`
Expected: tests pass (may need minor selector adjustments based on actual markup).

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/ConditionChips.test.ts
git commit -m "test(frontend): ConditionChips component tests for sync-aware behavior"
```

---

## Task 11: Full integration verification

- [ ] **Step 1: Run the complete Rust test suite**

Run: `cargo test --workspace --lib 2>&1 | tail -30`
Expected: all pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace 2>&1 | tail -20`
Expected: no warnings.

- [ ] **Step 3: Run frontend tests**

Run: `npx vitest run 2>&1 | tail -20`
Expected: all pass.

- [ ] **Step 4: Run type check**

Run: `npm run check`
Expected: 0 errors.

- [ ] **Step 5: Manual smoke test (optional, if dev environment available)**

Run: `npm run tauri dev`
- Open the app, go to Settings → Sharing.
- Verify the "Sync known condition chips" toggle appears when sharing is on.
- Go to Record tab, verify chips load and display.
- Add a chip, verify it persists.
- Remove a chip, verify it disappears.

- [ ] **Step 6: Final commit (if any remaining changes)**

```bash
git add -A
git commit -m "test: full integration verification for condition chip sync"
```

---

## Self-Review Notes

### Spec coverage check
- ✅ Data model (table + struct + repo) — Tasks 1, 2, 3
- ✅ Merge algorithm (LWW + tombstones) — Task 2 (comprehensive tests)
- ✅ API layer (server handlers + remote client) — Tasks 5, 6
- ✅ Sync flow (push on edit, pull on connect) — Task 7 (commands)
- ✅ Opt-in setting — Task 4
- ✅ Frontend integration — Tasks 8, 9
- ✅ Error handling (graceful degradation) — Task 7 (paired_conditions_target gates + try/catch)
- ✅ PHI (log counts not text) — handlers use `count = chips.len()`
- ✅ Testing strategy — Tasks 1, 2, 4, 10 + integration Task 11

### Type consistency check
- `ConditionChip` struct fields match across: core type, DB repo, server DTO, remote client, TS interface.
- `deterministic_id` used consistently in repo `add`/`remove_by_text` and tests.
- Command names: `list_condition_chips`, `add_condition_chip`, `remove_condition_chip`, `sync_condition_chips_cmd` — consistent between Rust and TS.
- The Tauri command `sync_condition_chips_cmd` is suffixed `_cmd` to avoid clashing with the setting field name `sync_condition_chips`.

### Known caveats for implementer
1. The `load_config_sync` helper in `commands/settings.rs` may not exist — Task 7 Step 4 handles this.
2. The frontend test selectors (placeholder text, button text) may need adjustment based on actual markup — read the real `ConditionChips.svelte` before finalizing.
3. `chrono::Duration::days(30)` in the prune cutoff — ensure `chrono` has the `chrono` crate available in `src-tauri` (it should, it's used extensively).
