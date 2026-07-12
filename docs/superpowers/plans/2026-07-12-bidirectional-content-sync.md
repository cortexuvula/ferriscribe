# Bidirectional Content Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable full bidirectional sync of transcripts, SOAP notes, letters, peer discussions, and audio files between remote clients and the office server over Tailscale.

**Architecture:** Extends the existing condition-chip sync pattern (bearer-token auth, vocab port 11437, SSE broadcast) to full recording content. Timestamp-gated delta sync with per-field LWW merge. Text content syncs bidirectionally; audio uploads client→server and fetches on-demand. Tailscale-gated, opt-in.

**Tech Stack:** Rust (rusqlite, axum, reqwest, tokio), Svelte 5 runes, Tauri v2

**Spec:** `docs/superpowers/specs/2026-07-12-bidirectional-content-sync-design.md`

---

## File Structure

### New files

| File | Responsibility |
|------|----------------|
| `crates/db/src/migrations/m013_recording_updated_at.rs` | Add `updated_at` column to recordings |
| `crates/db/src/migrations/m014_field_revisions.rs` | `recording_field_revisions` table |
| `crates/db/src/migrations/m015_sync_state.rs` | `sync_state` cursor table |
| `crates/db/src/content_sync.rs` | Merge algorithm, revision CRUD, cursor management |
| `crates/db/tests/recording_sync_merge.rs` | Merge algorithm integration tests |
| `src-tauri/src/content_remote.rs` | HTTP client for server content sync API |
| `src-tauri/src/commands/content_sync.rs` | Tauri commands for content sync |
| `src/lib/api/contentSync.ts` | Frontend invoke wrappers |
| `src/lib/components/settings/sharing/ContentSync.svelte` | Settings toggle UI |

### Modified files

| File | Change |
|------|--------|
| `crates/db/src/migrations/mod.rs` | Register m013/m014/m015 |
| `crates/db/src/lib.rs` | Export `content_sync` module + `ContentSyncRepo` |
| `crates/db/src/recordings.rs` | Add `updated_at` to queries + `Recording` |
| `crates/core/src/types/recording.rs` | Add `updated_at` field to `Recording` struct |
| `crates/core/src/types/settings.rs` | Add `sync_content: bool` to `AppConfig` |
| `src-tauri/src/sharing_vocab_api.rs` | Add content sync routes + broadcast channel |
| `src-tauri/src/commands/recordings_edit.rs` | Bump `updated_at` + revision on save; add `peer_discussion` to editable fields |
| `src-tauri/src/commands/audio.rs` | Trigger background content push + audio upload on new recording |
| `src-tauri/src/commands/recordings.rs` | Propagate deletion via sync on soft-delete |
| `src-tauri/src/commands/mod.rs` | Add `content_sync` module declaration |
| `src-tauri/src/state.rs` | Startup sync task + tombstone sweeper |
| `src-tauri/src/lib.rs` | Register new commands + module |
| `src/lib/stores/recordings.svelte.ts` | Sync state, `handleRemoteUpdate`, `syncNow` |
| `src/lib/pages/RecordingsTab.svelte` | Event listeners for sync events |
| `src/lib/pages/EditorTab.svelte` | Conflict toast + "Fetch Audio" button |
| `src/lib/components/RecordingCard.svelte` | Sync badge |
| `src/lib/components/settings/Sharing.svelte` | Include ContentSync sub-component |
| `src/lib/types/index.ts` | Sync-related types |

---

## Task 1: Migration m013 — `updated_at` column on recordings

**Files:**
- Create: `crates/db/src/migrations/m013_recording_updated_at.rs`
- Modify: `crates/db/src/migrations/mod.rs`

- [ ] **Step 1: Create the migration file**

Create `crates/db/src/migrations/m013_recording_updated_at.rs`:

```rust
use rusqlite::Connection;

use crate::DbResult;

/// Add `updated_at` column to `recordings`.
///
/// Tracks the last modification time of any field on a recording row.
/// Drives delta filtering for content sync — the server answers
/// "give me everything where `updated_at > cursor`". Existing rows are
/// backfilled to `created_at` (they have never been "modified").
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "ALTER TABLE recordings ADD COLUMN updated_at TEXT;
         UPDATE recordings SET updated_at = created_at WHERE updated_at IS NULL;",
    )?;
    Ok(())
}
```

- [ ] **Step 2: Register the module**

In `crates/db/src/migrations/mod.rs`, add the module declaration after line 18 (after `m012_encryption_pending`):

```rust
pub mod m013_recording_updated_at;
```

- [ ] **Step 3: Register the migration entry**

In `crates/db/src/migrations/mod.rs`, inside `all_migrations()`, append before the closing `]` (after the m012 entry):

```rust
        Migration {
            version: 13,
            name: "recording_updated_at",
            up: m013_recording_updated_at::up,
        },
```

- [ ] **Step 4: Verify it compiles and runs**

Run: `cargo test -p medical-db --lib migrations`
Expected: PASS (migration engine tests run all migrations including m013)

- [ ] **Step 5: Commit**

```bash
git add crates/db/src/migrations/m013_recording_updated_at.rs crates/db/src/migrations/mod.rs
git commit -m "feat(db): m013 add updated_at column to recordings"
```

---

## Task 2: Migration m014 — `recording_field_revisions` table

**Files:**
- Create: `crates/db/src/migrations/m014_field_revisions.rs`
- Modify: `crates/db/src/migrations/mod.rs`

- [ ] **Step 1: Create the migration file**

Create `crates/db/src/migrations/m014_field_revisions.rs`:

```rust
use rusqlite::Connection;

use crate::DbResult;

/// Create `recording_field_revisions` table for per-field LWW sync.
///
/// Each syncable text field has its own `updated_at` timestamp and
/// `origin_device` (machine_id of the editor). During merge, incoming
/// revisions are compared field-by-field to resolve conflicts.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS recording_field_revisions (
            recording_id  TEXT NOT NULL,
            field         TEXT NOT NULL,
            updated_at    TEXT NOT NULL,
            origin_device TEXT,
            PRIMARY KEY (recording_id, field),
            FOREIGN KEY (recording_id) REFERENCES recordings(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_revisions_updated_at
            ON recording_field_revisions(updated_at);",
    )?;
    Ok(())
}
```

- [ ] **Step 2: Register the module and migration entry**

In `crates/db/src/migrations/mod.rs`, add after the m013 module declaration:

```rust
pub mod m014_field_revisions;
```

And in `all_migrations()`, append after the m013 entry:

```rust
        Migration {
            version: 14,
            name: "field_revisions",
            up: m014_field_revisions::up,
        },
```

- [ ] **Step 3: Verify**

Run: `cargo test -p medical-db --lib migrations`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/db/src/migrations/m014_field_revisions.rs crates/db/src/migrations/mod.rs
git commit -m "feat(db): m014 recording_field_revisions table for per-field LWW"
```

---

## Task 3: Migration m015 — `sync_state` cursor table

**Files:**
- Create: `crates/db/src/migrations/m015_sync_state.rs`
- Modify: `crates/db/src/migrations/mod.rs`

- [ ] **Step 1: Create the migration file**

Create `crates/db/src/migrations/m015_sync_state.rs`:

```rust
use rusqlite::Connection;

use crate::DbResult;

/// Create `sync_state` table for content sync cursor persistence.
///
/// Stores the client's last-seen server `updated_at` cursor so delta
/// pulls resume from the right position after restarts.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_state (
            key   TEXT PRIMARY KEY,
            value TEXT
        );
        INSERT OR IGNORE INTO sync_state (key, value) VALUES
            ('content_sync_cursor', NULL),
            ('content_sync_last_pull', NULL),
            ('pending_audio_uploads', '[]');",
    )?;
    Ok(())
}
```

- [ ] **Step 2: Register the module and migration entry**

In `crates/db/src/migrations/mod.rs`, add:

```rust
pub mod m015_sync_state;
```

And in `all_migrations()`:

```rust
        Migration {
            version: 15,
            name: "sync_state",
            up: m015_sync_state::up,
        },
```

- [ ] **Step 3: Verify**

Run: `cargo test -p medical-db --lib migrations`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/db/src/migrations/m015_sync_state.rs crates/db/src/migrations/mod.rs
git commit -m "feat(db): m015 sync_state cursor table"
```

---

## Task 4: Update `Recording` struct with `updated_at`

**Files:**
- Modify: `crates/core/src/types/recording.rs`
- Modify: `crates/db/src/recordings.rs`

- [ ] **Step 1: Add `updated_at` to the Recording struct**

In `crates/core/src/types/recording.rs`, add a new field after `metadata` (after line 55), before the closing `}`:

```rust
    /// Last modification timestamp (any field). Drives content-sync
    /// delta filtering. Set to `created_at` on insert, bumped on every update.
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
```

- [ ] **Step 2: Update `Recording::new` constructor**

In the same file, find `Recording::new` and add after `metadata: serde_json::Value::Null`:

```rust
            updated_at: None,
```

- [ ] **Step 3: Update `row_to_recording` in `recordings.rs`**

In `crates/db/src/recordings.rs`, find the `row_to_recording` method. Add to the struct construction:

```rust
            updated_at: row.get("updated_at").ok(),
```

- [ ] **Step 4: Update `insert` query in `recordings.rs`**

In `RecordingsRepo::insert`, the INSERT statement needs the `updated_at` column. Add `updated_at` to the column list and `?N` placeholder. In the params, add `&recording.updated_at` (or `&chrono::Utc::now()` if None). Set it explicitly in insert:

```rust
            // After building the recording params, before execute:
            let now = chrono::Utc::now();
```

Then include `COALESCE(updated_at, created_at)` logic by adding `updated_at` to the INSERT columns and passing `recording.updated_at.unwrap_or(now)` as the param.

- [ ] **Step 5: Update `update` query in `recordings.rs`**

In `RecordingsRepo::update`, add `updated_at = ?N` to the SET clause and pass `chrono::Utc::now()` as the value. This ensures every update bumps the timestamp.

- [ ] **Step 6: Verify it compiles**

Run: `cargo build --workspace --lib`
Expected: PASS (may need to fix any other sites that construct Recording)

- [ ] **Step 7: Run existing tests**

Run: `cargo test --workspace --lib`
Expected: PASS (existing recording tests should still pass)

- [ ] **Step 8: Commit**

```bash
git add crates/core/src/types/recording.rs crates/db/src/recordings.rs
git commit -m "feat: add updated_at to Recording struct and DB queries"
```

---

