# Error Handling Refactor Design

**Date**: 2026-05-26  
**Status**: Approved  
**Timeline**: 2 weeks  
**Scope**: Critical paths (security, database, processing)

## Problem Statement

The codebase has **184 `unwrap()`/`expect()` calls** in critical-path files that can cause runtime panics:

- `crates/db/src/generations.rs` (50 calls) — Training data capture
- `crates/sharing/src/orchestrator.rs` (49 calls) — Device pairing
- `crates/db/src/recordings.rs` (31 calls) — Consultation storage
- `crates/db/src/vectors.rs` (29 calls) — RAG embeddings
- `crates/security/src/key_storage.rs` (25 calls) — Encryption keys

These panics violate the principle of graceful degradation and create reliability risks in production use.

## Current State Analysis

### Existing Error Architecture (Solid Foundation)

The codebase already has well-designed error types:

**`core::error::AppError`** — Comprehensive top-level error enum:
- Variants for each subsystem: `Database`, `Security`, `Audio`, `AiProvider`, `SttProvider`, `TtsProvider`, `Agent`, `Rag`, `Processing`, `Export`, `Translation`, `Config`
- Structured variants with context: `EndpointOffline { service, endpoint, reason, provider_name }`, `InvalidEndpoint { field, host, kind }`
- Serde serialization for frontend consumption
- `ErrorContext` struct for structured logging with severity levels

**`db::DbError`** — Database-specific error enum:
- `Sqlite(#[from] rusqlite::Error)`
- `Pool(#[from] r2d2::Error)`
- `Migration(String)`, `NotFound(String)`, `Constraint(String)`, `Graph(String)`, `Other(String)`

**All crates use `thiserror`** for ergonomic error definitions.

### The Real Problem

Code uses `unwrap()`/`expect()` instead of propagating these well-designed error types. Example:

```rust
let prev_max: i64 = conn
    .query_row("SELECT COALESCE(MAX(regeneration_seq), 0) FROM generations...", 
               params![...], 
               |r| r.get(0))
    .unwrap_or(0);  // ❌ Silently defaults to 0 on error
```

## Refined Design Approach

Instead of building new error types, we will:

1. **Enhance existing error types** — Add missing variants where unwrap calls hide specific failures
2. **Replace unwrap/expect systematically** — Use `?` operator and `.map_err()` for context
3. **Add error context** — Use `ErrorContext` to enrich errors with operation metadata
4. **Update Tauri commands** — Ensure errors flow to frontend as structured JSON

## Refactoring Patterns

### Pattern 1: Simple Propagation with `?`

**Before:**
```rust
let prev_max: i64 = conn
    .query_row(
        "SELECT COALESCE(MAX(regeneration_seq), 0) FROM generations 
         WHERE recording_id = ? AND output_type = ?",
        params![input.recording_id.to_string(), input.output_type],
        |r| r.get(0),
    )
    .unwrap_or(0);  // ❌ Silently defaults to 0 on error
```

**After:**
```rust
let prev_max: i64 = conn
    .query_row(
        "SELECT COALESCE(MAX(regeneration_seq), 0) FROM generations 
         WHERE recording_id = ? AND output_type = ?",
        params![input.recording_id.to_string(), input.output_type],
        |r| r.get(0),
    )
    .optional()?  // ✅ Propagate real errors
    .flatten()
    .unwrap_or(0);  // Only default when row doesn't exist
```

### Pattern 2: Error Context Enrichment

**Before:**
```rust
let key = load_encryption_key().expect("Failed to load key");
```

**After:**
```rust
let key = load_encryption_key()
    .map_err(|e| DbError::Other(format!("Encryption key load failed: {}", e)))?;
```

### Pattern 3: Structured Error Context (for logging)

**Before:**
```rust
let recording = db.get_recording(id).unwrap();
process_recording(recording);
```

**After:**
```rust
let recording = db.get_recording(id).map_err(|e| {
    tracing::error!(
        error = %e,
        recording_id = %id,
        operation = "load_recording",
        "Failed to load recording"
    );
    e
})?;
process_recording(recording)?;
```

## Tauri Integration

### Command Error Handling

Tauri commands already return `Result<T, AppError>`, so we ensure errors propagate:

