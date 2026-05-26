# Error Handling Patterns

This document describes the error handling architecture and patterns used in rustMedicalAssistant.

## Error Type Hierarchy

### Top-Level: `AppError` (crates/core/src/error.rs)

The main error type that flows to the Tauri frontend:

```rust
#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(String),
    
    #[error("{provider_name} at {endpoint} is offline ({reason:?})")]
    EndpointOffline {
        service: ServiceKind,
        endpoint: String,
        reason: OfflineReason,
        provider_name: String,
    },
    
    #[error("Security error: {0}")]
    Security(String),
    
    // ... other variants
}
```

**Key features:**
- Serde serialization for frontend consumption
- Structured variants with context (e.g., `EndpointOffline` includes service, endpoint, reason)
- `kind_str()` method for stable machine-readable discriminants

### Crate-Specific Errors

Each crate defines its own error type:

- `DbError` (crates/db/src/lib.rs) — Database operations
- `SecurityError` (crates/security/src/lib.rs) — Encryption, key storage
- `SharingError` (crates/sharing/src/lib.rs) — Device pairing, network operations
- `ProcessingError` (crates/processing/src/lib.rs) — SOAP generation, document processing
- `TtsError` (crates/tts-providers/src/lib.rs) — Text-to-speech providers

**Pattern:** Crate errors convert to `AppError` at Tauri command boundaries:

```rust
#[tauri::command]
pub async fn get_generation(
    state: State<'_, AppState>,
    id: String,
) -> Result<Generation, AppError> {
    let generation = GenerationsRepo::get_by_id(&state.db, id)
        .map_err(|e| AppError::Database(e.to_string()))?;  // DbError → AppError
    Ok(generation)
}
```

## When to Use Each Pattern

### Pattern 1: `?` Operator (Preferred)

Use when the error type already matches or has a `From` implementation:

```rust
let row = conn.query_row("SELECT ...", params![id], |r| r.get(0))?;
```

**Why:** Cleanest, most idiomatic Rust. Automatically converts and propagates errors.

### Pattern 2: `.map_err()` with Context

Use when you need to add context or convert error types:

```rust
let key = load_encryption_key()
    .map_err(|e| SecurityError::Other(format!("key load failed: {}", e)))?;
```

**Why:** Preserves error chain while adding operation-specific context.

### Pattern 3: Structured Error Context

Use for complex operations where you need rich logging:

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
```

**Why:** Enables structured logging without PHI exposure (no patient data in logs).

### Pattern 4: Default Values (Use Sparingly)

Only use when a default is genuinely acceptable:

```rust
let count: i64 = conn
    .query_row("SELECT COUNT(*) FROM ...", [], |r| r.get(0))
    .unwrap_or(0);  // OK: count defaults to 0 if query fails
```

**Why:** Silently defaulting can hide real errors. Use only when the default is semantically correct.

## Anti-Patterns to Avoid

### ❌ `unwrap()` in Production Code

```rust
// BAD: Panics on error
let row = conn.query_row("SELECT ...", [], |r| r.get(0)).unwrap();
```

**Problem:** Crashes the application on any error. No graceful degradation.

**Fix:** Use `?` or `.map_err()`.

### ❌ `expect()` Without Justification

```rust
// BAD: Assumes success without documentation
let key = load_key().expect("should work");
```

**Problem:** Panics with a message, but still crashes.

**Fix:** Either propagate the error, or document why it's infallible:

```rust
// GOOD: Documented infallible operation
let hex = format!("{:02x}", byte);  // Writing to String never fails
```

### ❌ Silently Converting Errors to Defaults

```rust
// BAD: Hides UUID corruption
let id = Uuid::parse_str(&str).unwrap_or(Uuid::nil());
```

**Problem:** Masks data corruption. Downstream code can't distinguish "no ID" from "corrupt ID".

**Fix:** Propagate the error:

```rust
let id = Uuid::parse_str(&str)
    .map_err(|e| DbError::Other(format!("invalid UUID: {}", e)))?;
