# Sharing Crate Test Backfill — Tier 1 Design

**Status:** Draft — pending user review
**Date:** 2026-05-15
**Author:** Brainstorming session with Claude Code

## Goal

Add unit tests to five modules in the `medical-sharing` crate that currently have zero coverage. The audit ([2026-05-14 sharing-survey](../specs/2026-05-14-record-tab-layout-design.md) sibling audit) flagged this as "the office-server boundary with the most untested surface area." Tier 1 covers the highest-leverage targets: the security-critical `TokenStore` plus four small pure-function helpers.

## Problem

Looking at `crates/sharing/src/` today:

| Module | LOC | Has tests? |
|---|---|---|
| `token_store.rs` | 198 | ❌ |
| `auth_proxy.rs` | 160 | ❌ (Tier 2) |
| `orchestrator.rs` | 413 | ❌ (Tier 3) |
| `whisper_supervisor.rs` | 345 | ❌ (Tier 2) |
| `service_installer.rs` | 291 | ❌ partial — Tier 1 covers helpers |
| `pairing.rs` | 92 | ❌ — Tier 1 covers `generate_code` |
| `tailscale.rs` | 15 | ❌ |
| `suggested_label.rs` | 30 | ❌ |
| `mdns.rs` | 171 | ✅ (`mod tests` exists) |
| `qr.rs` | 114 | ✅ (`mod tests` + 2 tests) |

The crate is the boundary between the local FerriScribe app and a paired office server — multi-client deployment bugs originate here. The most consequential gap is `TokenStore`: every paired bearer token is issued, stored, validated, and revoked through that module, and there are no tests covering the security envelope (key-mismatch rejection, replay, double-revoke, etc.).

## Non-goals

- **Tier 2 modules** (`auth_proxy`, `whisper_supervisor::ensure_binary`) — out of scope; needs wiremock infrastructure that doesn't yet exist in the sharing crate. Follow-up.
- **Tier 3** (`orchestrator::SharingService`, per-platform `install_persistent_ollama`) — out of scope; needs an integration harness or syscall mocks.
- **Refactoring for testability** — none needed for Tier 1. Every chosen function is already testable as-is via `tempfile::tempdir()` or pure inputs.
- **Production code changes** — this is a test-only batch. Any production change is a sign of scope creep.

## Hard constraints honored

- **No new deps.** `tempfile` is already a workspace dep and is declared in `crates/sharing/Cargo.toml`'s `[dev-dependencies]`. No npm or cargo additions.
- **No PHI in logs.** Test-only change; pairing tokens are opaque random bytes, never PHI. Confirmed by reading the token format in `token_store.rs::issue` — `IssuedToken { token, hash }` where `token` is hex-encoded random bytes.
- **No network calls.** All tests are local: pure functions, in-memory state, or `tempfile::tempdir()`.

## Decisions captured

| Question | Choice |
|---|---|
| Scope | Tier 1 only (token_store + 4 pure-function modules) |
| Test infrastructure | `tempfile::tempdir()` for IO-bound code; pure inputs elsewhere |
| Refactoring for testability | None |
| Production code changes | None |
| Total new tests | 29 across 5 modules |

## Test plan per module

### `token_store.rs` — 12 tests (the security-critical one)

`TokenStore` is a SQLCipher-backed table of issued client tokens. The encryption key is a `&[u8; 32]`. Public API:

```rust
pub fn open<P: AsRef<Path>>(path: P, key: &[u8; 32]) -> Result<Self>;
pub fn issue(&self, label: &str) -> Result<IssuedToken>;
pub fn validate(&self, token: &str) -> Result<Option<ClientRow>>;
pub fn touch(&self, id: i64) -> Result<()>;
pub fn revoke(&self, id: i64) -> Result<()>;
pub fn update_label(&self, id: i64, new_label: &str) -> Result<()>;
pub fn list(&self) -> Result<Vec<ClientRow>>;
```

**Test helper** (private to the test module):

