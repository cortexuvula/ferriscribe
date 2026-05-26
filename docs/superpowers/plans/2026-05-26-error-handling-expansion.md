# Error Handling Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 16 high-risk unwrap/expect calls in production code using pattern-first approach

**Architecture:** Four phases organized by error type: (1) Mutex poisoning (11 calls), (2) Path.parent() (2 calls), (3) Reqwest builder (1 call), (4) HTTP header parsing (2 calls). Each phase adds a new error variant and applies it consistently across affected files.

**Tech Stack:** Rust, thiserror, Tauri, reqwest, tokio

---

## Task 1: Setup Isolated Workspace

**Files:**
- Create: `.worktrees/error-handling-expansion`
- Branch: `refactor/error-handling-expansion`

- [ ] **Step 1: Create git worktree**

```bash
cd /Users/cortexuvula/Development/rustMedicalAssistant
git worktree add .worktrees/error-handling-expansion -b refactor/error-handling-expansion
cd .worktrees/error-handling-expansion
```

- [ ] **Step 2: Install dependencies and verify baseline**

```bash
npm install
npm run check
npx vitest run
cargo test --workspace --lib
```

Expected: All tests pass (249 frontend, 768 backend)

- [ ] **Step 3: Verify clean state**

```bash
git status
```

Expected: Nothing to commit, working tree clean

---

## Task 2: Add MutexPoisoned Error Variant

**Files:**
- Modify: `crates/core/src/error.rs`
- Test: `crates/core/src/error.rs` (add test module)

- [ ] **Step 1: Write failing test for MutexPoisoned variant**

Add test module at bottom of `crates/core/src/error.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutex_poisoned_error_displays_context() {
        let err = AppError::MutexPoisoned("capture_handle: poisoned lock".to_string());
        let msg = err.to_string();
        assert!(msg.contains("capture_handle"), "got: {}", msg);
        assert!(msg.contains("poisoned lock"), "got: {}", msg);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p medical-core mutex_poisoned_error_displays_context
```

Expected: FAIL with "no variant `MutexPoisoned`"

- [ ] **Step 3: Add MutexPoisoned variant to AppError**

In `crates/core/src/error.rs`, find the `AppError` enum and add:

```rust
#[error("Mutex poisoned: {0}")]
MutexPoisoned(String),
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p medical-core mutex_poisoned_error_displays_context
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/error.rs
git commit -m "feat(error): add MutexPoisoned variant to AppError

Adds error variant for mutex poisoning scenarios with context string
for debugging which mutex was affected."
```

---

## Task 3: Fix Mutex Poisoning in audio.rs

**Files:**
- Modify: `src-tauri/src/commands/audio.rs`
- Test: Add tests in same file (test module already exists)

- [ ] **Step 1: Write failing test for mutex error propagation**

Add test in `src-tauri/src/commands/audio.rs` test module:

```rust
#[test]
fn mutex_poisoning_propagates_error() {
    use std::sync::{Arc, Mutex};
    use std::thread;
    
    let mutex = Arc::new(Mutex::new(0));
    let mutex_clone = Arc::clone(&mutex);
    
    // Poison the mutex by panicking while holding it
    let handle = thread::spawn(move || {
        let _guard = mutex_clone.lock().unwrap();
        panic!("intentional panic to poison mutex");
    });
    let _ = handle.join();
    
    // Verify lock() returns PoisonError
    let result = mutex.lock();
    assert!(result.is_err(), "mutex should be poisoned");
    
    // Verify we can convert to AppError
    let err = result.map_err(|e| AppError::MutexPoisoned(format!("test_mutex: {e}")));
    assert!(err.is_err());
    let app_err = err.unwrap_err();
    assert!(matches!(app_err, AppError::MutexPoisoned(_)));
}
```

- [ ] **Step 2: Run test to verify it passes (baseline)**

```bash
cargo test -p rust-medical-assistant mutex_poisoning_propagates_error
```

Expected: PASS (this test validates the pattern, not the fix)