```rust
#[tauri::command]
pub async fn get_generation(
    state: State<'_, AppState>,
    id: String,
) -> Result<Generation, AppError> {
    let id = Uuid::parse_str(&id)
        .map_err(|e| AppError::Database(format!("Invalid UUID: {}", e)))?;
    
    let generation = GenerationsRepo::get_by_id(&state.db, id)
        .map_err(|e| AppError::Database(e.to_string()))?;  // DbError → AppError
    
    Ok(generation)
}
```

### Frontend Error Handling

The frontend already handles structured errors via `AppError` serialization:

```typescript
try {
  const gen = await invoke<Generation>('get_generation', { id });
} catch (err: any) {
  if (err.kind === 'Database') {
    showErrorToast('Failed to load generation data');
  } else if (err.kind === 'EndpointOffline') {
    showEndpointOfflineDialog(err);
  }
}
```

## Implementation Timeline

### Week 1: Core Refactoring (5 files)

| Day | File | Focus | Tests Added |
|-----|------|-------|-------------|
| 1-2 | `crates/db/src/generations.rs` (50 unwrap) | Training data capture | Unit tests for error paths |
| 2-3 | `crates/sharing/src/orchestrator.rs` (49 unwrap) | Device pairing | Integration tests for connection failures |
| 3-4 | `crates/db/src/recordings.rs` (31 unwrap) | Consultation storage | Edge cases (missing recordings, corrupted data) |
| 4-5 | `crates/db/src/vectors.rs` (29 unwrap) | RAG embeddings | Embedding failures, DB constraint violations |
| 5 | `crates/security/src/key_storage.rs` (25 unwrap) | Encryption keys | Key rotation, missing keyfile scenarios |

### Week 2: Integration & Validation

- **Day 6-7**: Update Tauri commands to ensure all errors flow through `AppError`
- **Day 8-9**: Add integration tests for end-to-end error scenarios
- **Day 10**: Manual testing — trigger errors in UI, verify graceful degradation
- **Day 11-12**: Code review, documentation, PR

## Success Metrics

- ✅ Reduce unwrap/expect calls in target files from **184 → 0**
- ✅ Add **15+ new tests** covering error paths
- ✅ **Zero runtime panics** in critical paths (verified via `cargo test --no-run` + manual testing)
- ✅ Frontend shows **user-friendly errors** for all failure scenarios
- ✅ `cargo clippy --deny warnings` passes with no new warnings

## Key Principles

1. **Never panic in production code** — All unwrap/expect → `?` or `.map_err()`
2. **Add context at boundaries** — When crossing crate boundaries or in error handlers
3. **Preserve error chain** — Use `#[from]` and `.map_err()` to maintain cause
4. **Log at error sites** — Use `ErrorContext` for structured logging (no PHI!)
5. **Graceful degradation** — UI shows user-friendly messages, app continues

## Risk Mitigation

### If Week 1 runs long:
Cut `vectors.rs` (RAG is less critical than security/recordings)

### Process safeguards:
- Use git worktrees per CLAUDE.md convention
- Run `cargo clippy --deny warnings` after each file to catch new issues
- Staged commits (one per file) for easier review

### PHI compliance:
- Never log patient data in error messages
- Use IDs and operation names for context
- Review error messages for accidental PHI exposure

## Deliverables

1. **Refactored code** with proper error propagation in 5 target files
2. **New test files** for each refactored module (15+ tests)
3. **Updated error documentation** in `docs/error-handling.md`
4. **PR with staged commits** (one per file for easier review)
5. **Manual test report** documenting error scenarios verified in UI

## Testing Strategy

For each refactored file, add tests that verify:

- ✅ Happy path still works
- ✅ Error paths return correct `DbError`/`AppError` variants
- ✅ Error context includes operation metadata
- ✅ No panics on invalid inputs
- ✅ Frontend receives structured error JSON

### Example Test:

```rust
#[test]
fn get_generation_invalid_uuid_returns_database_error() {
    let db = test_db();
    let result = GenerationsRepo::get_by_id(&db, Uuid::nil());
    
    match result {
        Err(DbError::NotFound(_)) => (),  // Expected
        other => panic!("Expected NotFound, got {:?}", other),
    }
}
```

## Conclusion

This refactor enhances reliability without introducing new error types. By systematically replacing unwrap/expect calls with proper error propagation, we eliminate 184 potential panic sites while preserving the existing error architecture that already serves the application well.
