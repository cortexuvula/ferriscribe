# Item #4: Async Encryption — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make `stop_recording` return immediately by encrypting the WAV in a background task instead of blocking the command.

**Architecture:** Add `encryption_pending` column to `recordings`. `stop_recording` inserts with `encryption_pending = true`, spawns background encryption, returns. The reader already handles both encrypted and plaintext. A startup sweep encrypts any files left pending by a crash.

**Tech Stack:** Rust (rusqlite, tokio, medical-security).

**Spec:** `docs/superpowers/specs/2026-07-08-high-priority-improvements-design.md` (Item #4)

---

## File Structure

### Modified files

| File | Change |
|------|--------|
| `crates/db/src/migrations/m012_encryption_pending.rs` | New migration |
| `crates/db/src/migrations/mod.rs` | Register m012 |
| `crates/db/src/recordings.rs` | Add `set_encryption_done` + `list_encryption_pending` methods |
| `src-tauri/src/commands/audio.rs` | Change `stop_recording` to spawn background encryption |
| `src-tauri/src/state.rs` or `src-tauri/src/lib.rs` | Add startup sweep for pending encryption |

---

## Task 1: Migration m012 + repo methods

**Files:**
- Create: `crates/db/src/migrations/m012_encryption_pending.rs`
- Modify: `crates/db/src/migrations/mod.rs`
- Modify: `crates/db/src/recordings.rs`

- [ ] **Step 1: Create migration m012**

Create `crates/db/src/migrations/m012_encryption_pending.rs`:

```rust
use rusqlite::Connection;

use crate::DbResult;

/// Add `encryption_pending` column to track whether a recording's WAV file
/// has been encrypted at rest yet.
///
/// When `stop_recording` spawns background encryption, the row is inserted
/// with `encryption_pending = true`. The background task flips it to false
/// (0) on success. A startup sweep encrypts any rows still pending (from a
/// crash between insert and encrypt). The reader checks the magic bytes
/// (FE1) regardless of this flag — it's for the sweep, not the reader.
pub fn up(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "ALTER TABLE recordings ADD COLUMN encryption_pending INTEGER NOT NULL DEFAULT 0;",
    )?;
    Ok(())
}
```

- [ ] **Step 2: Register m012**

In `crates/db/src/migrations/mod.rs`, add `pub mod m012_encryption_pending;` and add to `all_migrations()`:
```rust
        Migration { version: 12, name: "encryption_pending", up: m012_encryption_pending::up },
```

- [ ] **Step 3: Add repo methods**

In `crates/db/src/recordings.rs`, add:

```rust
    /// Mark a recording's encryption as complete (encryption_pending = 0).
    pub fn set_encryption_done(conn: &Connection, id: &Uuid) -> DbResult<()> {
        conn.execute(
            "UPDATE recordings SET encryption_pending = 0 WHERE id = ?1",
            [&id.to_string()],
        )?;
        Ok(())
    }

    /// Return audio paths for all recordings with encryption_pending = 1.
    /// Used by the startup sweep to encrypt files left pending by a crash.
    pub fn list_encryption_pending(conn: &Connection) -> DbResult<Vec<(Uuid, PathBuf)>> {
        let mut stmt = conn.prepare(
            "SELECT id, audio_path FROM recordings WHERE encryption_pending = 1",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let id_str: String = row.get(0)?;
                let path: String = row.get(1)?;
                Ok((
                    Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::nil()),
                    PathBuf::from(path),
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
```

- [ ] **Step 4: Run migration + repo tests**

Run: `cargo test -p medical-db --lib migrations`
Run: `cargo test -p medical-db --lib recordings`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/db/src/migrations/m012_encryption_pending.rs crates/db/src/migrations/mod.rs crates/db/src/recordings.rs
git commit -m "feat(db): m012 migration + repo methods for async encryption"
```

---

## Task 2: Change stop_recording to spawn background encryption

**Files:**
- Modify: `src-tauri/src/commands/audio.rs`

- [ ] **Step 1: Read the current stop_recording**

Read `src-tauri/src/commands/audio.rs` around lines 240-310 (the stop_recording function). Find:
- The encryption block (lines ~263-282) — this is what we're changing
- The DB insert (line ~303)
- The `recording_active = false` block (lines ~224-227)

- [ ] **Step 2: Replace inline encryption with background spawn**

Replace the encryption block (the `if file_size > 0 { ... }` that awaits spawn_blocking) with:

```rust
    // Spawn background encryption — don't block stop_recording.
    // The reader (open_recording_wav) handles both plaintext and encrypted
    // files, so the transcription pipeline works regardless of whether
    // encryption has finished. The atomic rename guarantees the reader
    // never sees a half-encrypted file.
    if file_size > 0 {
        let enc_path = current.wav_path.clone();
        let rec_id = recording_uuid;
        let db = Arc::clone(&state.db);
        tokio::task::spawn_blocking(move || {
            match medical_security::file_crypto::encrypt_file_in_place(&enc_path) {
                Ok(()) => {
                    tracing::debug!(path = %enc_path.display(), "Recording encrypted at rest (background)");
                    // Mark encryption as done in the DB.
                    if let Ok(conn) = db.conn() {
                        let _ = RecordingsRepo::set_encryption_done(&conn, &rec_id);
                    }
                }
                Err(medical_security::file_crypto::FileCryptoError::Keychain(e)) => {
                    tracing::warn!(error = %e, "Could not encrypt recording (keychain unavailable); storing plaintext");
                    // Still mark as done so the sweep doesn't retry pointlessly.
                    if let Ok(conn) = db.conn() {
                        let _ = RecordingsRepo::set_encryption_done(&conn, &rec_id);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %enc_path.display(), "Could not encrypt recording; storing plaintext");
                    if let Ok(conn) = db.conn() {
                        let _ = RecordingsRepo::set_encryption_done(&conn, &rec_id);
                    }
                }
            }
        });
        // NOT awaited — fire and forget.
    }
```

**IMPORTANT:** The `recording_uuid` must be computed BEFORE the DB insert (it's currently at line ~284). Move the `recording_uuid` computation above this block. Also, the `Recording` struct needs `encryption_pending` set — but since it's a DB column with DEFAULT 0, and we want it to be 1 for new recordings, we need to set it. Check if the `Recording` struct has this field or if we need a separate UPDATE after insert.

Actually, the simpler approach: after the DB insert, do a quick `UPDATE recordings SET encryption_pending = 1 WHERE id = ?`. Add this right after `RecordingsRepo::insert(&conn, &recording)?;`:

```rust
    // Mark encryption as pending — the background task will clear this.
    if file_size > 0 {
        conn.execute(
            "UPDATE recordings SET encryption_pending = 1 WHERE id = ?1",
            [&recording_uuid.to_string()],
        )?;
    }
```

This avoids changing the `Recording` struct (which would require touching the insert method and all its callers).

- [ ] **Step 3: Verify the recording_uuid is available**

The `recording_uuid` is computed at line ~284 (`Uuid::parse_str(&current.id)`). This must happen BEFORE the encryption spawn. Check the current order and move it if needed.

- [ ] **Step 4: Build to verify**

Run: `cargo build -p rust-medical-assistant 2>&1 | tail -20`
Expected: compiles. Fix any borrow issues (the `db` Arc is cloned into the spawned task).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/audio.rs
git commit -m "perf(audio): stop_recording returns immediately via background encryption

The WAV is encrypted in a fire-and-forget background task instead of
blocking the stop command. The reader already handles both plaintext
and encrypted files (checks FE1 magic), so transcription works
regardless of encryption state. A startup sweep handles crash recovery."
```

---

## Task 3: Add startup sweep for pending encryption

**Files:**
- Modify: `src-tauri/src/lib.rs` or `src-tauri/src/state.rs`

- [ ] **Step 1: Find the startup sequence**

Read `src-tauri/src/lib.rs` or `src-tauri/src/state.rs` to find where the app initializes after DB open + migrations. Look for where `fail_stuck_processing` is called (it's the existing startup sweeper).

- [ ] **Step 2: Add the encryption sweep**

Near the `fail_stuck_processing` call, add:

```rust
    // Sweep: encrypt any recordings left pending by a crash.
    {
        let conn = state.db.conn()?;
        let pending = RecordingsRepo::list_encryption_pending(&conn)?;
        if !pending.is_empty() {
            tracing::info!(count = pending.len(), "Encrypting pending recordings from previous session");
            for (id, path) in &pending {
                match medical_security::file_crypto::encrypt_file_in_place(path) {
                    Ok(()) => {
                        let _ = RecordingsRepo::set_encryption_done(&conn, id);
                        tracing::debug!(recording_id = %id, "Encrypted pending recording");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, recording_id = %id, "Failed to encrypt pending recording");
                    }
                }
            }
        }
    }
```

- [ ] **Step 3: Build + test**

Run: `cargo build -p rust-medical-assistant 2>&1 | tail -10`
Run: `cargo test --workspace --lib 2>&1 | tail -10`
Expected: compiles, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(audio): startup sweep for pending encryption (crash recovery)"
```

---

## Self-Review

### Spec coverage
- ✅ Migration m012 (encryption_pending column) — Task 1
- ✅ Repo methods (set_encryption_done, list_encryption_pending) — Task 1
- ✅ Background encryption in stop_recording — Task 2
- ✅ Startup sweep — Task 3
- ✅ Reader safety (existing encrypted/plaintext branching) — no change needed

### Known caveats
1. The `recording_uuid` must be computed before the encryption spawn — verify ordering in Task 2 Step 3.
2. The `conn.execute("UPDATE recordings SET encryption_pending = 1 ...")` is a raw SQL update to avoid changing the Recording struct. This is intentional to minimize blast radius.
3. The background encryption task borrows `db: Arc<Database>` (cloned), not the raw connection — safe for spawn_blocking.
4. The keychain-unavailable case still marks `encryption_done` to prevent the sweep from retrying pointlessly.
5. This is the LAST item implemented (after #2, #3, #1) because it touches the most sensitive code path.