- [ ] **Step 3: Replace all 10 unwrap() calls in audio.rs**

For each of the 10 occurrences in `src-tauri/src/commands/audio.rs`, replace:

```rust
// Line 127
let mut handle_lock = state.capture_handle.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;

// Line 133
let mut rec_lock = state.current_recording.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("current_recording: {e}")))?;

// Line 183
let mut handle_lock = state.capture_handle.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;

// Line 214
let mut rec_lock = state.current_recording.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("current_recording: {e}")))?;

// Line 282
let mut handle_lock = state.capture_handle.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;

// Line 296
let mut rec_lock = state.current_recording.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("current_recording: {e}")))?;

// Line 318
let mut rec_lock = state.current_recording.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("current_recording: {e}")))?;

// Line 336
let handle_lock = state.capture_handle.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;

// Line 354
let handle_lock = state.capture_handle.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("capture_handle: {e}")))?;

// Line 385
let guard = state.current_recording.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("current_recording: {e}")))?;
```

- [ ] **Step 4: Run all tests to verify no regressions**

```bash
cargo test -p rust-medical-assistant --lib
```

Expected: All tests pass

- [ ] **Step 5: Verify no unwrap() calls remain in production code**

```bash
grep -n "\.lock()\.unwrap()" src-tauri/src/commands/audio.rs
```

Expected: No output (no matches)

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/commands/audio.rs
git commit -m "fix(audio): replace 10 mutex.unwrap() with proper error handling

Eliminates cascading thread failures by converting PoisonError to
AppError::MutexPoisoned with descriptive context for each mutex."
```

---

## Task 4: Fix Mutex Poisoning in pipeline.rs

**Files:**
- Modify: `src-tauri/src/commands/pipeline.rs`

- [ ] **Step 1: Replace unwrap() call in pipeline.rs**

In `src-tauri/src/commands/pipeline.rs` line 56, replace:

```rust
let mut guard = state.pipeline_cancels.lock()
    .map_err(|e| AppError::MutexPoisoned(format!("pipeline_cancels: {e}")))?;
```

- [ ] **Step 2: Run all tests to verify no regressions**

```bash
cargo test -p rust-medical-assistant --lib
```

Expected: All tests pass

- [ ] **Step 3: Verify no unwrap() calls remain in production code**

```bash
grep -n "\.lock()\.unwrap()" src-tauri/src/commands/pipeline.rs
```

Expected: No output (no matches)

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/pipeline.rs
git commit -m "fix(pipeline): replace mutex.unwrap() with proper error handling

Converts PoisonError to AppError::MutexPoisoned for pipeline_cancels mutex."
```

---

## Task 5: Add InvalidPath Error Variant

**Files:**
- Modify: `crates/sharing/src/lib.rs`
- Test: `crates/sharing/src/lib.rs` (add test module)

- [ ] **Step 1: Write failing test for InvalidPath variant**

Add test module at bottom of `crates/sharing/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn invalid_path_error_displays_path() {
        let path = PathBuf::from("/");
        let err = SharingError::InvalidPath(format!("no parent dir: {}", path.display()));
        let msg = err.to_string();
        assert!(msg.contains("no parent dir"), "got: {}", msg);
        assert!(msg.contains("/"), "got: {}", msg);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p medical-sharing invalid_path_error_displays_path
```

Expected: FAIL with "no variant `InvalidPath`"

- [ ] **Step 3: Add InvalidPath variant to SharingError**

In `crates/sharing/src/lib.rs`, find the `SharingError` enum and add:

```rust
#[error("Invalid path: {0}")]
InvalidPath(String),
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p medical-sharing invalid_path_error_displays_path
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/sharing/src/lib.rs
git commit -m "feat(sharing): add InvalidPath variant to SharingError

Adds error variant for path validation failures with descriptive message."
```

---

## Task 6: Fix Path.parent() in service_installer.rs

**Files:**
- Modify: `crates/sharing/src/service_installer.rs`
- Test: `crates/sharing/src/service_installer.rs` (add tests)