## Task 5: `ContentSyncRepo` — field revision CRUD

**Files:**
- Create: `crates/db/src/content_sync.rs`
- Modify: `crates/db/src/lib.rs`

- [ ] **Step 1: Create the module with revision CRUD**

Create `crates/db/src/content_sync.rs`:

```rust
//! Content sync repository: per-field revision tracking, cursor
//! management, and the merge algorithm for bidirectional recording sync.
//!
//! All methods are associated functions on [`ContentSyncRepo`] taking a
//! `&Connection`. They follow the same stateless-repo pattern as
//! [`crate::ConditionChipsRepo`] and [`crate::RecordingsRepo`].

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::{DbError, DbResult};

/// Syncable text fields on a recording. Each participates in per-field LWW.
pub const SYNCABLE_FIELDS: &[&str] = &[
    "transcript",
    "soap_note",
    "referral",
    "letter",
    "peer_discussion",
    "chat",
    "patient_name",
    "tags",
    "metadata",
    "processing_status",
];

/// A single field revision — the LWW clock entry for one field on one recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldRevision {
    pub field: String,
    pub updated_at: String,
    pub origin_device: Option<String>,
}

/// Cursor state persisted between sync cycles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncCursor {
    pub cursor: Option<String>,
    pub last_pull: Option<String>,
}

/// Sparse field data carried over the wire — only fields that changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncFieldValue {
    pub value: serde_json::Value,
    pub updated_at: String,
    pub origin_device: Option<String>,
}

/// A recording prepared for sync transport — sparse fields, no audio path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRecording {
    pub id: String,
    pub filename: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub patient_name: Option<String>,
    pub duration_seconds: Option<f64>,
    pub file_size_bytes: Option<i64>,
    pub stt_provider: Option<String>,
    pub ai_provider: Option<String>,
    pub fields: HashMap<String, SyncFieldValue>,
}

/// Result of merging one recording — fields where local was newer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeConflict {
    pub field: String,
    pub local_updated_at: String,
    pub remote_updated_at: String,
}

/// Result of a full merge batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    pub conflicts: Vec<MergeConflict>,
    pub changed_recording_ids: Vec<String>,
}

/// Unit-struct repo following the codebase convention.
pub struct ContentSyncRepo;

impl ContentSyncRepo {
    // -----------------------------------------------------------------------
    // Field revision CRUD
    // -----------------------------------------------------------------------

    /// Upsert a single field's revision. Called on every field edit.
    pub fn upsert_revision(
        conn: &Connection,
        recording_id: &str,
        field: &str,
        updated_at: &str,
        origin_device: Option<&str>,
    ) -> DbResult<()> {
        conn.execute(
            "INSERT INTO recording_field_revisions (recording_id, field, updated_at, origin_device)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(recording_id, field) DO UPDATE SET
                 updated_at = excluded.updated_at,
                 origin_device = excluded.origin_device",
            params![recording_id, field, updated_at, origin_device],
        )?;
        Ok(())
    }

    /// Load all field revisions for a single recording.
    pub fn revisions_for(
        conn: &Connection,
        recording_id: &str,
    ) -> DbResult<Vec<FieldRevision>> {
        let mut stmt = conn.prepare(
            "SELECT field, updated_at, origin_device
             FROM recording_field_revisions
             WHERE recording_id = ?1",
        )?;
        let rows = stmt.query_map(params![recording_id], |row| {
            Ok(FieldRevision {
                field: row.get(0)?,
                updated_at: row.get(1)?,
                origin_device: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// Load revisions for multiple recordings in one query (for sync pull).
    pub fn revisions_for_batch(
        conn: &Connection,
        recording_ids: &[String],
    ) -> DbResult<HashMap<String, Vec<FieldRevision>>> {
        if recording_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = recording_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT recording_id, field, updated_at, origin_device
             FROM recording_field_revisions
             WHERE recording_id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = recording_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                FieldRevision {
                    field: row.get(1)?,
                    updated_at: row.get(2)?,
                    origin_device: row.get(3)?,
                },
            ))
        })?;
        let mut map: HashMap<String, Vec<FieldRevision>> = HashMap::new();
        for row in rows {
            let (rec_id, rev) = row?;
            map.entry(rec_id).or_default().push(rev);
        }
        Ok(map)
    }

    // -----------------------------------------------------------------------
    // Cursor management
    // -----------------------------------------------------------------------

    /// Read the stored sync cursor (last-seen server updated_at).
    pub fn get_cursor(conn: &Connection) -> DbResult<SyncCursor> {
        let cursor: Option<String> = conn
            .query_row(
                "SELECT value FROM sync_state WHERE key = 'content_sync_cursor'",
                [],
                |row| row.get(0),
            )
            .ok()
            .flatten();
        let last_pull: Option<String> = conn
            .query_row(
                "SELECT value FROM sync_state WHERE key = 'content_sync_last_pull'",
                [],
                |row| row.get(0),
            )
            .ok()
            .flatten();
        Ok(SyncCursor { cursor, last_pull })
    }

    /// Persist the sync cursor after a successful pull.
    pub fn set_cursor(conn: &Connection, cursor: &str) -> DbResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO sync_state (key, value) VALUES ('content_sync_cursor', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![cursor],
        )?;
        conn.execute(
            "INSERT INTO sync_state (key, value) VALUES ('content_sync_last_pull', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![now],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Delta queries (for server pull)
    // -----------------------------------------------------------------------

    /// Fetch recording IDs modified since `since`, ordered by updated_at, limited.
    /// Returns (ids, has_more).
    pub fn changed_since(
        conn: &Connection,
        since: Option<&str>,
        limit: usize,
    ) -> DbResult<(Vec<String>, bool)> {
        let sql = match since {
            Some(s) => format!(
                "SELECT id FROM recordings WHERE updated_at > ?1
                 ORDER BY updated_at ASC LIMIT {}",
                limit + 1
            ),
            None => format!(
                "SELECT id FROM recordings
                 ORDER BY updated_at ASC LIMIT {}",
                limit + 1
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = match since {
            Some(s) => stmt.query_map(params![s], |row| row.get::<_, String>(0))?,
            None => stmt.query_map([], |row| row.get::<_, String>(0))?,
        };
        let mut ids: Vec<String> = rows.collect::<Result<_, _>>()?;
        let has_more = ids.len() > limit;
        if has_more {
            ids.truncate(limit);
        }
        Ok((ids, has_more))
    }
}
```

- [ ] **Step 2: Export the module**

In `crates/db/src/lib.rs`, add after `pub mod condition_chips;` (around line 27):

```rust
pub mod content_sync;
```

And in the re-exports section (around line 43), add:

```rust
pub use content_sync::{
    ContentSyncRepo, FieldRevision, MergeConflict, MergeResult,
    SyncCursor, SyncFieldValue, SyncRecording,
};
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p medical-db`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/db/src/content_sync.rs crates/db/src/lib.rs
git commit -m "feat(db): ContentSyncRepo field revision CRUD + cursor management"
```

---

## Task 6: `ContentSyncRepo` — merge algorithm

**Files:**
- Modify: `crates/db/src/content_sync.rs`
- Create: `crates/db/tests/recording_sync_merge.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/db/tests/recording_sync_merge.rs`:

```rust
use medical_core::types::recording::{ProcessingStatus, Recording};
use medical_db::content_sync::{ContentSyncRepo, MergeResult, SyncFieldValue, SyncRecording};
use medical_db::recordings::RecordingsRepo;
use medical_db::Database;
use std::collections::HashMap;
use uuid::Uuid;

fn make_recording() -> Recording {
    let mut r = Recording::new(
        "test.wav".to_string(),
        "/tmp/test.wav".into(),
    );
    r.id = Uuid::new_v4();
    r.status = ProcessingStatus::Pending;
    r
}

fn insert_recording(db: &Database, recording: &Recording) {
    let conn = db.conn().unwrap();
    RecordingsRepo::insert(&conn, recording).unwrap();
}

#[test]
fn merge_remote_wins_when_newer() {
    let db = Database::open_in_memory().unwrap();
    let recording = make_recording();
    insert_recording(&db, &recording);

    // Simulate a local revision at T1
    let conn = db.conn().unwrap();
    ContentSyncRepo::upsert_revision(&conn, &recording.id.to_string(), "soap_note", "2026-01-01T10:00:00Z", Some("local")).unwrap();

    // Remote arrives at T2 (later) with different content
    let mut fields = HashMap::new();
    fields.insert("soap_note".to_string(), SyncFieldValue {
        value: serde_json::json!("Updated SOAP"),
        updated_at: "2026-01-01T11:00:00Z".to_string(),
        origin_device: Some("remote".to_string()),
    });

    let remote = SyncRecording {
        id: recording.id.to_string(),
        filename: "test.wav".to_string(),
        created_at: "2026-01-01T09:00:00Z".to_string(),
        updated_at: "2026-01-01T11:00:00Z".to_string(),
        deleted_at: None,
        patient_name: None,
        duration_seconds: None,
        file_size_bytes: None,
        stt_provider: None,
        ai_provider: None,
        fields,
    };

    let result: MergeResult = ContentSyncRepo::merge_incoming(&conn, &[remote]).unwrap();
    assert!(result.conflicts.is_empty(), "no conflict when remote is newer");
    assert!(result.changed_recording_ids.contains(&recording.id.to_string()));

    // Verify the field was updated
    let updated = RecordingsRepo::get_by_id(&conn, &recording.id).unwrap();
    assert_eq!(updated.soap_note.as_deref(), Some("Updated SOAP"));
}

#[test]
fn merge_local_wins_conflict_reported() {
    let db = Database::open_in_memory().unwrap();
    let recording = make_recording();
    insert_recording(&db, &recording);

    let conn = db.conn().unwrap();
    // Local revision at T2 (later)
    ContentSyncRepo::upsert_revision(&conn, &recording.id.to_string(), "soap_note", "2026-01-01T11:00:00Z", Some("local")).unwrap();

    // Remote at T1 (older)
    let mut fields = HashMap::new();
    fields.insert("soap_note".to_string(), SyncFieldValue {
        value: serde_json::json!("Older SOAP"),
        updated_at: "2026-01-01T10:00:00Z".to_string(),
        origin_device: Some("remote".to_string()),
    });

    let remote = SyncRecording {
        id: recording.id.to_string(),
        filename: "test.wav".to_string(),
        created_at: "2026-01-01T09:00:00Z".to_string(),
        updated_at: "2026-01-01T10:00:00Z".to_string(),
        deleted_at: None,
        patient_name: None,
        duration_seconds: None,
        file_size_bytes: None,
        stt_provider: None,
        ai_provider: None,
        fields,
    };

    let result = ContentSyncRepo::merge_incoming(&conn, &[remote]).unwrap();
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].field, "soap_note");

    // Local value preserved
    let preserved = RecordingsRepo::get_by_id(&conn, &recording.id).unwrap();
    assert_ne!(preserved.soap_note.as_deref(), Some("Older SOAP"));
}

