# Remote Provider Refactor Design

**Date:** 2026-05-26
**Status:** Draft
**Priority:** Low (Code Organization)
**Estimated effort:** 3-4 hours

## Problem Statement

`crates/stt-providers/src/remote_provider.rs` has grown to 1036 lines, mixing multiple concerns:

- Endpoint resolution and caching logic (70 lines)
- HTTP client communication with Whisper server (140 lines)
- Provider orchestration and trait implementation (240 lines)
- Tests (555 lines)

This makes the file harder to navigate, test, and maintain. The endpoint resolution and HTTP client logic are tightly coupled to the provider struct, preventing reuse and granular testing.

## Goals

1. **Readability** — Separate concerns into focused modules
2. **Testability** — Enable isolated unit testing of endpoint resolution and HTTP client
3. **Reusability** — Make endpoint resolution logic available for other components
4. **Maintainability** — Reduce cognitive load when working on any single concern

## Proposed Solution: Three-Module Split

Split `remote_provider.rs` into three focused modules:

### 1. `endpoint.rs` — Endpoint Resolution (~70 lines)

**Responsibility:** Resolve and cache Whisper server URLs from RemoteEndpoint configuration.

**Public API:**
```rust
pub struct ResolvedCache {
    url: String,
    resolved_at: std::time::Instant,
}

pub const CACHE_TTL: Duration = Duration::from_secs(30);

pub async fn current_base_url(
    endpoint: &Option<RemoteEndpoint>,
    base_url: &str,
    cache: &mut Option<ResolvedCache>,
) -> AppResult<String>
```

**Contents:**
- `ResolvedCache` struct
- `CACHE_TTL` constant
- `current_base_url()` function (currently a method on `RemoteSttProvider`)

**Why separate:**
- Pure function with clear inputs/outputs
- Contains caching logic that could be reused
- Can be tested without HTTP client or provider state
- Single responsibility: URL resolution

### 2. `client.rs` — HTTP Client (~140 lines)

**Responsibility:** Handle HTTP communication with Whisper API server.

**Public API:**
```rust
pub struct VerboseJson {
    pub segments: Vec<VerboseSegment>,
    pub language: Option<String>,
    pub text: Option<String>,
}

pub struct VerboseSegment {
    pub start: f32,
    pub end: f32,
    pub text: Option<String>,
}

pub async fn post_audio(
    client: &Client,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    wav_bytes: Vec<u8>,
    language: Option<&str>,
    cancel: &CancellationToken,
) -> AppResult<VerboseJson>
```

**Contents:**
- `VerboseJson` and `VerboseSegment` structs (fields become `pub`)
- `post_audio()` function (currently a method on `RemoteSttProvider`)
- HTTP error handling (401/403, client errors, server errors)
- Cancellation logic using `tokio::select!`

**Why separate:**
- Encapsulates all HTTP concerns
- Complex error handling logic (auth errors, network errors, response parsing)
- Can be tested with mock servers independently
- Clear boundary: "everything HTTP"

### 3. `remote_provider.rs` — Provider Orchestration (~240 lines)

**Responsibility:** Orchestrate endpoint resolution, HTTP calls, and diarization.

**Contents:**
- `RemoteSttProvider` struct definition
- `new()` and `new_with_endpoint()` constructors
- `set_endpoint()` method
- `diarization_available()` helper
- `SttProvider` trait implementation
- Diarization logic (Stage 3 in `transcribe()`)

**Dependencies:**
- Calls `endpoint::current_base_url()` for URL resolution
- Calls `client::post_audio()` for HTTP communication
- Uses `endpoint::ResolvedCache` as a field type

## Module Interactions

```
remote_provider.rs (orchestration)
    ├─> endpoint::current_base_url()
    │       Input: endpoint, base_url, cache
    │       Output: AppResult<String>
    │
    ├─> client::post_audio()
    │       Input: client, base_url, model, api_key, wav_bytes, language, cancel
    │       Output: AppResult<VerboseJson>
    │
    └─> Local diarization logic (stays in remote_provider.rs)
```

The provider struct holds state and passes slices of it to helper functions:
- `endpoint: RwLock<Option<RemoteEndpoint>>`
- `url_cache: Mutex<Option<ResolvedCache>>`
- `client: Client`
- `model: String`
- `api_key: RwLock<Option<String>>`

## Test Distribution

### `endpoint.rs` tests (~80 lines)

Move from `remote_provider.rs`:
- `current_base_url_returns_static_when_no_endpoint`
- `current_base_url_caches_for_30s`
- `set_endpoint_clears_url_cache` (refactored to test endpoint logic directly)

Add new tests:
- `current_base_url_resolves_from_endpoint`
- `current_base_url_respects_cache_ttl`
- `current_base_url_returns_error_on_resolution_failure`

### `client.rs` tests (~300 lines)

Move from `remote_provider.rs`:
- All `post_audio()` error handling tests:
  - `http_401_with_unknown_token_reason_maps_to_repair_message`
  - `http_401_without_reason_header_maps_to_generic_auth_error`
  - `http_503_maps_to_server_internal_error`
  - `http_500_with_partial_body_includes_diagnostic_marker`
  - `malformed_json_maps_to_parse_error`
