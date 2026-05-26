# Error Handling Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate all unwrap/expect calls in critical-path files (generations.rs, orchestrator.rs, recordings.rs, vectors.rs, key_storage.rs) and replace with proper error propagation.

**Architecture:** Enhance existing `DbError` and `AppError` types with new variants where needed, then systematically replace unwrap/expect with `?` operator and `.map_err()` for context enrichment. Use TDD approach with tests for error paths.

**Tech Stack:** Rust, thiserror, rusqlite, Tauri

---

## File Structure

### Modified Files:
- `crates/db/src/generations.rs` — Fix 2 unwrap_or_else calls in UUID parsing (lines 108-109)
- `crates/sharing/src/orchestrator.rs` — Fix unwrap calls in device pairing logic
- `crates/db/src/recordings.rs` — Fix unwrap calls in consultation storage
- `crates/db/src/vectors.rs` — Fix unwrap calls in RAG embeddings
- `crates/security/src/key_storage.rs` — Fix unwrap calls in encryption key management
- `crates/db/src/lib.rs` — Add new DbError variants if needed for UUID parsing errors

### Test Files:
- `crates/db/src/generations.rs` (inline tests module) — Add tests for error paths
- Similar for other files

---

## Task 1: Fix UUID parsing in generations.rs

**Files:**
- Modify: `crates/db/src/generations.rs:108-109`
- Test: `crates/db/src/generations.rs` (inline tests)

- [ ] **Step 1: Write failing test for invalid UUID handling**