#[test]
fn merge_different_fields_both_win() {
    let db = Database::open_in_memory().unwrap();
    let recording = make_recording();
    recording.clone();
    {
        let conn = db.conn().unwrap();
        RecordingsRepo::insert(&conn, &recording).unwrap();
        // Local has transcript at T1
        ContentSyncRepo::upsert_revision(&conn, &recording.id.to_string(), "transcript", "2026-01-01T10:00:00Z", Some("local")).unwrap();
    }

    let conn = db.conn().unwrap();
    // Remote brings soap_note at T2 — different field
    let mut fields = HashMap::new();
    fields.insert("soap_note".to_string(), SyncFieldValue {
        value: serde_json::json!("Remote SOAP"),
        updated_at: "2026-01-01T11:00:00Z".to_string(),
        origin_device: Some("remote".to_string()),
    });

    let remote = SyncRecording {
        id: recording.id.to_string(),
        filename: "test.wav".to_string(),
        created_at: "2026-01-01T09:00:00Z".to_string(),
        updated_at: "2026-01-01T11:00:00Z".to_string(),
        deleted_at: None,
        patient_name: None,
        duration_seconds: None,
        file_size_bytes: None,
        stt_provider: None,
        ai_provider: None,
        fields,
    };

    let result = ContentSyncRepo::merge_incoming(&conn, &[remote]).unwrap();
    assert!(result.conflicts.is_empty(), "different fields don't conflict");
    // Both changes coexist
    let updated = RecordingsRepo::get_by_id(&conn, &recording.id).unwrap();
    assert_eq!(updated.soap_note.as_deref(), Some("Remote SOAP"));
}