```rust
fn fresh_store() -> (TokenStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tokens.db");
    let key = [42u8; 32]; // deterministic for test reproducibility
    let store = TokenStore::open(&path, &key).expect("open fresh store");
    (store, dir)
}
```

(Return the `TempDir` so it lives the duration of the test — drop closes it.)

**12 tests:**

1. `open_creates_fresh_database` — `open` against a non-existent path succeeds; second `list()` returns empty.
2. `open_reopens_existing_database` — issue a token, drop the store, reopen with the same key + path, the issued token still validates.
3. `open_with_wrong_key_rejects` — issue with key A, reopen with key B, expect `Err` (SQLCipher key mismatch). The error variant should be the appropriate `TokenStoreError`.
4. `issue_returns_opaque_token_and_hash` — `IssuedToken { token, hash }` has both populated; `token` is the user-facing secret, `hash` is the stored verifier. Token length is sensible (e.g., ≥32 hex chars).
5. `issue_returns_different_tokens_each_call` — two consecutive `issue("a")` and `issue("b")` produce distinct token strings (entropy check).
6. `validate_returns_some_for_issued_token` — `validate(issued.token)` returns `Some(ClientRow)` with the matching label.
7. `validate_returns_none_for_unknown_token` — `validate("not-a-real-token")` returns `Ok(None)` (not `Err`).
8. `validate_returns_none_for_revoked_token` — issue → revoke by `id` → validate of the original token returns `None`.
9. `touch_updates_last_seen` — issue → record `last_seen_at` → `touch(id)` → list and observe a later `last_seen_at`.
10. `revoke_is_idempotent` — issue → `revoke(id)` → `revoke(id)` again returns `Ok` (double-revoke is not an error).
11. `update_label_changes_visible_label` — issue → `update_label(id, "renamed")` → `list()[0].label == "renamed"`.
12. `list_returns_all_non_revoked_or_all_rows` — issue 3, revoke 1, list returns either 2 or 3 depending on the existing semantics (read the implementation to confirm). Either way, the test asserts the *current* behavior so a future refactor surfaces.

### `tailscale.rs` — 5 tests

`pub fn parse_self_dns_name(json: &[u8]) -> Option<String>`. A 15-line JSON parser that extracts `Self.DNSName` from `tailscale status --json` output.