- [ ] **Step 1: Write failing test for root path handling**

Add tests in `crates/sharing/src/service_installer.rs` test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_path_returns_error() {
        let path = PathBuf::from("/");
        let result = path.parent().ok_or_else(|| {
            SharingError::InvalidPath(format!("no parent dir: {}", path.display()))
        });
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SharingError::InvalidPath(_)));
    }

    #[test]
    fn valid_path_succeeds() {
        let path = PathBuf::from("/home/user/.config/systemd/user/ferriScribe.service");
        let result = path.parent().ok_or_else(|| {
            SharingError::InvalidPath(format!("no parent dir: {}", path.display()))
        });
        assert!(result.is_ok());
        let parent = result.unwrap();
        assert_eq!(parent, PathBuf::from("/home/user/.config/systemd/user"));
    }
}
```

- [ ] **Step 2: Run tests to verify they pass (baseline)**

```bash
cargo test -p medical-sharing root_path_returns_error
cargo test -p medical-sharing valid_path_succeeds
```

Expected: Both PASS (these tests validate the pattern, not the fix)

- [ ] **Step 3: Replace unwrap() calls in service_installer.rs**

In `crates/sharing/src/service_installer.rs`, replace both occurrences:

```rust
// Line 135 (macOS launchd plist)
let parent = path.parent()
    .ok_or_else(|| SharingError::InvalidPath(format!("no parent dir: {}", path.display())))?;
std::fs::create_dir_all(parent)
    .map_err(SharingError::Io)?;

// Line 199 (Linux systemd unit)
let parent = path.parent()
    .ok_or_else(|| SharingError::InvalidPath(format!("no parent dir: {}", path.display())))?;
std::fs::create_dir_all(parent)
    .map_err(SharingError::Io)?;
```

- [ ] **Step 4: Run all tests to verify no regressions**

```bash
cargo test -p medical-sharing --lib
```

Expected: All tests pass

- [ ] **Step 5: Verify no unwrap() calls remain in production code**

```bash
awk '/^#\[cfg\(test\)\]/{exit} {print}' crates/sharing/src/service_installer.rs | grep -n "\.parent()\.unwrap()"
```

Expected: No output (no matches)

- [ ] **Step 6: Commit**

```bash
git add crates/sharing/src/service_installer.rs
git commit -m "fix(sharing): replace path.parent().unwrap() with proper error handling