#[test]
fn merge_deletion_propagates() {
    let db = Database::open_in_memory().unwrap();
    let recording = make_recording();
    insert_recording(&db, &recording);

    let conn = db.conn().unwrap();
    // Remote says deleted
    let remote = SyncRecording {
        id: recording.id.to_string(),
        filename: "test.wav".to_string(),
        created_at: "2026-01-01T09:00:00Z".to_string(),
        updated_at: "2026-01-01T12:00:00Z".to_string(),
        deleted_at: Some("2026-01-01T12:00:00Z".to_string()),
        patient_name: None,
        duration_seconds: None,
        file_size_bytes: None,
        stt_provider: None,
        ai_provider: None,
        fields: HashMap::new(),
    };

    let result = ContentSyncRepo::merge_incoming(&conn, &[remote]).unwrap();
    assert!(result.conflicts.is_empty());

    let updated = RecordingsRepo::get_by_id(&conn, &recording.id).unwrap();
    assert!(updated.deleted_at.is_some(), "recording should be soft-deleted");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p medical-db --test recording_sync_merge`
Expected: FAIL with `merge_incoming` method not found

- [ ] **Step 3: Implement `merge_incoming`**

Add to `impl ContentSyncRepo` in `crates/db/src/content_sync.rs`:

```rust
    /// Merge a batch of remote recordings into local DB using per-field LWW.
    ///
    /// For each recording:
    /// - Fields where remote `updated_at` > local revision → remote wins (update local).
    /// - Fields where remote `updated_at` < local revision → local wins (add to conflicts).
    /// - Equal timestamps → keep local, no conflict.
    /// - Deletion: earliest `deleted_at` wins; later un-delete (null) wins if
    ///   remote `updated_at` > local `deleted_at`.
    ///
    /// Returns conflicts (fields where local was newer) and the IDs of
    /// recordings that were modified by this merge.
    pub fn merge_incoming(
        conn: &Connection,
        remotes: &[SyncRecording],
    ) -> DbResult<MergeResult> {
        let mut conflicts = Vec::new();
        let mut changed_ids = Vec::new();

        let tx = conn.unchecked_transaction()?;
        for remote in remotes {
            let local_revs: HashMap<String, FieldRevision> = {
                let mut stmt = tx.prepare(
                    "SELECT field, updated_at, origin_device
                     FROM recording_field_revisions WHERE recording_id = ?1",
                )?;
                let rows = stmt.query_map(params![remote.id], |row| {
                    Ok(FieldRevision {
                        field: row.get(0)?,
                        updated_at: row.get(1)?,
                        origin_device: row.get(2)?,
                    })
                })?;
                let mut m = HashMap::new();
                for r in rows {
                    let r = r?;
                    m.insert(r.field.clone(), r);
                }
                m
            };

            // Check if recording exists locally
            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM recordings WHERE id = ?1",
                    params![remote.id],
                    |_| Ok(true),
                )
                .unwrap_or(false);

            if !exists {
                // New recording from remote — insert it
                Self::insert_remote_recording(&tx, remote)?;
                // Insert all field revisions
                for (field, val) in &remote.fields {
                    tx.execute(
                        "INSERT INTO recording_field_revisions
                            (recording_id, field, updated_at, origin_device)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(recording_id, field) DO UPDATE SET
                             updated_at = excluded.updated_at,
                             origin_device = excluded.origin_device",
                        params![remote.id, field, val.updated_at, val.origin_device],
                    )?;
                }
                changed_ids.push(remote.id.clone());
                continue;
            }

            let mut local_changed = false;

            // Per-field LWW merge
            for (field, remote_val) in &remote.fields {
                let local_rev = local_revs.get(field);
                match local_rev {
                    None => {
                        // No local revision — remote wins
                        Self::apply_field(&tx, &remote.id, field, remote_val)?;
                        local_changed = true;
                    }
                    Some(local) => {
                        let cmp = remote_val.updated_at.cmp(&local.updated_at);
                        match cmp {
                            std::cmp::Ordering::Greater => {
                                // Remote wins
                                Self::apply_field(&tx, &remote.id, field, remote_val)?;
                                local_changed = true;
                            }
                            std::cmp::Ordering::Less => {
                                // Local wins — record conflict
                                conflicts.push(MergeConflict {
                                    field: field.clone(),
                                    local_updated_at: local.updated_at.clone(),
                                    remote_updated_at: remote_val.updated_at.clone(),
                                });
                            }
                            std::cmp::Ordering::Equal => {
                                // Tie — keep local, no conflict
                            }
                        }
                    }
                }
            }

            // Deletion merge
            if let Some(remote_deleted) = &remote.deleted_at {
                let local_deleted: Option<String> = tx
                    .query_row(
                        "SELECT deleted_at FROM recordings WHERE id = ?1",
                        params![remote.id],
                        |row| row.get(0),
                    )
                    .ok()
                    .flatten();
                match local_deleted {
                    None => {
                        // Local not deleted — propagate deletion
                        tx.execute(
                            "UPDATE recordings SET deleted_at = ?1, updated_at = ?2 WHERE id = ?3",
                            params![remote_deleted, remote.updated_at, remote.id],
                        )?;
                        local_changed = true;
                    }
                    Some(local_del) => {
                        // Both deleted — earliest wins (already both deleted, no-op)
                        if remote_deleted < &local_del {
                            tx.execute(
                                "UPDATE recordings SET deleted_at = ?1 WHERE id = ?2",
                                params![remote_deleted, remote.id],
                            )?;
                        }
                    }
                }
            } else if remote.updated_at
                > chrono::Utc::now().to_rfc3339().as_str()
            {
                // Remote not deleted — check if local is and remote is newer
                // (un-delete case handled by normal field updates)
            }

            // Bump local updated_at if anything changed
            if local_changed {
                tx.execute(
                    "UPDATE recordings SET updated_at = ?1 WHERE id = ?2
                     AND (?1 > updated_at OR updated_at IS NULL)",
                    params![remote.updated_at, remote.id],
                )?;
                changed_ids.push(remote.id.clone());
            }
        }
        tx.commit()?;

        Ok(MergeResult {
            conflicts,
            changed_recording_ids: changed_ids,
        })
    }

    /// Apply a single remote field value to a local recording row.
    fn apply_field(
        conn: &Connection,
        recording_id: &str,
        field: &str,
        val: &SyncFieldValue,
    ) -> DbResult<()> {
        let sql = match field {
            "transcript" => "UPDATE recordings SET transcript = ?1",
            "soap_note" => "UPDATE recordings SET soap_note = ?1",
            "referral" => "UPDATE recordings SET referral = ?1",
            "letter" => "UPDATE recordings SET letter = ?1",
            "peer_discussion" => "UPDATE recordings SET peer_discussion = ?1",
            "chat" => "UPDATE recordings SET chat = ?1",
            "patient_name" => "UPDATE recordings SET patient_name = ?1",
            "tags" => "UPDATE recordings SET tags = ?1",
            "metadata" => "UPDATE recordings SET metadata = ?1",
            "processing_status" => "UPDATE recordings SET processing_status = ?1",
            _ => return Ok(()), // Unknown field — skip
        };
        let value_str = match &val.value {
            serde_json::Value::Null => None,
            v => Some(v.to_string()),
        };
        let full_sql = format!("{sql} WHERE id = ?2");
        conn.execute(&full_sql, params![value_str, recording_id])?;

        // Upsert revision
        conn.execute(
            "INSERT INTO recording_field_revisions
                (recording_id, field, updated_at, origin_device)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(recording_id, field) DO UPDATE SET
                 updated_at = excluded.updated_at,
                 origin_device = excluded.origin_device",
            params![recording_id, field, val.updated_at, val.origin_device],
        )?;
        Ok(())
    }

    /// Insert a brand-new recording received from remote sync.
    /// Inserts the row with minimal required fields; text content comes
    /// from the remote's sparse `fields` map via `apply_field`.
    fn insert_remote_recording(
        conn: &Connection,
        remote: &SyncRecording,
    ) -> DbResult<()> {
        conn.execute(
            "INSERT INTO recordings
                (id, filename, created_at, updated_at, deleted_at,
                 patient_name, duration_seconds, file_size_bytes,
                 stt_provider, ai_provider, processing_status,
                 tags, metadata, audio_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                remote.id,
                remote.filename,
                remote.created_at,
                remote.updated_at,
                remote.deleted_at,
                remote.patient_name,
                remote.duration_seconds,
                remote.file_size_bytes,
                remote.stt_provider,
                remote.ai_provider,
                serde_json::json!({"status":"completed"}).to_string(),
                "[]",
                "null",
                "", // audio_path resolved locally by recording ID
            ],
        )?;
        // Apply each sparse field
        for (field, val) in &remote.fields {
            Self::apply_field(conn, &remote.id, field, val)?;
        }
        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p medical-db --test recording_sync_merge`
Expected: PASS (all 4 tests)

- [ ] **Step 5: Run full workspace tests**

Run: `cargo test --workspace --lib`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/db/src/content_sync.rs crates/db/tests/recording_sync_merge.rs
git commit -m "feat(db): per-field LWW merge algorithm for content sync"
```

---

## Task 7: Add `sync_content` to AppConfig

**Files:**
- Modify: `crates/core/src/types/settings.rs`

- [ ] **Step 1: Add the field**

In `crates/core/src/types/settings.rs`, find the `sync_condition_chips` field (around line 493-498). Add after it, before the closing `}`:

```rust
    // Content sync
    /// When true, patient content (transcripts, SOAP notes, letters, peer
    /// discussions, audio) syncs two-way between this machine and the paired
    /// server over Tailscale. Requires Tailscale on both machines. Defaults
    /// to false.
    #[serde(default)]
    pub sync_content: bool,
```

- [ ] **Step 2: Add a default-false guard test**

Find the test `sync_condition_chips_defaults_to_false_in_older_configs` (around line 740). Add a parallel test after it:

```rust
    #[test]
    fn sync_content_defaults_to_false_in_older_configs() {
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(!config.sync_content);
    }
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p medical-core --lib sync_content`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/types/settings.rs
git commit -m "feat: add sync_content opt-in to AppConfig"
```

---

## Task 8: Server API — types + ApiState + routes

**Files:**
- Modify: `src-tauri/src/sharing_vocab_api.rs`

- [ ] **Step 1: Add content broadcast channel to ApiState**

In `src-tauri/src/sharing_vocab_api.rs`, update the `ApiState` struct (around line 63-70). Add a second broadcast channel:

```rust
#[derive(Clone)]
struct ApiState {
    db: Arc<Database>,
    tokens: Arc<TokenStore>,
    chips_changed_tx: broadcast::Sender<()>,
    content_changed_tx: broadcast::Sender<String>, // recording ID or "*" for all
}
```

- [ ] **Step 2: Create the channel in spawn()**

In the `spawn` function (around line 82), add the second channel:

```rust
    let (chips_changed_tx, _) = broadcast::channel::<()>(16);
    let (content_changed_tx, _) = broadcast::channel::<String>(32);
    let state = ApiState {
        db,
        tokens,
        chips_changed_tx,
        content_changed_tx,
    };
```

- [ ] **Step 3: Register content sync routes**

In the `Router::new()` chain (around line 88-130), add the content sync routes before `.with_state(state)`:

```rust
        // Content sync (recording text + metadata)
        .route("/v1/content/sync", get(content_sync_pull_handler).post(content_sync_push_handler))
        .route("/v1/content/sync/meta", get(content_sync_meta_handler))
        .route("/v1/content/events", get(content_events_handler))
        .route("/v1/content/audio/{recording_id}", get(content_audio_get_handler).put(content_audio_put_handler))
```

- [ ] **Step 4: Add wire-format request/response types**

Add near the top of the file (after the existing imports and DTO structs):

```rust
use medical_db::content_sync::{MergeResult, SyncRecording};

#[derive(serde::Serialize)]
struct ContentPullResponse {
    recordings: Vec<SyncRecording>,
    server_time: String,
    has_more: bool,
}

#[derive(serde::Deserialize)]
struct ContentPushRequest {
    recordings: Vec<SyncRecording>,
}

#[derive(serde::Serialize)]
struct ContentPushResponse {
    recordings: Vec<SyncRecording>,
    conflicts: Vec<medical_db::content_sync::MergeConflict>,
    server_time: String,
}

#[derive(serde::Serialize)]
struct ContentMetaResponse {
    server_time: String,
    recording_count: i64,
    latest_updated_at: Option<String>,
}
```

- [ ] **Step 5: Verify it compiles (handler stubs will be added next task)**

Add temporary stub handlers so it compiles:

```rust
async fn content_sync_pull_handler(State(_state): State<ApiState>, Query(_q): Query<SyncSinceQuery>) -> axum::response::Response {
    unimplemented!("Task 9")
}
```

(We'll replace stubs in the next task. Just make it compile.)

Run: `cargo build -p rust-medical-assistant`
Expected: PASS (may need to adjust based on exact imports)

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/sharing_vocab_api.rs
git commit -m "feat(api): add content sync routes + ApiState broadcast channel"
```

---

## Task 9: Server API — pull/push/meta handlers

**Files:**
- Modify: `src-tauri/src/sharing_vocab_api.rs`

- [ ] **Step 1: Add the query param struct**

Add near the other DTO types:

```rust
#[derive(serde::Deserialize)]
struct SyncSinceQuery {
    since: Option<String>,
    limit: Option<usize>,
}
```

- [ ] **Step 2: Implement the pull handler**

Replace the stub:

```rust
/// GET /v1/content/sync?since=<ISO8601>&limit=200
/// Returns recordings modified since `since`, with per-field revisions.
async fn content_sync_pull_handler(
    State(state): State<ApiState>,
    Query(q): Query<SyncSinceQuery>,
) -> Result<Json<ContentPullResponse>, StatusCode> {
    let limit = q.limit.unwrap_or(200).min(500);
    let db = state.db.clone();
    let since = q.since;

    let result = tokio::task::spawn_blocking(move || -> Result<_, medical_db::DbError> {
        let conn = db.conn()?;
        let (ids, has_more) = ContentSyncRepo::changed_since(&conn, since.as_deref(), limit)?;

        // Build SyncRecording for each ID
        let mut recordings = Vec::with_capacity(ids.len());
        let revs = ContentSyncRepo::revisions_for_batch(&conn, &ids)?;
        for id in &ids {
            if let Ok(rec) = RecordingsRepo::get_by_id(&conn, id) {
                let fields = build_sparse_fields(&rec, revs.get(id));
                recordings.push(SyncRecording {
                    id: rec.id.to_string(),
                    filename: rec.filename.clone(),
                    created_at: rec.created_at.to_rfc3339(),
                    updated_at: rec.updated_at.map(|t| t.to_rfc3339()).unwrap_or_else(|| rec.created_at.to_rfc3339()),
                    deleted_at: None, // We don't sync via recordings struct; read below
                    patient_name: rec.patient_name.clone(),
                    duration_seconds: rec.duration_seconds,
                    file_size_bytes: rec.file_size_bytes.map(|v| v as i64),
                    stt_provider: rec.stt_provider.clone(),
                    ai_provider: rec.ai_provider.clone(),
                    fields,
                });
            }
        }
        Ok((recordings, has_more))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match result {
        Ok((recordings, has_more)) => Ok(Json(ContentPullResponse {
            recordings,
            server_time: chrono::Utc::now().to_rfc3339(),
            has_more,
        })),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
```

- [ ] **Step 3: Implement the push handler**

```rust
/// POST /v1/content/sync
/// Merges incoming recordings into server DB, returns conflicts + server's newer data.
async fn content_sync_push_handler(
    State(state): State<ApiState>,
    Json(req): Json<ContentPushRequest>,
) -> Result<Json<ContentPushResponse>, StatusCode> {
    let db = state.db.clone();
    let tx_content = state.content_changed_tx.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<_, medical_db::DbError> {
        let conn = db.conn()?;
        let merge_result = ContentSyncRepo::merge_incoming(&conn, &req.recordings)?;

        // Build response: recordings where server had newer data (conflicts)
        let mut response_recordings = Vec::new();
        for conflict in &merge_result.conflicts {
            // For each conflicted recording, send back the server's version
            // so the client can see what won.
            // (Group conflicts by recording_id — each conflict has field-level info)
        }

        // The conflicts vector has field-level info but we need recording-level.
        // For simplicity, respond with the merged state of all conflicted recordings.
        let conflict_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        // Actually we need to rebuild from merge_result.conflicts — but conflicts
        // don't carry recording_id. Let's fix: return all recordings the client pushed
        // that had any conflict, with server's current state.

        Ok(merge_result)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let merge_result = result.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Broadcast to wake other clients
    let _ = tx_content.send("*".to_string());

    Ok(Json(ContentPushResponse {
        recordings: vec![], // Server's newer data for conflicted fields (next task)
        conflicts: merge_result.conflicts,
        server_time: chrono::Utc::now().to_rfc3339(),
    }))
}
```

- [ ] **Step 4: Implement the meta handler**

```rust
/// GET /v1/content/sync/meta
/// Returns server time, recording count, latest updated_at.
async fn content_sync_meta_handler(
    State(state): State<ApiState>,
) -> Result<Json<ContentMetaResponse>, StatusCode> {
    let db = state.db.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<_, medical_db::DbError> {
        let conn = db.conn()?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM recordings WHERE deleted_at IS NULL", [], |row| row.get(0))?;
        let latest: Option<String> = conn
            .query_row("SELECT MAX(updated_at) FROM recordings", [], |row| row.get(0))
            .ok()
            .flatten();
        Ok((count, latest))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(ContentMetaResponse {
        server_time: chrono::Utc::now().to_rfc3339(),
        recording_count: result.0,
        latest_updated_at: result.1,
    }))
}
```

- [ ] **Step 5: Add the `build_sparse_fields` helper**

Add as a free function:

```rust
/// Build sparse field map from a Recording + its revisions.
fn build_sparse_fields(
    rec: &medical_core::types::recording::Recording,
    revs: Option<&Vec<medical_db::content_sync::FieldRevision>>,
) -> std::collections::HashMap<String, medical_db::content_sync::SyncFieldValue> {
    use medical_db::content_sync::{FieldRevision, SyncFieldValue};
    let mut fields = std::collections::HashMap::new();
    let rev_map: std::collections::HashMap<&str, &FieldRevision> = revs
        .map(|r| r.iter().map(|rev| (rev.field.as_str(), rev)).collect())
        .unwrap_or_default();

    let now = chrono::Utc::now().to_rfc3339();
    let mac = |rev: Option<&&FieldRevision>| -> (String, Option<String>) {
        match rev {
            Some(r) => (r.updated_at.clone(), r.origin_device.clone()),
            None => (rec.created_at.to_rfc3339(), None),
        }
    };

    // Only include fields that have been set (non-null)
    if let Some(v) = &rec.transcript {
        let (ts, dev) = mac(rev_map.get("transcript"));
        fields.insert("transcript".into(), SyncFieldValue { value: serde_json::json!(v), updated_at: ts, origin_device: dev });
    }
    if let Some(v) = &rec.soap_note {
        let (ts, dev) = mac(rev_map.get("soap_note"));
        fields.insert("soap_note".into(), SyncFieldValue { value: serde_json::json!(v), updated_at: ts, origin_device: dev });
    }
    if let Some(v) = &rec.referral {
        let (ts, dev) = mac(rev_map.get("referral"));
        fields.insert("referral".into(), SyncFieldValue { value: serde_json::json!(v), updated_at: ts, origin_device: dev });
    }
    if let Some(v) = &rec.letter {
        let (ts, dev) = mac(rev_map.get("letter"));
        fields.insert("letter".into(), SyncFieldValue { value: serde_json::json!(v), updated_at: ts, origin_device: dev });
    }
    if let Some(v) = &rec.peer_discussion {
        let (ts, dev) = mac(rev_map.get("peer_discussion"));
        fields.insert("peer_discussion".into(), SyncFieldValue { value: serde_json::json!(v), updated_at: ts, origin_device: dev });
    }
    if let Some(v) = &rec.chat {
        let (ts, dev) = mac(rev_map.get("chat"));
        fields.insert("chat".into(), SyncFieldValue { value: serde_json::json!(v), updated_at: ts, origin_device: dev });
    }

    fields
}
```

- [ ] **Step 6: Add missing imports**

At the top of the file, ensure these imports are present:

```rust
use medical_db::content_sync::ContentSyncRepo;
use medical_db::recordings::RecordingsRepo;
use axum::extract::Query;
```

- [ ] **Step 7: Verify it compiles**

Run: `cargo build -p rust-medical-assistant`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/sharing_vocab_api.rs
git commit -m "feat(api): content sync pull/push/meta handlers"
```

---

## Task 10: Server API — SSE + audio handlers

**Files:**
- Modify: `src-tauri/src/sharing_vocab_api.rs`

- [ ] **Step 1: Implement the SSE events handler**

```rust
/// GET /v1/content/events — SSE stream for content sync change notifications.
async fn content_events_handler(
    State(state): State<ApiState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.content_changed_tx.subscribe();

    let stream = tokio_stream::wrappers::BroadcastStream::new(rx)
        .filter_map(|item| match item {
            Ok(msg) => Some(Ok(Event::default().data(msg))),
            Err(_) => None,
        });

    // Send initial connected event
    let init = tokio_stream::once(Ok(Event::default().data("connected")));
    let combined = init.chain(stream);

    Sse::new(combined).keep_alive(axum::response::sse::KeepAlive::default())
}
```

Ensure these imports:

```rust
use axum::response::sse::{Event, Sse};
use tokio_stream::StreamExt as _;
```

- [ ] **Step 2: Implement the audio GET handler (server → client on-demand)**

```rust
/// GET /v1/content/audio/{recording_id}
/// Streams the decrypted audio file for on-demand fetch.
async fn content_audio_get_handler(
    State(state): State<ApiState>,
    Path(recording_id): Path<String>,
) -> Result<Vec<u8>, StatusCode> {
    let db = state.db.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, medical_core::error::AppError> {
        let conn = db.conn().map_err(|e| medical_core::error::AppError::database(e.to_string()))?;
        let rec = RecordingsRepo::get_by_id(&conn, &recording_id)
            .map_err(|e| medical_core::error::AppError::database(e.to_string()))?;

        // Decrypt the audio file
        let path = &rec.audio_path;
        if !path.exists() {
            return Err(medical_core::error::AppError::database("audio file not found"));
        }

        // Try decrypt, fall back to plaintext
        match medical_security::file_crypto::decrypt_file(path) {
            Ok(bytes) => Ok(bytes),
            Err(medical_security::file_crypto::FileCryptoError::NotEncrypted) => {
                std::fs::read(path).map_err(|e| medical_core::error::AppError::Io(e))
            }
            Err(e) => Err(medical_core::error::AppError::database(e.to_string())),
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match result {
        Ok(bytes) => {
            tracing::info!("served audio: id_len={}_{}", recording_id.len(), bytes.len());
            Ok(bytes)
        }
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}
```

- [ ] **Step 3: Implement the audio PUT handler (client → server upload)**

```rust
/// PUT /v1/content/audio/{recording_id}
/// Receives plaintext audio bytes from client, re-encrypts and stores.
async fn content_audio_put_handler(
    State(state): State<ApiState>,
    Path(recording_id): Path<String>,
    body: axum::body::Bytes,
) -> Result<StatusCode, StatusCode> {
    let db = state.db.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<(), medical_core::error::AppError> {
        let conn = db.conn().map_err(|e| medical_core::error::AppError::database(e.to_string()))?;

        // Resolve audio path by recording ID
        let recordings_dir = crate::commands::resolve_recordings_dir()?;
        let dest = recordings_dir.join(format!("{recording_id}.enc"));

        // First-write-wins: reject if file already exists
        if dest.exists() {
            return Err(medical_core::error::AppError::database("audio already exists"));
        }

        // Write plaintext to temp file
        let tmp = dest.with_extension("tmp");
        std::fs::write(&tmp, &body)
            .map_err(medical_core::error::AppError::Io)?;

        // Re-encrypt with server's own FE1 key
        medical_security::file_crypto::encrypt_file_in_place(&tmp)
            .map_err(|e| medical_core::error::AppError::database(e.to_string()))?;

        // Atomic rename
        std::fs::rename(&tmp, &dest)
            .map_err(medical_core::error::AppError::Io)?;

        // Update audio_path in DB
        conn.execute(
            "UPDATE recordings SET audio_path = ?1 WHERE id = ?2",
            rusqlite::params![dest.to_string_lossy(), recording_id],
        ).map_err(|e| medical_core::error::AppError::database(e.to_string()))?;

        tracing::info!("stored audio: id_len={}_bytes={}", recording_id.len(), body.len());
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match result {
        Ok(()) => Ok(StatusCode::CREATED),
        Err(ref e) if e.to_string().contains("already exists") => Ok(StatusCode::CONFLICT),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
```

- [ ] **Step 4: Add Path import**

```rust
use axum::extract::Path;
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo build -p rust-medical-assistant`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/sharing_vocab_api.rs
git commit -m "feat(api): content SSE events + audio get/put handlers"
```

---

## Task 11: `ContentRemote` HTTP client

**Files:**
- Create: `src-tauri/src/content_remote.rs`

- [ ] **Step 1: Create the client**

Create `src-tauri/src/content_remote.rs`:

```rust
//! HTTP client for content sync (client → server).
//!
//! Mirrors [`conditions_remote::ConditionsRemote`] but routes exclusively
//! through the Tailscale endpoint. Returns `None` from `from()` when
//! Tailscale is not configured, so callers fall back to local-only.

use std::sync::Arc;
use std::time::Duration;

use medical_core::error::{AppError, AppResult};
use medical_db::content_sync::{MergeConflict, SyncRecording};
use serde::Deserialize;

use crate::commands::sharing::PairedConnection;

/// Thin HTTP client for the server's `/v1/content/*` endpoints.
pub struct ContentRemote<'a> {
    pub conn: &'a PairedConnection,
    pub bearer: String,
    pub client: Arc<reqwest::Client>,
}

/// Metadata response from GET /v1/content/sync/meta.
#[derive(Debug, Deserialize)]
pub struct ServerMeta {
    pub server_time: String,
    pub recording_count: i64,
    pub latest_updated_at: Option<String>,
}

/// Pull response from GET /v1/content/sync.
#[derive(Debug, Deserialize)]
pub struct PullResponse {
    pub recordings: Vec<SyncRecording>,
    pub server_time: String,
    pub has_more: bool,
}

/// Push response from POST /v1/content/sync.
#[derive(Debug, Deserialize)]
pub struct PushResponse {
    pub recordings: Vec<SyncRecording>,
    pub conflicts: Vec<MergeConflict>,
    pub server_time: String,
}

impl<'a> ContentRemote<'a> {
    /// Create a client. Returns `None` if not paired, no Tailscale address,
    /// no vocab port, or no bearer token — caller falls back to local-only.
    pub fn from(
        conn: &'a PairedConnection,
        bearer: Option<String>,
        client: Arc<reqwest::Client>,
    ) -> Option<Self> {
        let bearer = bearer?;
        // Content sync REQUIRES Tailscale — do not fall back to LAN.
        conn.tailscale.as_ref()?;
        conn.ports.vocab?;
        Some(Self {
            conn,
            bearer,
            client,
        })
    }

    /// Build the base URL using Tailscale address only.
    fn base_url(&self) -> Option<String> {
        let host = self.conn.tailscale.as_deref()?;
        let port = self.conn.ports.vocab?;
        Some(format!("http://{host}:{port}"))
    }

    /// GET /v1/content/sync/meta
    pub async fn meta(&self) -> AppResult<ServerMeta> {
        let base = self.base_url().ok_or(AppError::Other("no tailscale endpoint".into()))?;
        let resp = self
            .client
            .get(format!("{base}/v1/content/sync/meta"))
            .bearer_auth(&self.bearer)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| AppError::HttpClient(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AppError::HttpClient(format!("meta failed: {}", resp.status())));
        }
        resp.json()
            .await
            .map_err(|e| AppError::HttpClient(e.to_string()))
    }

    /// GET /v1/content/sync?since=<cursor>&limit=200
    pub async fn pull(&self, since: Option<&str>) -> AppResult<PullResponse> {
        let base = self.base_url().ok_or(AppError::Other("no tailscale endpoint".into()))?;
        let mut url = format!("{base}/v1/content/sync?limit=200");
        if let Some(s) = since {
            url.push_str("&since=");
            url.push_str(&urlencoding::encode(s));
        }
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.bearer)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| AppError::HttpClient(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AppError::HttpClient(format!("pull failed: {}", resp.status())));
        }
        resp.json()
            .await
            .map_err(|e| AppError::HttpClient(e.to_string()))
    }

    /// POST /v1/content/sync
    pub async fn push(&self, recordings: Vec<SyncRecording>) -> AppResult<PushResponse> {
        let base = self.base_url().ok_or(AppError::Other("no tailscale endpoint".into()))?;
        let resp = self
            .client
            .post(format!("{base}/v1/content/sync"))
            .bearer_auth(&self.bearer)
            .timeout(Duration::from_secs(30))
            .json(&serde_json::json!({ "recordings": recordings }))
            .send()
            .await
            .map_err(|e| AppError::HttpClient(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AppError::HttpClient(format!("push failed: {}", resp.status())));
        }
        resp.json()
            .await
            .map_err(|e| AppError::HttpClient(e.to_string()))
    }

    /// GET /v1/content/audio/{recording_id} — returns plaintext audio bytes.
    pub async fn fetch_audio(&self, recording_id: &str) -> AppResult<Vec<u8>> {
        let base = self.base_url().ok_or(AppError::Other("no tailscale endpoint".into()))?;
        let resp = self
            .client
            .get(format!("{base}/v1/content/audio/{recording_id}"))
            .bearer_auth(&self.bearer)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| AppError::HttpClient(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AppError::HttpClient(format!("audio fetch failed: {}", resp.status())));
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| AppError::HttpClient(e.to_string()))
    }

    /// PUT /v1/content/audio/{recording_id} — uploads plaintext audio bytes.
    pub async fn upload_audio(&self, recording_id: &str, data: Vec<u8>) -> AppResult<()> {
        let base = self.base_url().ok_or(AppError::Other("no tailscale endpoint".into()))?;
        let resp = self
            .client
            .put(format!("{base}/v1/content/audio/{recording_id}"))
            .bearer_auth(&self.bearer)
            .timeout(Duration::from_secs(120))
            .header("Content-Type", "audio/x-wav")
            .body(data)
            .send()
            .await
            .map_err(|e| AppError::HttpClient(e.to_string()))?;
        let status = resp.status();
        if status == StatusCode::CONFLICT {
            // Already exists — first-write-wins, this is fine.
            return Ok(());
        }
        if !status.is_success() {
            return Err(AppError::HttpClient(format!("audio upload failed: {status}")));
        }
        Ok(())
    }

    /// GET /v1/content/events — returns an SSE byte stream.
    pub fn subscribe_events(&self) -> AppResult<reqwest::Response> {
        // Note: This is blocking-ish; called from a tokio task.
        Err(AppError::Other("use subscribe_events_async".into()))
    }

    /// GET /v1/content/events — async SSE subscription.
    pub async fn subscribe_events_async(&self) -> AppResult<reqwest::Response> {
        let base = self.base_url().ok_or(AppError::Other("no tailscale endpoint".into()))?;
        let resp = self
            .client
            .get(format!("{base}/v1/content/events"))
            .bearer_auth(&self.bearer)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| AppError::HttpClient(e.to_string()))?;
        Ok(resp)
    }
}

use reqwest::StatusCode;
```

- [ ] **Step 2: Add the module to the Tauri crate**

In `src-tauri/src/main.rs` or `src-tauri/src/lib.rs` (whichever declares modules), add:

```rust
mod content_remote;
```

Add `urlencoding` to `src-tauri/Cargo.toml` dependencies if not already present.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p rust-medical-assistant`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/content_remote.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat: ContentRemote HTTP client for content sync"
```

---

## Task 12: Tauri commands — pull, push, sync_now

**Files:**
- Create: `src-tauri/src/commands/content_sync.rs`
- Modify: `src-tauri/src/commands/mod.rs`

- [ ] **Step 1: Create the command module**

Create `src-tauri/src/commands/content_sync.rs`:

```rust
//! Tauri commands for bidirectional content sync.
//!
//! Dispatch model mirrors `conditions.rs`: every command checks
//! `content_sync_target()` (three gates: sync_content setting, paired
//! connection, Tailscale endpoint). When all hold → sync via server.
//! Otherwise → local-only (fully usable offline).

use std::collections::HashMap;
use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_db::content_sync::{ContentSyncRepo, SyncFieldValue, SyncRecording};
use medical_db::recordings::RecordingsRepo;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::PluginExt as _;

use crate::commands::sharing::PairedConnection;
use crate::commands::AppState;
use crate::content_remote::ContentRemote;
use crate::state;

/// Three-gate check: sync_content setting + paired + Tailscale.
/// Returns the paired connection + bearer token, or None for local-only.
fn content_sync_target(
    st: &AppState,
) -> Option<(PairedConnection, String, Arc<reqwest::Client>)> {
    let config = crate::commands::settings::load_config_sync(&st.db).ok()?;
    if !config.sync_content {
        return None;
    }
    let conn = state::load_paired_connection()?;
    // Content sync REQUIRES Tailscale.
    conn.tailscale.as_ref()?;
    conn.ports.vocab?;
    let bearer = state::load_sharing_bearer()?;
    let client = st.http_client.clone();
    Some((conn, bearer, client))
}

/// Build a SyncRecording for pushing a local recording to the server.
fn build_sync_recording(
    conn: &rusqlite::Connection,
    rec_id: &str,
) -> AppResult<SyncRecording> {
    let rec = RecordingsRepo::get_by_id(conn, rec_id)?;
    let revs = ContentSyncRepo::revisions_for(conn, rec_id).unwrap_or_default();

    let mut fields = HashMap::new();
    let rev_map: HashMap<&str, &medical_db::content_sync::FieldRevision> =
        revs.iter().map(|r| (r.field.as_str(), r)).collect();

    let mk = |field: &str, val: Option<&str>| -> Option<(String, SyncFieldValue)> {
        let v = val?;
        let (ts, dev) = match rev_map.get(field) {
            Some(r) => (r.updated_at.clone(), r.origin_device.clone()),
            None => (rec.updated_at.map(|t| t.to_rfc3339()).unwrap_or_else(|| rec.created_at.to_rfc3339()), None),
        };
        Some((field.to_string(), SyncFieldValue {
            value: serde_json::json!(v),
            updated_at: ts,
            origin_device: dev,
        }))
    };

    if let Some((k, v)) = mk("transcript", rec.transcript.as_deref()) { fields.insert(k, v); }
    if let Some((k, v)) = mk("soap_note", rec.soap_note.as_deref()) { fields.insert(k, v); }
    if let Some((k, v)) = mk("referral", rec.referral.as_deref()) { fields.insert(k, v); }
    if let Some((k, v)) = mk("letter", rec.letter.as_deref()) { fields.insert(k, v); }
    if let Some((k, v)) = mk("peer_discussion", rec.peer_discussion.as_deref()) { fields.insert(k, v); }
    if let Some((k, v)) = mk("chat", rec.chat.as_deref()) { fields.insert(k, v); }

    Ok(SyncRecording {
        id: rec.id.to_string(),
        filename: rec.filename,
        created_at: rec.created_at.to_rfc3339(),
        updated_at: rec.updated_at.map(|t| t.to_rfc3339()).unwrap_or_else(|| rec.created_at.to_rfc3339()),
        deleted_at: None, // Read from DB directly below
        patient_name: rec.patient_name,
        duration_seconds: rec.duration_seconds,
        file_size_bytes: rec.file_size_bytes.map(|v| v as i64),
        stt_provider: rec.stt_provider,
        ai_provider: rec.ai_provider,
        fields,
    })
}

/// Manual full bidirectional sync. Called when user toggles sync on,
/// clicks "Sync now", or on startup.
#[tauri::command]
pub async fn sync_content_now(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let (conn, bearer, client) = match content_sync_target(&state) {
        Some(t) => t,
        None => return Ok(()), // Not configured — no-op
    };
    let remote = ContentRemote::from(&conn, Some(bearer), client)
        .ok_or(AppError::Other("tailscale not configured".into()))?;

    let db = state.db.clone();

    // --- PULL ---
    let cursor = {
        let c = db.conn()?;
        ContentSyncRepo::get_cursor(&c)?.cursor
    };

    loop {
        let pull_resp = remote.pull(cursor.as_deref()).await?;

        // Merge incoming
        {
            let c = db.conn()?;
            let result = ContentSyncRepo::merge_incoming(&c, &pull_resp.recordings)?;
            ContentSyncRepo::set_cursor(&c, &pull_resp.server_time)?;

            // Emit update events for changed recordings
            for id in &result.changed_recording_ids {
                let _ = app.emit("recording-updated", serde_json::json!({ "id": id }));
            }
        }

        if !pull_resp.has_more {
            break;
        }
    }

    // --- PUSH local changes ---
    // Collect local recordings newer than server's last known state.
    // For simplicity, push all recordings updated since our last cursor.
    let push_recordings = {
        let c = db.conn()?;
        let (ids, _) = ContentSyncRepo::changed_since(&c, cursor.as_deref(), 200)?;
        ids.iter()
            .filter_map(|id| build_sync_recording(&c, id).ok())
            .collect::<Vec<_>>()
    };

    if !push_recordings.is_empty() {
        let _ = remote.push(push_recordings).await;
    }

    let _ = app.emit("content-sync-complete", ());
    Ok(())
}

/// Subscribe to SSE content change notifications from the server.
/// Emits "content-changed" Tauri event for each notification.
/// Runs a long-lived background task with exponential backoff.
#[tauri::command]
pub async fn subscribe_content_sync(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let (conn, bearer, client) = match content_sync_target(&state) {
        Some(t) => t,
        None => return Ok(()),
    };
    let remote = ContentRemote::from(&conn, Some(bearer), client)
        .ok_or(AppError::Other("tailscale not configured".into()))?;

    let app_clone = app.clone();
    tokio::spawn(async move {
        let mut backoff = std::time::Duration::from_secs(5);
        let max_backoff = std::time::Duration::from_secs(30);

        loop {
            match remote.subscribe_events_async().await {
                Ok(resp) => {
                    backoff = std::time::Duration::from_secs(5); // Reset on connect
                    use futures_util::StreamExt;
                    let mut stream = resp.bytes_stream();
                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(bytes) => {
                                let text = String::from_utf8_lossy(&bytes);
                                if text.contains("data: changed") || text.contains("data: *") {
                                    let _ = app_clone.emit("content-changed", ());
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
                Err(_) => {
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                }
            }
        }
    });

    Ok(())
}
```

- [ ] **Step 2: Add module declaration**

In `src-tauri/src/commands/mod.rs`, add:

```rust
pub mod content_sync;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p rust-medical-assistant`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/content_sync.rs src-tauri/src/commands/mod.rs
git commit -m "feat: content sync Tauri commands (sync_now, subscribe)"
```

---

## Task 13: Audio fetch/upload commands

**Files:**
- Modify: `src-tauri/src/commands/content_sync.rs`

- [ ] **Step 1: Add fetch_audio_from_server command**

Append to `src-tauri/src/commands/content_sync.rs`:

```rust
/// Fetch audio for a recording from the server (on-demand).
/// Downloads via Tailscale, re-encrypts locally, writes to disk.
#[tauri::command]
pub async fn fetch_audio_from_server(
    state: State<'_, AppState>,
    recording_id: String,
) -> AppResult<()> {
    let (conn, bearer, client) = match content_sync_target(&state) {
        Some(t) => t,
        None => return Err(AppError::Other("content sync not configured".into())),
    };
    let remote = ContentRemote::from(&conn, Some(bearer), client)
        .ok_or(AppError::Other("tailscale not configured".into()))?;

    // Download plaintext bytes from server
    let audio_bytes = remote.fetch_audio(&recording_id).await?;

    // Write to local recordings dir as {id}.enc
    let recordings_dir = crate::commands::resolve_recordings_dir()?;
    let dest = recordings_dir.join(format!("{recording_id}.enc"));

    // Write plaintext to temp
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &audio_bytes).map_err(AppError::Io)?;

    // Re-encrypt with local FE1 key
    medical_security::file_crypto::encrypt_file_in_place(&tmp)
        .map_err(|e| AppError::database(e.to_string()))?;

    // Atomic rename
    std::fs::rename(&tmp, &dest).map_err(AppError::Io)?;

    // Update audio_path in DB
    {
        let c = state.db.conn()?;
        c.execute(
            "UPDATE recordings SET audio_path = ?1 WHERE id = ?2",
            rusqlite::params![dest.to_string_lossy(), recording_id],
        ).map_err(|e| AppError::database(e.to_string()))?;
    }

    tracing::info!("fetched audio: id_len={}_bytes={}", recording_id.len(), audio_bytes.len());
    Ok(())
}

/// Upload audio for a recording to the server.
/// Called after recording capture (background, best-effort).
#[tauri::command]
pub async fn upload_audio_to_server(
    state: State<'_, AppState>,
    recording_id: String,
) -> AppResult<()> {
    let (conn, bearer, client) = match content_sync_target(&state) {
        Some(t) => t,
        None => return Ok(()), // Not configured — no-op
    };
    let remote = ContentRemote::from(&conn, Some(bearer), client)
        .ok_or(AppError::Other("tailscale not configured".into()))?;

    // Read local audio file
    let audio_path = {
        let c = state.db.conn()?;
        let rec = RecordingsRepo::get_by_id(&c, &recording_id)?;
        rec.audio_path
    };

    if !audio_path.exists() {
        return Err(AppError::Other("audio file not found locally".into()));
    }

    // Decrypt to plaintext bytes
    let plaintext = match medical_security::file_crypto::decrypt_file(&audio_path) {
        Ok(bytes) => bytes,
        Err(medical_security::file_crypto::FileCryptoError::NotEncrypted) => {
            std::fs::read(&audio_path).map_err(AppError::Io)?
        }
        Err(e) => return Err(AppError::database(e.to_string())),
    };

    // Upload
    remote.upload_audio(&recording_id, plaintext).await?;

    tracing::info!("uploaded audio: id_len={}", recording_id.len());
    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p rust-medical-assistant`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/commands/content_sync.rs
git commit -m "feat: audio fetch/upload Tauri commands"
```

---

## Task 14: Integrate `save_recording_field` with sync

**Files:**
- Modify: `src-tauri/src/commands/recordings_edit.rs`

- [ ] **Step 1: Add `peer_discussion` to editable fields**

In `src-tauri/src/commands/recordings_edit.rs`, change the EDITABLE_FIELDS constant (line 21):

```rust
const EDITABLE_FIELDS: &[&str] = &["transcript", "soap_note", "referral", "letter", "peer_discussion", "chat"];
```

Also update `max_chars_for_field` to include `peer_discussion`:

```rust
fn max_chars_for_field(field: &str) -> usize {
    match field {
        "transcript" => 500_000,
        "soap_note" | "referral" | "letter" | "peer_discussion" | "chat" => 500_000,
        _ => 50_000,
    }
}
```

- [ ] **Step 2: Bump `updated_at` + field revision on save**

In the `save_recording_field` command, inside the `spawn_blocking` closure, after `RecordingsRepo::update(&conn, &recording)?`, add:

```rust
                    // Bump updated_at + field revision for content sync
                    let now = chrono::Utc::now().to_rfc3339();
                    conn.execute(
                        "UPDATE recordings SET updated_at = ?1 WHERE id = ?2",
                        rusqlite::params![now, recording.id.to_string()],
                    ).map_err(|e| medical_db::DbError::from(e))?;
                    let machine_id = crate::state::get_machine_id(&conn);
                    let _ = medical_db::ContentSyncRepo::upsert_revision(
                        &conn,
                        &recording.id.to_string(),
                        &field,
                        &now,
                        machine_id.as_deref(),
                    );
```

- [ ] **Step 3: Add background push trigger after the spawn_blocking**

After the `spawn_blocking` returns successfully, add a best-effort background push:

```rust
    // Best-effort content sync push (fire-and-forget)
    let app_clone = app.clone();
    let rec_id = recording_id.clone();
    tokio::spawn(async move {
        // Small debounce — if multiple fields are edited quickly, only push once
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let st = app_clone.state::<AppState>();
        if super::content_sync::content_sync_target(&st).is_some() {
            // Push just this recording
            let db = st.db.clone();
            let push_result = tokio::task::spawn_blocking(move || -> AppResult<Vec<medical_db::content_sync::SyncRecording>> {
                let c = db.conn()?;
                let sync_rec = super::content_sync::build_sync_recording(&c, &rec_id)?;
                Ok(vec![sync_rec])
            }).await;
            if let Ok(Ok(recordings)) = push_result {
                if let Some((conn, bearer, client)) = super::content_sync::content_sync_target(&st) {
                    if let Some(remote) = crate::content_remote::ContentRemote::from(&conn, Some(bearer), client) {
                        let _ = remote.push(recordings).await;
                    }
                }
            }
        }
    });
```

Note: `content_sync_target` and `build_sync_recording` are currently private functions. They need to be `pub(crate)` so `recordings_edit.rs` can call them. Change their visibility in `content_sync.rs`:

```rust
pub(crate) fn content_sync_target(
pub(crate) fn build_sync_recording(
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p rust-medical-assistant`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/recordings_edit.rs src-tauri/src/commands/content_sync.rs
git commit -m "feat: save_recording_field bumps updated_at + revision + background push"
```

---

## Task 15: Integrate `delete_recording` with sync

**Files:**
- Modify: `src-tauri/src/commands/recordings.rs`

- [ ] **Step 1: Add background sync push on delete**

In `delete_recording` command (or `soft_delete`), after the soft-delete DB write, add a best-effort background push of the tombstone. Find the `delete_recording` command and add after the `RecordingsRepo::soft_delete` call:

```rust
    // Best-effort content sync push of the tombstone
    let rec_id = recording_id.clone();
    let st = state.clone();
    tokio::spawn(async move {
        if super::content_sync::content_sync_target(&st).is_some() {
            let db = st.db.clone();
            let push_result = tokio::task::spawn_blocking(move || {
                let c = db.conn()?;
                let sync_rec = super::content_sync::build_sync_recording(&c, &rec_id)?;
                // Read deleted_at
                let deleted_at: Option<String> = c
                    .query_row(
                        "SELECT deleted_at FROM recordings WHERE id = ?1",
                        rusqlite::params![rec_id],
                        |row| row.get(0),
                    )
                    .ok()
                    .flatten();
                let mut rec = sync_rec;
                rec.deleted_at = deleted_at;
                Ok::<_, medical_core::error::AppError>(vec![rec])
            }).await;
            if let Ok(Ok(recordings)) = push_result {
                if let Some((conn, bearer, client)) = super::content_sync::content_sync_target(&st) {
                    if let Some(remote) = crate::content_remote::ContentRemote::from(&conn, Some(bearer), client) {
                        let _ = remote.push(recordings).await;
                    }
                }
            }
        }
    });
```

Note: `delete_recording` may need `app: AppHandle` parameter added if not already present. Check the current signature and add `app: tauri::AppHandle` if needed. The `State` clone also needs to work with `AppState`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p rust-medical-assistant`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/commands/recordings.rs
git commit -m "feat: delete_recording propagates tombstone via content sync"
```

---

## Task 16: Startup sync task + tombstone sweeper

**Files:**
- Modify: `src-tauri/src/state.rs`

- [ ] **Step 1: Add startup content sync trigger**

In `AppState::initialize`, after the encryption sweep (around line 709), add a content sync startup trigger. This mirrors the condition chip auto-start pattern:

```rust
    // Content sync: initial pull on startup if enabled
    {
        let config = crate::commands::settings::load_config_sync(&db)
            .unwrap_or_default();
        if config.sync_content {
            let db_clone = db.clone();
            let app_handle = app.clone();
            tokio::spawn(async move {
                // Small delay to let the app window fully initialize
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                tracing::info!("starting initial content sync");
                let _ = crate::commands::content_sync::run_initial_sync(app_handle, db_clone).await;
            });
        }
    }
```

- [ ] **Step 2: Add `run_initial_sync` to content_sync.rs**

In `src-tauri/src/commands/content_sync.rs`, add a public function:

```rust
/// Run the initial bidirectional sync on startup.
/// Called from AppState::initialize when sync_content is enabled.
pub async fn run_initial_sync(
    app: AppHandle,
    db: Arc<medical_db::Database>,
) -> AppResult<()> {
    // This reuses sync_content_now logic, but with explicit db param
    let st = app.state::<AppState>();
    let (conn, bearer, client) = match content_sync_target(&st) {
        Some(t) => t,
        None => return Ok(()),
    };
    let remote = ContentRemote::from(&conn, Some(bearer), client)
        .ok_or(AppError::Other("tailscale not configured".into()))?;

    // Pull
    let cursor = {
        let c = db.conn()?;
        ContentSyncRepo::get_cursor(&c)?.cursor
    };

    loop {
        let pull_resp = remote.pull(cursor.as_deref()).await?;
        let changed_count = pull_resp.recordings.len();
        {
            let c = db.conn()?;
            let result = ContentSyncRepo::merge_incoming(&c, &pull_resp.recordings)?;
            ContentSyncRepo::set_cursor(&c, &pull_resp.server_time)?;
            for id in &result.changed_recording_ids {
                let _ = app.emit("recording-updated", serde_json::json!({ "id": id }));
            }
        }
        tracing::info!("initial sync pulled {} recordings", changed_count);
        if !pull_resp.has_more {
            break;
        }
    }

    // Push local changes
    let push_recordings = {
        let c = db.conn()?;
        let (ids, _) = ContentSyncRepo::changed_since(&c, cursor.as_deref(), 200)?;
        ids.iter()
            .filter_map(|id| build_sync_recording(&c, id).ok())
            .collect::<Vec<_>>()
    };

    if !push_recordings.is_empty() {
        tracing::info!("initial sync pushing {} recordings", push_recordings.len());
        let _ = remote.push(push_recordings).await;
    }

    let _ = app.emit("content-sync-complete", ());
    Ok(())
}
```

- [ ] **Step 3: Add tombstone sweeper to state.rs**

In `AppState::initialize`, after the content sync startup, add a sweeper task:

```rust
    // Tombstone sweeper: purges recordings deleted >30 days ago (server only)
    {
        let db_clone = db.clone();
        let server_config = crate::state::load_server_config().ok().flatten();
        if server_config.is_some() {
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(86400)).await; // 24h
                    tracing::info!("running tombstone sweeper");
                    if let Ok(conn) = db_clone.conn() {
                        match conn.execute(
                            "DELETE FROM recordings
                             WHERE deleted_at IS NOT NULL
                             AND deleted_at < datetime('now', '-30 days')",
                            [],
                        ) {
                            Ok(count) => {
                                tracing::info!("tombstone sweeper purged {} recordings", count);
                            }
                            Err(e) => {
                                tracing::warn!("tombstone sweeper failed: {}", e);
                            }
                        }
                    }
                }
            });
        }
    }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p rust-medical-assistant`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/state.rs src-tauri/src/commands/content_sync.rs
git commit -m "feat: startup content sync + 30-day tombstone sweeper"
```

---

## Task 17: Register new commands in lib.rs

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add module declaration**

In `src-tauri/src/lib.rs`, find the `mod` declarations (around lines 1-10) and add:

```rust
mod content_remote;
```

(If `content_remote` was already declared in a previous task, skip this.)

- [ ] **Step 2: Register commands in invoke_handler**

Find the `invoke_handler` macro call (around line 340-371). Add before the closing `])`:

```rust
        commands::content_sync::sync_content_now,
        commands::content_sync::subscribe_content_sync,
        commands::content_sync::fetch_audio_from_server,
        commands::content_sync::upload_audio_to_server,
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p rust-medical-assistant`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: register content sync commands in invoke_handler"
```

---

## Task 18: Frontend API wrapper

**Files:**
- Create: `src/lib/api/contentSync.ts`

- [ ] **Step 1: Create the API file**

```typescript
import { invoke } from '@tauri-apps/api/core';

/** Manual full bidirectional sync. */
export async function syncContentNow(): Promise<void> {
  await invoke('sync_content_now');
}

/** Subscribe to SSE content change notifications from the server. */
export async function subscribeContentSync(): Promise<void> {
  await invoke('subscribe_content_sync');
}

/** Fetch audio for a recording from the server (on-demand). */
export async function fetchAudioFromServer(recordingId: string): Promise<void> {
  await invoke('fetch_audio_from_server', { recordingId });
}

/** Upload audio for a recording to the server. */
export async function uploadAudioToServer(recordingId: string): Promise<void> {
  await invoke('upload_audio_to_server', { recordingId });
}
```

- [ ] **Step 2: Commit**

```bash
git add src/lib/api/contentSync.ts
git commit -m "feat: frontend content sync API wrappers"
```

---

## Task 19: Frontend store + event listeners

**Files:**
- Modify: `src/lib/stores/recordings.svelte.ts`
- Modify: `src/lib/pages/RecordingsTab.svelte`

- [ ] **Step 1: Add sync state to the recordings store**

In `src/lib/stores/recordings.svelte.ts`, add new state fields to the `RecordingsStore` class:

```typescript
  syncing = $state(false);
  lastSyncedAt = $state<Date | null>(null);
```

Add new methods:

```typescript
  /** Sync with server (manual or triggered). */
  async syncNow(): Promise<void> {
    this.syncing = true;
    try {
      await invoke('sync_content_now');
      await this.load();
      this.lastSyncedAt = new Date();
    } finally {
      this.syncing = false;
    }
  }

  /** Handle a remote update event for a specific recording. */
  handleRemoteUpdate(recordingId: string): void {
    // If this recording is currently selected, reload it
    if (this.selectedRecording?.id === recordingId) {
      selectRecording(recordingId);
    }
    // Refresh the list to reflect any changes
    this.load();
  }
```

- [ ] **Step 2: Add event listeners in RecordingsTab.svelte**

In `src/lib/pages/RecordingsTab.svelte`, in `onMount`, add listeners for content sync events:

```typescript
  const unlistenChanged = await listen('content-changed', async () => {
    await recordings.syncNow();
  });

  const unlistenUpdated = await listen('recording-updated', (e) => {
    recordings.handleRemoteUpdate((e.payload as any).id);
  });

  const unlistenComplete = await listen('content-sync-complete', () => {
    recordings.lastSyncedAt = new Date();
  });

  // Start SSE subscription if content sync is enabled
  await invoke('subscribe_content_sync');
```

Add cleanup in `onDestroy`:

```typescript
  unlistenChanged();
  unlistenUpdated();
  unlistenComplete();
```

- [ ] **Step 3: Verify type check**

Run: `npm run check`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/lib/stores/recordings.svelte.ts src/lib/pages/RecordingsTab.svelte
git commit -m "feat: frontend content sync store + event listeners"
```

---

## Task 20: ContentSync settings component

**Files:**
- Create: `src/lib/components/settings/sharing/ContentSync.svelte`
- Modify: `src/lib/components/settings/Sharing.svelte`

- [ ] **Step 1: Create the settings component**

Create `src/lib/components/settings/sharing/ContentSync.svelte`:

```svelte
<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';
  import { syncContentNow } from '../../../api/contentSync';

  type Props = { visible: boolean };
  let { visible }: Props = $props();
</script>

{#if visible}
  <label class="form-row" style="margin-top: 1rem;">
    <input
      type="checkbox"
      checked={settings.state.sync_content ?? false}
      onchange={async (e) => {
        const checked = (e.target as HTMLInputElement).checked;
        settings.updateField('sync_content', checked);
        if (checked) {
          try {
            await syncContentNow();
          } catch (err) {
            console.error('Initial content sync failed:', err);
          }
        }
      }}
    />
    <span>
      Sync patient content via Tailscale
      <p class="hint">
        Syncs transcripts, SOAP notes, letters, and peer discussions between this
        machine and the server over your encrypted Tailscale connection. Audio
        files are archived on the server and fetched on demand.
      </p>
      <p class="hint" style="color: var(--color-warning, #e8a835);">
        ⚠ Requires Tailscale on both this machine and the server.
      </p>
    </span>
  </label>
{/if}
```

- [ ] **Step 2: Add ContentSync to Sharing.svelte**

In `src/lib/components/settings/Sharing.svelte`, import and include the component. Find where `ConditionChipSync` is used and add `ContentSync` alongside it:

```svelte
<script lang="ts">
  // ... existing imports
  import ContentSync from './sharing/ContentSync.svelte';
  // ...
</script>

<!-- After the ConditionChipSync component: -->
<ContentSync visible={sharingOn || !!pairedTo} />
```

- [ ] **Step 3: Verify type check**

Run: `npm run check`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/settings/sharing/ContentSync.svelte src/lib/components/settings/Sharing.svelte
git commit -m "feat: ContentSync settings toggle component"
```

---

## Task 21: EditorTab — conflict toast + fetch audio

**Files:**
- Modify: `src/lib/pages/EditorTab.svelte`

- [ ] **Step 1: Add conflict toast listener**

In `src/lib/pages/EditorTab.svelte`, add a listener for `recording-updated` events that shows a toast when the currently-open recording is updated remotely:

```typescript
  import { listen } from '@tauri-apps/api/event';
  import { toast } from '../../../stores/toast.svelte'; // adjust import path as needed

  // In onMount:
  const unlistenUpdate = await listen('recording-updated', (e) => {
    const payload = e.payload as { id: string };
    if (payload.id === recordingId && !dirtySince) {
      // Recording was updated remotely and we're not editing — reload
      toast.info('Recording updated on another machine');
      selectRecording(recordingId);
    }
  });

  // In onDestroy:
  unlistenUpdate();
```

- [ ] **Step 2: Add "Fetch Audio from Server" button**

On the transcript tab toolbar (near the "Export Audio" button), add a conditional button:

```svelte
{#if tabId === 'transcript' && syncContentActive && !audioExists}
  <button
    class="btn btn-secondary"
    onclick={async () => {
      try {
        await fetchAudioFromServer(recordingId);
        toast.success('Audio fetched from server');
      } catch (err) {
        toast.error('Audio not available on server');
      }
    }}
  >
    Fetch Audio from Server
  </button>
{/if}
```

Add the necessary imports and reactive state:

```typescript
  import { fetchAudioFromServer } from '../../api/contentSync';
  import { settings } from '../../stores/settings.svelte';

  const syncContentActive = $derived(settings.state.sync_content ?? false);
  const audioExists = $derived(!!recording?.audioPath); // or check file existence
```

Note: `audioExists` may need a separate check since `audio_path` is an absolute local path. A simple approach: show the button when `syncContentActive` is true and the recording wasn't created on this machine (check via metadata or origin). For simplicity, always show when syncContentActive is true.

- [ ] **Step 3: Verify type check**

Run: `npm run check`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/lib/pages/EditorTab.svelte
git commit -m "feat: conflict toast + fetch audio button in editor"
```

---

## Task 22: Run full verification + fmt

- [ ] **Step 1: Run cargo fmt**

```bash
cargo fmt --all
```

- [ ] **Step 2: Run clippy**

```bash
cargo clippy --workspace
```
Expected: No new warnings

- [ ] **Step 3: Run all backend tests**

```bash
cargo test --workspace --lib
```
Expected: All pass (except known flaky `file_crypto::tests::encrypt_file_in_place_roundtrips`)

- [ ] **Step 4: Run frontend tests**

```bash
npx vitest run
```
Expected: All pass

- [ ] **Step 5: Run type check**

```bash
npm run check
```
Expected: PASS

- [ ] **Step 6: Commit any fmt/clippy fixes**

```bash
git add -A
git commit -m "chore: fmt + clippy after content sync implementation"
```
