# Error Handling Expansion Design

**Date:** 2026-05-26  
**Status:** Approved  
**Priority:** Low (Code Quality)  
**Driver:** Eliminate remaining unwrap/expect calls in production code

## Problem Statement

The initial error handling refactor (v0.10.91) fixed unwrap/expect calls in 5 critical-path files (generations.rs, orchestrator.rs, recordings.rs, vectors.rs, key_storage.rs). However, the original audit identified 893 total unwrap/expect calls in the codebase.

**Comprehensive audit findings:**

| Category | Count | Risk Level | Action |
|----------|-------|------------|--------|
| Test code (`#[cfg(test)]`, `tests/`, `test_helpers.rs`) | ~840 | ✅ Acceptable | No change |
| `Regex::new(...).unwrap()` in LazyLock | ~30 | ✅ Safe (compile-time validated) | No change |
| `write!()` to String | 1 | ✅ Safe (infallible) | No change |
| Documented infallible ops | 2 | ✅ Safe | No change |
| **Mutex `.lock().unwrap()`** | **11** | 🔴 Critical (cascading thread failures) | **Fix** |
| **`path.parent().unwrap()`** | **2** | 🔴 High (panics on root paths) | **Fix** |
| **`reqwest::Client::builder().expect()`** | **1** | 🔴 Medium (unlikely but possible) | **Fix** |
| **`api_key.parse().unwrap()`** | **2** | 🟡 Low (validates before send) | **Fix** |
| `tauri::Builder.run().expect()` | 1 | ⚪ Acceptable (app entry point) | No change |

**Scope:** Fix 16 high-risk unwrap/expect calls across 5 files.

## Design: Pattern-First Approach

### Architecture

Fix all issues of one type at a time, establishing consistent error handling patterns:

**Phase 1: Mutex Poisoning (11 calls in 2 files)**
- `src-tauri/src/commands/audio.rs`: 10 calls
- `src-tauri/src/commands/pipeline.rs`: 1 call

**Phase 2: Path.parent() (2 calls in 1 file)**
- `crates/sharing/src/service_installer.rs`: 2 calls

**Phase 3: Reqwest Builder (1 call in 1 file)**
- `crates/rag/src/embeddings.rs`: 1 call

**Phase 4: HTTP Header Parsing (2 calls in 1 file)**
- `crates/tts-providers/src/elevenlabs_tts.rs`: 2 calls

### Error Handling Patterns

#### Pattern 1: Mutex Poisoning

**Problem:** `Mutex::lock().unwrap()` panics if the mutex was poisoned by a thread panic while holding the lock. This causes cascading failures across threads.

**Solution:**
```rust
// Before:
let mut guard = state.capture_handle.lock().unwrap();

// After:
let mut guard = state.capture_handle.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;
```

**Implementation:**
- Add `AppError::MutexPoisoned(String)` variant to `crates/core/src/error.rs`
- Each mutex gets a descriptive name in the error message for debugging
- Converts `PoisonError<T>` → `AppError` with context

**Why this matters:** Mutex poisoning indicates a thread panicked while holding the lock. Continuing with `.unwrap()` would crash the entire application. Returning an error allows graceful degradation (e.g., show error message to user, retry operation).

#### Pattern 2: Path.parent()

**Problem:** `Path::parent()` returns `None` for root paths like `/` or `C:\`. Calling `.unwrap()` panics on these edge cases.

**Solution:**
```rust
// Before:
std::fs::create_dir_all(path.parent().unwrap())

// After:
let parent = path.parent()
    .ok_or_else(|| SharingError::InvalidPath(format!("no parent dir: {}", path.display())))?;
std::fs::create_dir_all(parent)
```

**Implementation:**
- Add `SharingError::InvalidPath(String)` variant to `crates/sharing/src/lib.rs`
- Provides clear error message with the problematic path
- Allows caller to handle the error gracefully

**Why this matters:** Service installation could fail if a user specifies an unusual path. Returning an error allows the UI to show a helpful message instead of crashing.

#### Pattern 3: Reqwest Client Builder

**Problem:** `Client::builder().build()` can theoretically fail (e.g., invalid TLS configuration). Using `.expect()` assumes success without validation.

**Solution:**
```rust
// Before:
let client = Client::builder()
    .connect_timeout(Duration::from_secs(10))
    .build()
    .expect("reqwest client builder should not fail with valid config");