Handles edge case where service installation path has no parent directory
(e.g., root path). Returns InvalidPath error instead of panicking."
```

---

## Task 7: Add HttpClient Error Variant

**Files:**
- Modify: `crates/core/src/error.rs`
- Test: `crates/core/src/error.rs` (add to existing test module)

- [ ] **Step 1: Write failing test for HttpClient variant**

Add test in existing test module in `crates/core/src/error.rs`:

```rust
#[test]
fn http_client_error_displays_details() {
    let err = AppError::HttpClient("failed to build client: TLS error".to_string());
    let msg = err.to_string();
    assert!(msg.contains("failed to build client"), "got: {}", msg);
    assert!(msg.contains("TLS error"), "got: {}", msg);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p medical-core http_client_error_displays_details
```

Expected: FAIL with "no variant `HttpClient`"

- [ ] **Step 3: Add HttpClient variant to AppError**

In `crates/core/src/error.rs`, add to `AppError` enum:

```rust
#[error("HTTP client error: {0}")]
HttpClient(String),
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p medical-core http_client_error_displays_details
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/error.rs
git commit -m "feat(error): add HttpClient variant to AppError

Adds error variant for HTTP client construction failures."
```

---

## Task 8: Fix Reqwest Builder in embeddings.rs

**Files:**
- Modify: `crates/rag/src/embeddings.rs`

- [ ] **Step 1: Replace expect() call in embeddings.rs**

In `crates/rag/src/embeddings.rs` line 36, replace:

```rust
let client = Client::builder()
    .connect_timeout(std::time::Duration::from_secs(10))
    .timeout(std::time::Duration::from_secs(120))
    .build()
    .map_err(|e| AppError::HttpClient(format!("failed to build client: {e}")))?;
```

- [ ] **Step 2: Run all tests to verify no regressions**

```bash
cargo test -p medical-rag --lib
```

Expected: All tests pass

- [ ] **Step 3: Verify no expect() calls remain in production code**

```bash
awk '/^#\[cfg\(test\)\]/{exit} {print}' crates/rag/src/embeddings.rs | grep -n "\.expect("
```

Expected: No output (no matches)

- [ ] **Step 4: Commit**

```bash
git add crates/rag/src/embeddings.rs
git commit -m "fix(rag): replace reqwest builder expect() with proper error handling

Converts client construction failure to AppError::HttpClient instead of
panicking. Handles edge cases like missing TLS libraries."
```

---

## Task 9: Add InvalidHeader Error Variant

**Files:**
- Modify: `crates/tts-providers/src/lib.rs`
- Test: `crates/tts-providers/src/lib.rs` (add test module)

- [ ] **Step 1: Write failing test for InvalidHeader variant**

Add test module at bottom of `crates/tts-providers/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_header_error_displays_context() {
        let err = TtsError::InvalidHeader("api-key header: invalid ASCII".to_string());
        let msg = err.to_string();
        assert!(msg.contains("api-key header"), "got: {}", msg);
        assert!(msg.contains("invalid ASCII"), "got: {}", msg);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p medical-tts-providers invalid_header_error_displays_context
```

Expected: FAIL with "no variant `InvalidHeader`"

- [ ] **Step 3: Add InvalidHeader variant to TtsError**

In `crates/tts-providers/src/lib.rs`, find the `TtsError` enum and add:

```rust
#[error("Invalid header: {0}")]
InvalidHeader(String),
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p medical-tts-providers invalid_header_error_displays_context
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/tts-providers/src/lib.rs
git commit -m "feat(tts): add InvalidHeader variant to TtsError

Adds error variant for HTTP header validation failures."
```

---

## Task 10: Fix Header Parsing in elevenlabs_tts.rs

**Files:**
- Modify: `crates/tts-providers/src/elevenlabs_tts.rs`
- Test: `crates/tts-providers/src/elevenlabs_tts.rs` (add tests)

- [ ] **Step 1: Write failing test for invalid header characters**

Add tests in `crates/tts-providers/src/elevenlabs_tts.rs` test module:

```rust
#[test]
fn invalid_header_characters_return_error() {
    use reqwest::header::HeaderValue;
    
    let api_key = "valid-key-\n-with-newline";
    let result: Result<HeaderValue, _> = api_key.parse()
        .map_err(|e| TtsError::InvalidHeader(format!("api-key header: {e}")));
    
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, TtsError::InvalidHeader(_)));
}

#[test]
fn valid_header_succeeds() {
    use reqwest::header::HeaderValue;
    
    let api_key = "valid-key-abc123";
    let result: Result<HeaderValue, _> = api_key.parse()
        .map_err(|e| TtsError::InvalidHeader(format!("api-key header: {e}")));
    
    assert!(result.is_ok());
}
```

- [ ] **Step 2: Run tests to verify they pass (baseline)**

```bash
cargo test -p medical-tts-providers invalid_header_characters_return_error
cargo test -p medical-tts-providers valid_header_succeeds
```

Expected: Both PASS (these tests validate the pattern, not the fix)

- [ ] **Step 3: Replace unwrap() calls in elevenlabs_tts.rs**

In `crates/tts-providers/src/elevenlabs_tts.rs` lines 32-33, replace:

```rust
h.insert("xi-api-key", api_key.parse()
    .map_err(|e| TtsError::InvalidHeader(format!("api-key header: {e}")))?);
h.insert("Content-Type", "application/json".parse()
    .map_err(|e| TtsError::InvalidHeader(format!("content-type header: {e}")))?);