- Authorization tests:
  - `authorization_header_sent_when_api_key_present`
  - `no_authorization_header_when_api_key_absent`

Tests will use `wiremock` for mock HTTP servers (already a dev dependency).

### `remote_provider.rs` tests (~175 lines)

Keep in `remote_provider.rs`:
- Constructor validation:
  - `new_blocks_public_host_by_default`
  - `new_accepts_public_host_when_allow_public`
  - `new_accepts_local_hosts_with_default_allow_public`
  - `new_accepts_empty_host`
- Endpoint management:
  - `set_endpoint_rejects_public_lan_address`
  - `set_endpoint_accepts_lan_and_tailscale_addresses`
- Integration tests:
  - `happy_path_returns_segments_without_diarization`
  - `transcribe_returns_promptly_when_cancelled_mid_request`
  - `segments_without_text_are_skipped`
  - `transcribe_returns_endpoint_offline_when_remote_unreachable`
  - `diarization_available_is_false_without_models`

## Migration Strategy

Use incremental migration to minimize risk and maintain test coverage at each step.

### Step 1: Extract endpoint resolution (1-1.5 hours)

1. Create `crates/stt-providers/src/endpoint.rs`
2. Move `ResolvedCache` struct and `CACHE_TTL` constant
3. Extract `current_base_url()` as a standalone async function
4. Update `RemoteSttProvider::current_base_url()` to call the new function
5. Move endpoint-related tests to `endpoint.rs`
6. Run `cargo test` to verify no regressions
7. Commit: "refactor: extract endpoint resolution from remote_provider"

### Step 2: Extract HTTP client (1.5-2 hours)

1. Create `crates/stt-providers/src/client.rs`
2. Move `VerboseJson` and `VerboseSegment` structs (make fields `pub`)
3. Extract `post_audio()` as a standalone async function
4. Update `RemoteSttProvider::post_audio()` to call the new function
5. Move HTTP client tests to `client.rs`
6. Update test helpers to construct minimal state for `post_audio()` calls
7. Run `cargo test` to verify no regressions
8. Commit: "refactor: extract HTTP client from remote_provider"

### Step 3: Clean up (0.5-1 hour)

1. Update `crates/stt-providers/src/lib.rs` to expose new modules:
   ```rust
   pub mod endpoint;
   pub mod client;
   ```
2. Review imports and visibility modifiers
3. Remove any dead code or unused imports in `remote_provider.rs`
4. Run `cargo clippy` and `cargo test`
5. Commit: "refactor: finalize remote_provider module split"

## Acceptance Criteria

- [ ] `remote_provider.rs` reduced to ~240 lines (from 1036)
- [ ] `endpoint.rs` contains only endpoint resolution logic (~70 lines)
- [ ] `client.rs` contains only HTTP client logic (~140 lines)
- [ ] All 555 lines of tests pass and are distributed to appropriate modules
- [ ] `cargo test -p medical-stt-providers` passes with no failures
- [ ] `cargo clippy` passes with no warnings
- [ ] No changes to public API or behavior
- [ ] Git history shows clear incremental progression

## Risks and Mitigations

### Risk 1: Breaking changes to public API
**Mitigation:** The `RemoteSttProvider` struct and `SttProvider` trait implementation remain unchanged. Only internal organization changes.

### Risk 2: Test coverage gaps during migration
**Mitigation:** Run `cargo test` after each step. Don't proceed to next step until all tests pass.

### Risk 3: Circular dependencies between modules
**Mitigation:** Clear dependency direction: `remote_provider.rs` → `client.rs` → `endpoint.rs`. No reverse dependencies.

### Risk 4: Over-engineering
**Mitigation:** Three-module split is the minimum viable refactor. Could stop after Step 1 (endpoint extraction) if that's sufficient.

## Alternatives Considered

### Alternative A: Two-module split (endpoint + everything else)
- **Pros:** Smaller change, lower risk
- **Cons:** Still leaves a 970-line file, misses testability benefits
- **Verdict:** Doesn't achieve goals

### Alternative B: Four-module split (add separate provider.rs)
- **Pros:** Maximum granularity
- **Cons:** Construction logic tightly coupled to provider state, higher complexity
- **Verdict:** Over-engineering for current needs

### Alternative C: Move to separate crate
- **Pros:** Strong isolation, reusable across crates
- **Cons:** Premature abstraction, adds cross-crate dependency overhead
- **Verdict:** Not needed for current use case

## Success Metrics

1. **Line count reduction:** `remote_provider.rs` drops from 1036 to ~240 lines (77% reduction)
2. **Test isolation:** Each module can be tested independently
3. **Code navigation:** Finding endpoint or HTTP logic requires opening one file instead of searching
4. **Reusability:** `endpoint::current_base_url()` can be called from other code without instantiating `RemoteSttProvider`

## References

- Current implementation: `crates/stt-providers/src/remote_provider.rs`
- Module entry point: `crates/stt-providers/src/lib.rs`
- Related code: `crates/sharing/src/remote_endpoint.rs` (uses `RemoteEndpoint` type)