```

### ❌ Logging PHI in Error Messages

```rust
// BAD: Exposes patient data
tracing::error!("Failed to process: {}", transcript);
```

**Problem:** Violates HIPAA/PHI constraints. Patient data in logs.

**Fix:** Log IDs and operation names only:

```rust
tracing::error!(
    recording_id = %id,
    operation = "process_transcript",
    "Processing failed"
);
```

## Error Handling in Tests

Test code is allowed to use `unwrap()` and `expect()`:

```rust
#[test]
fn test_happy_path() {
    let result = do_something().unwrap();  // OK in tests
    assert_eq!(result, expected);
}
```

**Why:** Tests should fail fast on unexpected errors. Panics are appropriate here.

## Specialized Error Patterns (v0.10.92)

Extended error handling to cover 16 additional unwrap/expect calls across 5 files:

### Mutex Poisoning (11 calls)

**When to use:** When calling `Mutex::lock()` in production code.

```rust
let mut guard = mutex.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;
```

**Why:** A poisoned mutex indicates a thread panicked while holding the lock. Returning an error allows graceful degradation instead of cascading thread failures.

**Used in:**
- `src-tauri/src/commands/audio.rs` (10 calls)
- `src-tauri/src/commands/pipeline.rs` (1 call)

### Invalid Path (2 calls)

**When to use:** When calling `Path::parent()` which can return `None` for root paths.

```rust
let parent = path.parent()
    .ok_or_else(|| SharingError::InvalidPath(format!("no parent dir: {}", path.display())))?;
std::fs::create_dir_all(parent).map_err(SharingError::Io)?;
```

**Why:** Service installation paths might not have a parent directory (e.g., root path). Returning an error provides clear feedback instead of panicking.

**Used in:**
- `crates/sharing/src/service_installer.rs` (2 calls)

### HTTP Client (1 call)

**When to use:** When building HTTP clients with `reqwest::Client::builder()`.

```rust
let client = Client::builder()
    .connect_timeout(std::time::Duration::from_secs(10))
    .timeout(std::time::Duration::from_secs(120))
    .build()
    .map_err(|e| AppError::HttpClient(format!("failed to build client: {e}")))?;
```

**Why:** Client construction can fail due to system-level issues (e.g., TLS library initialization). Returning an error allows the RAG system to fail gracefully.

**Used in:**
- `crates/rag/src/embeddings.rs` (1 call)

### Invalid Header (2 calls)

**When to use:** When parsing HTTP header values from user input or API keys.

```rust
let header_value = api_key.parse()
    .map_err(|e| TtsError::InvalidHeader(format!("api-key header: {e}")))?;
```

**Why:** HTTP headers must be valid ASCII without control characters. Validating upfront provides clear error messages for invalid API keys.

**Used in:**
- `crates/tts-providers/src/elevenlabs_tts.rs` (2 calls)

## Summary

| Pattern | When to Use | Example |
|---------|-------------|---------|
| `?` | Error types match or have `From` | `let row = conn.query_row(...)?;` |
| `.map_err()` | Add context or convert types | `.map_err(\|e\| AppError::Database(e.to_string()))?` |
| Structured context | Complex operations with logging | `.map_err(\|e\| { tracing::error!(...); e })?` |
| Default values | Semantically correct defaults only | `.unwrap_or(0)` for counts |
| `unwrap()`/`expect()` | **Tests only** | `result.unwrap()` in `#[test]` |

## Related Documentation

- [Initial Error Handling Refactor](specs/2026-05-26-error-handling-refactor-design.md) — Original 5-file refactor (v0.10.91)
- [Error Handling Expansion](specs/2026-05-26-error-handling-expansion-design.md) — Extended patterns (v0.10.92)
- [Implementation Plan](plans/2026-05-26-error-handling-refactor.md) — Initial refactor tasks
- [Expansion Implementation Plan](plans/2026-05-26-error-handling-expansion.md) — Extended patterns tasks