```

- [ ] **Step 4: Run all tests to verify no regressions**

```bash
cargo test -p medical-tts-providers --lib
```

Expected: All tests pass

- [ ] **Step 5: Verify no unwrap() calls remain in production code**

```bash
awk '/^#\[cfg\(test\)\]/{exit} {print}' crates/tts-providers/src/elevenlabs_tts.rs | grep -n "\.parse()\.unwrap()"
```

Expected: No output (no matches)

- [ ] **Step 6: Commit**

```bash
git add crates/tts-providers/src/elevenlabs_tts.rs
git commit -m "fix(tts): replace header parsing unwrap() with proper error handling

Validates API key and content-type headers before sending request.
Returns TtsError::InvalidHeader with context instead of panicking on
invalid characters (e.g., trailing newline, non-ASCII)."
```

---

## Task 11: Final Validation and Cleanup

**Files:**
- All modified files
- Update: `docs/error-handling.md`

- [ ] **Step 1: Run full test suite**

```bash
cargo test --workspace --lib
npm run check
npx vitest run
```

Expected: All tests pass (768 backend, 249 frontend)

- [ ] **Step 2: Run clippy**

```bash
cargo clippy --workspace
```

Expected: No warnings

- [ ] **Step 3: Verify all 16 unwrap/expect calls are fixed**

```bash
# Check mutex poisoning
grep -rn "\.lock()\.unwrap()" src-tauri/src/commands/ --include="*.rs" | grep -v "test"

# Check path.parent()
grep -rn "\.parent()\.unwrap()" crates/sharing/src/ --include="*.rs" | grep -v "test"

# Check reqwest builder
grep -rn "\.build()\.expect(" crates/rag/src/ --include="*.rs" | grep -v "test"

# Check header parsing
grep -rn "\.parse()\.unwrap()" crates/tts-providers/src/ --include="*.rs" | grep -v "test"
```

Expected: No output from all four commands (no matches)

- [ ] **Step 4: Update error-handling.md documentation**

Add section to `docs/error-handling.md`:

```markdown
## Error Handling Expansion (v0.10.92)

Extended error handling to cover 16 additional unwrap/expect calls:

### Mutex Poisoning (11 calls)
- `AppError::MutexPoisoned(String)` for `Mutex::lock()` failures
- Used in: `audio.rs` (10 calls), `pipeline.rs` (1 call)
- Prevents cascading thread failures when a thread panics while holding a lock

### Invalid Path (2 calls)
- `SharingError::InvalidPath(String)` for `Path::parent()` failures
- Used in: `service_installer.rs` (2 calls)
- Handles edge case where service path has no parent directory

### HTTP Client (1 call)
- `AppError::HttpClient(String)` for `Client::builder().build()` failures
- Used in: `embeddings.rs` (1 call)
- Handles system-level failures (e.g., missing TLS libraries)

### Invalid Header (2 calls)
- `TtsError::InvalidHeader(String)` for `HeaderValue::from_str()` failures
- Used in: `elevenlabs_tts.rs` (2 calls)
- Validates API keys before sending requests
```

- [ ] **Step 5: Commit documentation update**

```bash
git add docs/error-handling.md
git commit -m "docs: update error-handling.md with expansion patterns

Documents the 4 new error handling patterns added in v0.10.92:
- Mutex poisoning
- Invalid path
- HTTP client
- Invalid header"
```

- [ ] **Step 6: Manual smoke test**

```bash
npm run tauri dev
```

Expected: App starts, recording/playback works, no crashes

- [ ] **Step 7: Final commit summary**

```bash
git log --oneline refactor/error-handling-expansion --not master
```

Expected: 11 commits (1 setup + 10 implementation)

---

## Success Criteria

- [ ] All 16 unwrap/expect calls replaced with proper error handling
- [ ] All new error variants have at least one test
- [ ] Existing 768 backend tests still pass
- [ ] Existing 249 frontend tests still pass
- [ ] `cargo clippy --workspace` passes with no warnings
- [ ] Manual smoke test: dev server starts, basic features work
- [ ] All 11 tasks completed with clean commits