Add test to `crates/db/src/generations.rs` in the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn row_to_generation_returns_error_on_invalid_uuid() {
    let conn = migrated();
    let rec_id = Uuid::new_v4();
    conn.execute(
        "INSERT INTO recordings (id, filename, processing_status, created_at) \
         VALUES (?, 'test.wav', 'done', datetime('now'))",
        params![rec_id.to_string()],
    )
    .unwrap();
    
    // Insert a row with an invalid UUID format
    let invalid_uuid = "not-a-valid-uuid";
    conn.execute(
        "INSERT INTO generations
           (id, recording_id, output_type, created_at, ai_provider, ai_model,
            input_transcript, draft_text, corpus_status, regeneration_seq)
         VALUES (?, ?, 'soap', datetime('now'), 'ollama', 'llama3',
                 'transcript', 'draft', 'candidate', 1)",
        params![invalid_uuid, rec_id.to_string()],
    )
    .unwrap();
    
    // Attempt to retrieve should return error, not silently use nil UUID
    let result = conn.query_row(
        "SELECT id, recording_id, output_type, created_at, finalized_at,
                ai_provider, ai_model, prompt_template_name,
                input_transcript, input_context_json,
                draft_text, final_text,
                corpus_status, corpus_curated_at,
                edit_distance, edit_ratio, regeneration_seq
         FROM generations WHERE id = ?",
        params![invalid_uuid],
        GenerationsRepo::row_to_generation,
    );
    
    // Should error due to invalid UUID, not return Ok with Uuid::nil()
    assert!(result.is_err() || matches!(result, Ok(Generation { id, .. }) if id != Uuid::nil()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p medical-db --lib generations::tests::row_to_generation_returns_error_on_invalid_uuid`

Expected: FAIL — test expects error but gets `Ok(Generation { id: Uuid::nil(), ... })`

- [ ] **Step 3: Add UuidParse error variant to DbError**

Modify `crates/db/src/lib.rs` in the `DbError` enum:

```rust
#[derive(Error, Debug)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Pool error: {0}")]
    Pool(#[from] r2d2::Error),
    #[error("Migration error: {0}")]
    Migration(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Constraint violation: {0}")]
    Constraint(String),
    #[error("Graph error: {0}")]
    Graph(String),
    #[error("UUID parse error in {field}: {0}")]
    UuidParse(String, String),  // NEW: (error, field_name)
    #[error("{0}")]
    Other(String),
}
```

- [ ] **Step 4: Fix row_to_generation to propagate UUID errors**

Modify `crates/db/src/generations.rs` lines 106-126:

```rust
fn row_to_generation(row: &rusqlite::Row) -> rusqlite::Result<Generation> {
    let id_str: String = row.get(0)?;
    let id = Uuid::parse_str(&id_str)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(e),
        ))?;
    
    let recording_id_str: String = row.get(1)?;
    let recording_id = Uuid::parse_str(&recording_id_str)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(e),
        ))?;
    
    Ok(Generation {
        id,
        recording_id,
        output_type: row.get(2)?,
        created_at: row.get(3)?,
        finalized_at: row.get(4)?,
        ai_provider: row.get(5)?,
        ai_model: row.get(6)?,
        prompt_template_name: row.get(7)?,
        input_transcript: row.get(8)?,
        input_context_json: row.get(9)?,
        draft_text: row.get(10)?,
        final_text: row.get(11)?,
        corpus_status: row.get(12)?,
        corpus_curated_at: row.get(13)?,
        edit_distance: row.get(14)?,
        edit_ratio: row.get(15)?,
        regeneration_seq: row.get(16)?,
    })
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p medical-db --lib generations::tests::row_to_generation_returns_error_on_invalid_uuid`

Expected: PASS

- [ ] **Step 6: Run all generations tests to ensure no regressions**

Run: `cargo test -p medical-db --lib generations`

Expected: All tests PASS

- [ ] **Step 7: Commit**

```bash
git add crates/db/src/lib.rs crates/db/src/generations.rs
git commit -m "refactor(db): propagate UUID parse errors in generations.rs

Replace silent UUID::nil() fallback with proper error propagation.
Invalid UUIDs in database now return rusqlite::Error instead of
silently converting to nil UUID.

Adds test to verify error handling for corrupted UUID data.

Part of error handling refactor (generations.rs: 2 unwrap calls fixed)"
```

---

## Task 2: Audit and fix remaining unwrap calls in generations.rs

**Files:**
- Modify: `crates/db/src/generations.rs:66` (unwrap_or on query_row)

- [ ] **Step 1: Analyze line 66 unwrap_or(0) pattern**

Read the context around line 66:

```rust
let prev_max: i64 = conn
    .query_row(
        "SELECT COALESCE(MAX(regeneration_seq), 0) FROM generations \
         WHERE recording_id = ? AND output_type = ?",
        params![input.recording_id.to_string(), input.output_type],
        |r| r.get(0),
    )
    .unwrap_or(0);
```

This is actually **correct behavior** — the SQL uses `COALESCE(..., 0)` to ensure a row is always returned, so `unwrap_or(0)` is defensive programming for the case where the query fails (e.g., connection lost). However, we should still propagate real errors.

- [ ] **Step 2: Write test for query failure scenario**

Add test to verify error propagation when connection is closed:

```rust
#[test]
fn record_generation_propagates_query_errors() {
    let conn = migrated();
    let rec_id = Uuid::new_v4();
    conn.execute(
        "INSERT INTO recordings (id, filename, processing_status, created_at) \
         VALUES (?, 'test.wav', 'done', datetime('now'))",
        params![rec_id.to_string()],
    )
    .unwrap();
    
    // Drop the table to simulate query failure
    conn.execute("DROP TABLE generations", []).unwrap();
    
    let result = GenerationsRepo::record_generation(
        &conn,
        GenerationInsert {
            recording_id: rec_id,
            output_type: "soap",
            ai_provider: "ollama",
            ai_model: "llama3",
            prompt_template_name: None,
            input_transcript: "t",
            input_context_json: None,
            draft_text: "d",
        },
    );
    
    assert!(result.is_err(), "should propagate query error, not silently use 0");
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p medical-db --lib generations::tests::record_generation_propagates_query_errors`

Expected: FAIL — currently panics or silently uses 0

- [ ] **Step 4: Replace unwrap_or(0) with proper error handling**

Modify `crates/db/src/generations.rs` lines 59-67:

```rust
let prev_max: i64 = conn
    .query_row(
        "SELECT COALESCE(MAX(regeneration_seq), 0) FROM generations \
         WHERE recording_id = ? AND output_type = ?",
        params![input.recording_id.to_string(), input.output_type],
        |r| r.get(0),
    )?;  // Propagate query errors
let seq = prev_max + 1;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p medical-db --lib generations::tests::record_generation_propagates_query_errors`

Expected: PASS

- [ ] **Step 6: Run all generations tests**

Run: `cargo test -p medical-db --lib generations`

Expected: All tests PASS

- [ ] **Step 7: Commit**

```bash
git add crates/db/src/generations.rs
git commit -m "refactor(db): propagate query errors in record_generation

Replace unwrap_or(0) with ? operator to propagate database errors
instead of silently defaulting to sequence 0.

Adds test for query failure scenario (table dropped).

Part of error handling refactor (generations.rs: 1 unwrap call fixed)"
```

---

## Task 3: Run clippy and fix warnings in generations.rs

**Files:**
- Modify: `crates/db/src/generations.rs` (as needed)

- [ ] **Step 1: Run clippy on db crate**

Run: `cargo clippy -p medical-db --lib -- -D warnings`

Expected: May show warnings about error handling patterns

- [ ] **Step 2: Fix any clippy warnings**

Address warnings such as:
- Unnecessary `.unwrap_or()` when `?` would work
- Error type conversions
- Unused imports

- [ ] **Step 3: Verify clippy passes**

Run: `cargo clippy -p medical-db --lib -- -D warnings`

Expected: No warnings

- [ ] **Step 4: Commit**

```bash
git add crates/db/src/generations.rs
git commit -m "style(db): address clippy warnings in generations.rs

Part of error handling refactor"
```

---

## Task 4: Verify generations.rs has zero unwrap/expect calls in production code

**Files:**
- Check: `crates/db/src/generations.rs`

- [ ] **Step 1: Count unwrap/expect calls in production code**

Run: `grep -n "\.unwrap()\|\.expect(" crates/db/src/generations.rs | grep -v "#\[cfg(test)\]" | grep -v "mod tests"`

Expected: Should show only test code unwraps, not production code

- [ ] **Step 2: Verify no unwrap in impl GenerationsRepo block**

Run: `awk '/^impl GenerationsRepo/,/^}/' crates/db/src/generations.rs | grep -n "\.unwrap()\|\.expect("`

Expected: No output (zero matches)

- [ ] **Step 3: Document completion**

Add comment to top of generations.rs:

```rust
//! Repository for `generations` (training-corpus capture table).
//!
//! See docs/superpowers/specs/2026-05-11-training-corpus-design.md.
//! Personal use only; data never leaves the device unless the
//! clinician explicitly exports via the (Phase 3) pipeline.
//!
//! **Error handling:** All methods use proper error propagation via `?`
//! and return `DbResult<T>`. No unwrap/expect calls in production code.
```

- [ ] **Step 4: Commit**

```bash
git add crates/db/src/generations.rs
git commit -m "docs(db): mark generations.rs as error-handling complete

Verified zero unwrap/expect calls in production code.
All database operations now propagate errors properly.

Part of error handling refactor - generations.rs COMPLETE (3 unwrap calls fixed)"
```

---

---

## Task 5: Refactor orchestrator.rs (sharing crate)

**Files:**
- Modify: `crates/sharing/src/orchestrator.rs`
- Test: `crates/sharing/src/orchestrator.rs` (inline tests)

Follow the same TDD pattern as Tasks 1-4:

- [ ] **Step 1: Audit unwrap calls in orchestrator.rs**

Run: `grep -n "\.unwrap()\|\.expect(" crates/sharing/src/orchestrator.rs`

Identify all unwrap/expect calls and categorize them:
- Test code (acceptable)
- Production code (must fix)

- [ ] **Step 2: Write failing tests for each error path**

For each unwrap call in production code, write a test that:
- Triggers the error condition
- Expects a proper error return, not a panic
- Verifies error context is preserved

- [ ] **Step 3: Replace unwrap/expect with proper error handling**

Use these patterns:
- `.unwrap()` → `?` with `.map_err()` for context
- `.expect("message")` → `.map_err(|e| Error::new(format!("message: {}", e)))?`
- `.unwrap_or(default)` → `.unwrap_or_else(|e| { tracing::warn!(%e); default })` if intentional, or `?` if error should propagate

- [ ] **Step 4: Add error context at crate boundaries**

When errors cross from orchestrator to other crates, add context:

```rust
let result = some_db_operation()
    .map_err(|e| SharingError::Database(format!("orchestrator operation failed: {}", e)))?;
```

- [ ] **Step 5: Run tests to verify no regressions**

Run: `cargo test -p sharing`

Expected: All tests PASS

- [ ] **Step 6: Run clippy**

Run: `cargo clippy -p sharing -- -D warnings`

Expected: No warnings

- [ ] **Step 7: Commit**

```bash
git add crates/sharing/src/orchestrator.rs
git commit -m "refactor(sharing): replace unwrap calls in orchestrator.rs

Replace all unwrap/expect calls with proper error propagation.
Add tests for error paths and verify error context is preserved.

Part of error handling refactor (orchestrator.rs: N unwrap calls fixed)"
```

---

## Task 6: Refactor recordings.rs (db crate)

**Files:**
- Modify: `crates/db/src/recordings.rs`
- Test: `crates/db/src/recordings.rs` (inline tests)

Follow the same TDD pattern as Tasks 1-4:

- [ ] **Step 1: Audit unwrap calls in recordings.rs**

Run: `grep -n "\.unwrap()\|\.expect(" crates/db/src/recordings.rs`

- [ ] **Step 2: Write failing tests for each error path**

Focus on:
- Missing recording IDs
- Invalid foreign key references
- Constraint violations
- Corrupted data scenarios

- [ ] **Step 3: Replace unwrap/expect with proper error handling**

- [ ] **Step 4: Add error context at crate boundaries**

- [ ] **Step 5: Run tests**

Run: `cargo test -p medical-db --lib recordings`

Expected: All tests PASS

- [ ] **Step 6: Run clippy**

Run: `cargo clippy -p medical-db --lib -- -D warnings`

Expected: No warnings

- [ ] **Step 7: Commit**

```bash
git add crates/db/src/recordings.rs
git commit -m "refactor(db): replace unwrap calls in recordings.rs

Part of error handling refactor (recordings.rs: N unwrap calls fixed)"
```

---

## Task 7: Refactor vectors.rs (db crate)

**Files:**
- Modify: `crates/db/src/vectors.rs`
- Test: `crates/db/src/vectors.rs` (inline tests)

Follow the same TDD pattern:

- [ ] **Step 1: Audit unwrap calls**

Run: `grep -n "\.unwrap()\|\.expect(" crates/db/src/vectors.rs`

- [ ] **Step 2: Write failing tests for error paths**

Focus on:
- Embedding failures
- Vector dimension mismatches
- Database constraint violations

- [ ] **Step 3: Replace unwrap/expect with proper error handling**

- [ ] **Step 4: Add error context**

- [ ] **Step 5: Run tests**

Run: `cargo test -p medical-db --lib vectors`

- [ ] **Step 6: Run clippy**

Run: `cargo clippy -p medical-db --lib -- -D warnings`

- [ ] **Step 7: Commit**

```bash
git add crates/db/src/vectors.rs
git commit -m "refactor(db): replace unwrap calls in vectors.rs

Part of error handling refactor (vectors.rs: N unwrap calls fixed)"
```

---

## Task 8: Refactor key_storage.rs (security crate)

**Files:**
- Modify: `crates/security/src/key_storage.rs`
- Test: `crates/security/src/key_storage.rs` (inline tests)

Follow the same TDD pattern:

- [ ] **Step 1: Audit unwrap calls**

Run: `grep -n "\.unwrap()\|\.expect(" crates/security/src/key_storage.rs`

- [ ] **Step 2: Write failing tests for error paths**

Focus on:
- Missing key files
- Corrupted key data
- Permission errors
- Key rotation failures

- [ ] **Step 3: Replace unwrap/expect with proper error handling**

- [ ] **Step 4: Add error context**

- [ ] **Step 5: Run tests**

Run: `cargo test -p security --lib key_storage`

- [ ] **Step 6: Run clippy**

Run: `cargo clippy -p security --lib -- -D warnings`

- [ ] **Step 7: Commit**

```bash
git add crates/security/src/key_storage.rs
git commit -m "refactor(security): replace unwrap calls in key_storage.rs

Part of error handling refactor (key_storage.rs: N unwrap calls fixed)"
```

---

## Task 9: Integration testing and validation

**Files:**
- Create: `tests/error_handling_integration.rs`
- Modify: Tauri commands as needed

- [ ] **Step 1: Verify zero unwrap/expect in production code**

Run for each refactored file:
```bash
awk '/^impl /,/^}/' crates/db/src/generations.rs | grep -c "\.unwrap()\|\.expect("
awk '/^impl /,/^}/' crates/sharing/src/orchestrator.rs | grep -c "\.unwrap()\|\.expect("
awk '/^impl /,/^}/' crates/db/src/recordings.rs | grep -c "\.unwrap()\|\.expect("
awk '/^impl /,/^}/' crates/db/src/vectors.rs | grep -c "\.unwrap()\|\.expect("
awk '/^impl /,/^}/' crates/security/src/key_storage.rs | grep -c "\.unwrap()\|\.expect("
```

Expected: All return 0

- [ ] **Step 2: Run full test suite**

Run: `cargo test --workspace`

Expected: All tests PASS

- [ ] **Step 3: Run clippy on entire workspace**

Run: `cargo clippy --workspace -- -D warnings`

Expected: No warnings

- [ ] **Step 4: Manual UI testing**

Test error scenarios in the running app:
- Trigger database errors (corrupt test DB, missing files)
- Verify frontend shows user-friendly error messages
- Confirm no crashes/panics
- Check that error logs don't contain PHI

- [ ] **Step 5: Document error handling patterns**

Update `docs/error-handling.md` (create if needed) with:
- Error type hierarchy (AppError, DbError, crate-specific errors)
- When to use each pattern (`.map_err()`, `?`, `ErrorContext`)
- Examples of good error handling
- Anti-patterns to avoid

- [ ] **Step 6: Create PR**

```bash
git checkout -b refactor/error-handling-critical-paths
git push origin refactor/error-handling-critical-paths
gh pr create --title "refactor: error handling in critical-path files" \
  --body "Systematic replacement of unwrap/expect calls in 5 critical files..."
```

---

## Summary

**Total scope:**
- 5 files refactored (generations.rs, orchestrator.rs, recordings.rs, vectors.rs, key_storage.rs)
- ~184 unwrap/expect calls eliminated
- 15+ new tests for error paths
- Zero runtime panics in critical paths

**Success metrics:**
- ✅ All refactored files have zero unwrap/expect in production code
- ✅ All tests pass (existing + new error path tests)
- ✅ Clippy passes with `-D warnings`
- ✅ Manual testing confirms graceful degradation
- ✅ Frontend shows user-friendly error messages

**Risk mitigation:**
- Staged commits (one per file) for easier review
- TDD approach ensures error paths are tested
- No breaking changes to public APIs (error types enhanced, not replaced)
- PHI compliance maintained (no patient data in error messages)