1. `parses_valid_json_with_dnsname` — `{"Self":{"DNSName":"myhost.tail-scale.ts.net."}}` → `Some("myhost.tail-scale.ts.net.")`.
2. `returns_none_for_empty_input` — `b""` → `None`.
3. `returns_none_for_missing_self_field` — `{"Other":{}}` → `None`.
4. `returns_none_for_missing_dnsname_field` — `{"Self":{"NoSuchField":"x"}}` → `None`.
5. `returns_none_for_malformed_json` — `b"not json"` → `None` (and doesn't panic).

### `suggested_label.rs` — 7 tests

`pub fn sanitise(raw: &str) -> String` and `pub fn suggested_client_label() -> String`. Pure string sanitisation for OS hostnames.

1. `sanitise_passes_through_ascii_alphanumeric` — `"clinic-laptop"` → `"clinic-laptop"`.
2. `sanitise_handles_empty_input` — `""` → some sensible default (read the implementation; assert *current* behavior).
3. `sanitise_strips_or_replaces_symbols` — `"name!@#$"` → reasonable cleaned output.
4. `sanitise_trims_long_input` — input of 200 chars → output ≤ some cap (assert the cap by reading the implementation; if no cap, the test is descriptive).
5. `sanitise_handles_unicode` — `"診療所"` → either passed through or replaced (assert current behavior).
6. `sanitise_strips_trailing_punctuation` — `"name..."` → `"name"`.
7. `suggested_client_label_returns_non_empty` — smoke test that the function returns at least one character.

### `service_installer.rs` — 2 tests

Only the pure-function helper `xml_escape` gets tested. Other helpers are skipped:

- `find_ollama_binary` reads `PATH` directly — `serial_test` isn't a workspace dep, and `PATH` is a process-wide global, so a parallel test could pollute it. Flagged as a future refactor candidate (inject `PathBuf` list instead).
- `ollama_port_in_use` hardcodes `127.0.0.1:11434`; we'd either depend on Ollama not being installed locally (flaky in dev) or bind the well-known port from a test (anti-social and conflicts with parallel tests). Skip.

1. `xml_escape_basic_chars` — `&`, `<`, `>`, `"` each replaced correctly; ordinary chars pass through.
2. `xml_escape_empty_input` — `""` → `""`.

### `pairing.rs` — 3 tests for `generate_code`

`pub fn generate_code() -> String`. A pure RNG over 6 digits.

1. `generate_code_is_six_digits` — output `.len() == 6` AND every char is `'0'..='9'`.
2. `generate_code_produces_distinct_outputs` — 100 calls; collect into a `HashSet`; size ≥ 95 (allows for the astronomically unlikely birthday collision but catches a fully-broken RNG).
3. `generate_code_covers_full_range` — 1000 calls, check that at least one starts with `'0'` and at least one starts with `'9'` (otherwise the digit distribution is suspect).

## Architecture overview

```
crates/sharing/src/
├── token_store.rs           MODIFIED — append `#[cfg(test)] mod tests` (12 tests)
├── tailscale.rs             MODIFIED — append `#[cfg(test)] mod tests` (5 tests)
├── suggested_label.rs       MODIFIED — append `#[cfg(test)] mod tests` (7 tests)
├── service_installer.rs     MODIFIED — append `#[cfg(test)] mod tests` (2 tests)
├── pairing.rs               MODIFIED — append `#[cfg(test)] mod tests` (3 tests)
└── (all others unchanged)
```

Each test module is appended at the bottom of its file. No production code is touched.

## Component contracts

Each test module's contract is: **assert the existing behavior of the public functions**. If a test reveals a bug, that's a separate fix — flag it in the implementer's report, do NOT silently change production code. Per the project's TDD discipline, the failing test is the bug report.

## Data flow

Not applicable. Test-only change with no runtime behavior.

## Error handling

| Scenario | Behavior |
|---|---|
| `token_store::open` with a corrupted DB file | Returns `TokenStoreError` (not panic) — test #1 indirectly verifies via the fresh-DB path. |
| `tempfile::tempdir()` fails to create a directory | Test panics with `unwrap()` — appropriate for tests; user-visible test failure points at the host filesystem. |
| `find_ollama_binary` PATH manipulation race | Documented in test #3/#4; uses `serial_test::serial` if available, otherwise dropped. |
| Production code panics inside a test | Test fails — desired outcome (we want to know). |

## Testing

The tests themselves ARE the deliverable. To verify:

```bash
cargo test -p medical-sharing --lib
```

Expected: total test count grows by 29. All pass on macOS, Linux, Windows (the test code is cross-platform).

`cargo test --workspace --lib` should not regress any other crate.

## Open questions

None blocking.

**Future iterations (Tier 2):**
- `auth_proxy::spawn_auth_proxy` with `wiremock` (bearer auth, forwarding, error codes).
- `whisper_supervisor::ensure_binary` with a local fake HTTP server returning controlled SHA256 mismatches and partial-download scenarios.

**Future iterations (Tier 3):**
- `orchestrator::SharingService` — needs a full integration harness with a real `TokenStore`, an in-process `auth_proxy` listener, and a stubbed `WhisperSupervisor`. Worth its own design pass.
- Per-platform `install_persistent_ollama` — would need a syscall-level mocking layer (e.g., a trait for `launchctl` / `systemctl` / `sc.exe` invocation that prod code uses but tests can substitute).

## Implementation order

1. `token_store.rs` first (highest value; if we get pulled into a long fix-needed loop, this is the test set that matters most).
2. `tailscale.rs`, `suggested_label.rs` — pure-function quick wins.
3. `service_installer.rs` — helpers; defer the PATH-manipulation tests if `serial_test` is missing.
4. `pairing.rs::generate_code` — last; the smallest test module.

Each module is its own commit so a failure can be rolled back without disturbing the others.