// After:
let client = Client::builder()
    .connect_timeout(Duration::from_secs(10))
    .build()
    .map_err(|e| AppError::HttpClient(format!("failed to build client: {e}")))?;
```

**Implementation:**
- Add `AppError::HttpClient(String)` variant to `crates/core/src/error.rs`
- Graceful degradation instead of panic

**Why this matters:** While unlikely with valid configuration, this could fail due to system-level issues (e.g., missing TLS libraries). Returning an error allows the RAG system to fail gracefully.

#### Pattern 4: HTTP Header Parsing

**Problem:** `HeaderValue::from_str()` can fail if the value contains invalid characters (e.g., non-ASCII, control characters). Using `.unwrap()` assumes the API key is always valid.

**Solution:**
```rust
// Before:
h.insert("xi-api-key", api_key.parse().unwrap());
h.insert("Content-Type", "application/json".parse().unwrap());

// After:
h.insert("xi-api-key", api_key.parse()
    .map_err(|e| TtsError::InvalidHeader(format!("api-key header: {e}")))?);
h.insert("Content-Type", "application/json".parse()
    .map_err(|e| TtsError::InvalidHeader(format!("content-type header: {e}")))?);
```

**Implementation:**
- Add `TtsError::InvalidHeader(String)` variant to `crates/tts-providers/src/lib.rs`
- Validates API key format before sending request
- Prevents silent failures or malformed requests

**Why this matters:** If a user pastes an API key with invalid characters (e.g., trailing newline, Unicode), the request would fail with a cryptic error. Validating upfront provides a clear error message.

### Testing Strategy

**Per-pattern validation:**

1. **Mutex poisoning pattern**
   - Add test that verifies error propagation when mutex is poisoned
   - Mock a poisoned mutex (spawn thread, panic while holding lock)
   - Verify `AppError::MutexPoisoned` is returned with correct context
   - Files: `audio.rs` tests, `pipeline.rs` tests

2. **Path.parent() pattern**
   - Test with valid paths (should succeed)
   - Test with root paths like `/` or `C:\` (should return `InvalidPath` error)
   - Files: `service_installer.rs` tests

3. **Reqwest builder pattern**
   - Test successful client creation (happy path)
   - Error path is hard to trigger without invalid config, so document the assumption
   - Files: `embeddings.rs` tests

4. **Header parsing pattern**
   - Test with valid header strings (should succeed)
   - Test with invalid characters in header name/value (should return `InvalidHeader` error)
   - Files: `elevenlabs_tts.rs` tests

**Integration tests:**
- Run full test suite after each pattern is complete
- Manual smoke test: start app, verify recording/playback still works

**Success criteria:**
- [ ] All 16 unwrap/expect calls replaced with proper error handling
- [ ] All new error variants have at least one test
- [ ] Existing 768 backend tests still pass
- [ ] `cargo clippy --workspace` passes with no warnings
- [ ] Manual smoke test: dev server starts, basic features work

## Execution Approach

### Git Strategy
- Use isolated git worktree: `.worktrees/error-handling-expansion`
- Branch name: `refactor/error-handling-expansion`
- One commit per pattern (4 commits total)
- Final PR with all patterns

### Risk Mitigation
- Each pattern is a separate commit (easy to bisect)
- Full test suite after each pattern
- Manual smoke test before committing
- Rollback plan: revert individual commits if issues found

### Dependencies and Constraints

**Constraints:**
- Must maintain compatibility with existing error handling (v0.10.91)
- Cannot break existing functionality
- Must preserve HIPAA compliance (no new data collection)

**Dependencies:**
- Existing error type hierarchy (`AppError`, `DbError`, `SecurityError`, `SharingError`, `TtsError`)
- Tauri command signature compatibility

## Timeline

**Estimated effort:** 2 hours

- Phase 1 (Mutex poisoning): 45 minutes
- Phase 2 (Path.parent): 30 minutes
- Phase 3 (Reqwest builder): 15 minutes
- Phase 4 (Header parsing): 30 minutes

## Success Metrics

- **Code Quality:** Zero high-risk unwrap/expect calls in production code
- **Stability:** All tests pass, no regressions
- **Maintainability:** Consistent error handling patterns across codebase
- **Timeline:** Complete in one session

## Related Documentation

- [Error Handling Patterns Guide](../../error-handling.md)
- [Initial Error Handling Refactor](specs/2026-05-26-error-handling-refactor-design.md)
- [Dependency Updates](specs/2026-05-26-dependency-updates-design.md)
