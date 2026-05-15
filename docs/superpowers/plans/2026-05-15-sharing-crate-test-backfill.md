# Sharing Crate Test Backfill — Tier 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add 29 unit tests across 5 currently-untested modules in `crates/sharing/src/`, covering the security-critical `TokenStore` and four small pure-function helpers. **No production code changes.**

**Architecture:** Each module gets its own `#[cfg(test)] mod tests` block appended at the bottom of the file. Each module is one commit. `TokenStore` uses `tempfile::tempdir()` + a deterministic 32-byte key. Other modules use pure inputs.

**Tech Stack:** Rust 2024, `cargo test`. Existing workspace deps only: `tempfile` (already in `crates/sharing/Cargo.toml`'s `[dev-dependencies]`). `hex` is already an indirect dep used by token_store.rs itself. No new deps.

**Spec:** [`docs/superpowers/specs/2026-05-15-sharing-crate-test-backfill-design.md`](../specs/2026-05-15-sharing-crate-test-backfill-design.md)

---

## File Structure

**Modified files (test-only additions):**
- `crates/sharing/src/token_store.rs` — append `#[cfg(test)] mod tests` (12 tests)
- `crates/sharing/src/tailscale.rs` — append `#[cfg(test)] mod tests` (5 tests)
- `crates/sharing/src/suggested_label.rs` — append `#[cfg(test)] mod tests` (7 tests)
- `crates/sharing/src/service_installer.rs` — append `#[cfg(test)] mod tests` (2 tests)
- `crates/sharing/src/pairing.rs` — append `#[cfg(test)] mod tests` (3 tests)

**Worktree:** Use `superpowers:using-git-worktrees` to create `.worktrees/sharing-test-backfill` from `master` at commit `c4dd4fa` (the spec commit).

---

## Task 1: `token_store.rs` — 12 security-critical tests (TDD-ish)

**Files:**
- Modify: `crates/sharing/src/token_store.rs` (append `#[cfg(test)] mod tests` block)

**Why first:** This is the highest-value test set. `TokenStore` is the security boundary between paired clients and the office server. If we ship only one module's worth of tests, this is the one.

**Helper for the test module:**

```rust
fn fresh_store() -> (TokenStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("tokens.db");
    let key = [42u8; 32]; // deterministic for reproducibility
    let store = TokenStore::open(&path, &key).expect("open fresh store");
    (store, dir)
}
```

- [ ] **Step 1: Append the test module**

Append the following to `crates/sharing/src/token_store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_store() -> (TokenStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.db");
        let key = [42u8; 32];
        let store = TokenStore::open(&path, &key).expect("open fresh store");
        (store, dir)
    }

    #[test]
    fn open_creates_fresh_database_with_no_clients() {
        let (store, _dir) = fresh_store();
        let rows = store.list().expect("list");
        assert!(rows.is_empty());
    }

    #[test]
    fn open_reopens_existing_database_with_same_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.db");
        let key = [42u8; 32];

        let token = {
            let store = TokenStore::open(&path, &key).expect("open1");
            let issued = store.issue("first-client").expect("issue");
            issued.token
        };

        // Drop the first store, reopen with the same path + key.
        let store = TokenStore::open(&path, &key).expect("open2");
        let validated = store.validate(&token).expect("validate");
        assert!(validated.is_some(), "issued token should still validate after reopen");
        assert_eq!(validated.unwrap().label, "first-client");
    }

    #[test]
    fn open_with_wrong_key_rejects_database_access() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.db");
        let key_a = [42u8; 32];
        let key_b = [99u8; 32];

        {
            let store = TokenStore::open(&path, &key_a).expect("open with key_a");
            let _ = store.issue("client-a").expect("issue");
        }

        // Reopening with key_b should fail (SQLCipher key mismatch on any query).
        let reopened = TokenStore::open(&path, &key_b);
        match reopened {
            Ok(store) => {
                // open() may succeed lazily; the first real query must fail.
                let r = store.list();
                assert!(
                    r.is_err(),
                    "list() with the wrong key must fail; got {r:?}"
                );
            }
            Err(_) => {} // open() rejected up front — also acceptable
        }
    }

    #[test]
    fn issue_returns_id_and_opaque_token() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("alpha").expect("issue");
        assert!(issued.id > 0, "id should be positive: {}", issued.id);
        // Token is base64-url-encoded 32 random bytes → 43 chars without padding.
        assert!(
            issued.token.len() >= 32,
            "token suspiciously short: {} chars",
            issued.token.len()
        );
        assert!(
            issued.token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "token must be base64-url-safe: {}",
            issued.token
        );
    }

    #[test]
    fn issue_returns_different_tokens_each_call() {
        let (store, _dir) = fresh_store();
        let t1 = store.issue("a").expect("issue a").token;
        let t2 = store.issue("b").expect("issue b").token;
        let t3 = store.issue("c").expect("issue c").token;
        assert_ne!(t1, t2);
        assert_ne!(t2, t3);
        assert_ne!(t1, t3);
    }

    #[test]
    fn validate_returns_some_for_issued_token() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("clinic-laptop").expect("issue");
        let row = store.validate(&issued.token).expect("validate").expect("Some");
        assert_eq!(row.id, issued.id);
        assert_eq!(row.label, "clinic-laptop");
        assert!(row.revoked_at.is_none());
    }

    #[test]
    fn validate_returns_none_for_unknown_token() {
        let (store, _dir) = fresh_store();
        let _ = store.issue("a").expect("issue");
        let row = store.validate("not-a-real-token").expect("validate");
        assert!(row.is_none());
    }

    #[test]
    fn validate_returns_none_for_revoked_token() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("doomed").expect("issue");
        store.revoke(issued.id).expect("revoke");
        let row = store.validate(&issued.token).expect("validate");
        assert!(row.is_none(), "revoked token should not validate");
    }

    #[test]
    fn touch_updates_last_seen_at() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("touched").expect("issue");

        // Before touch: last_seen_at is None.
        let before = store
            .validate(&issued.token)
            .expect("validate before")
            .expect("Some before");
        assert!(before.last_seen_at.is_none());

        store.touch(issued.id).expect("touch");

        let after = store
            .validate(&issued.token)
            .expect("validate after")
            .expect("Some after");
        assert!(after.last_seen_at.is_some(), "touch must populate last_seen_at");
    }

    #[test]
    fn revoke_is_idempotent() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("x").expect("issue");
        store.revoke(issued.id).expect("first revoke");
        // Second revoke must not error (no-op or successful idempotent UPDATE).
        store.revoke(issued.id).expect("second revoke is idempotent");
    }

    #[test]
    fn update_label_changes_visible_label_and_rejects_empty() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("old-name").expect("issue");

        store
            .update_label(issued.id, "renamed")
            .expect("update_label happy path");
        let rows = store.list().expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "renamed");

        // Empty (and whitespace-only) labels are rejected.
        let err = store
            .update_label(issued.id, "")
            .expect_err("empty label must error");
        assert!(matches!(err, TokenStoreError::EmptyLabel));
        let err2 = store
            .update_label(issued.id, "   ")
            .expect_err("whitespace-only label must error");
        assert!(matches!(err2, TokenStoreError::EmptyLabel));

        // Updating a non-existent or revoked id returns NotFound.
        let err3 = store
            .update_label(9999, "ghost")
            .expect_err("updating non-existent id must error");
        assert!(matches!(err3, TokenStoreError::NotFound));
    }

    #[test]
    fn update_label_truncates_to_80_chars() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("orig").expect("issue");
        let long = "a".repeat(200);
        store
            .update_label(issued.id, &long)
            .expect("update with long label");
        let rows = store.list().expect("list");
        assert_eq!(rows[0].label.chars().count(), 80, "label truncated to 80 chars");
    }

    #[test]
    fn list_returns_only_non_revoked_rows_in_id_order() {
        let (store, _dir) = fresh_store();
        let a = store.issue("a").expect("a");
        let b = store.issue("b").expect("b");
        let c = store.issue("c").expect("c");
        store.revoke(b.id).expect("revoke b");

        let rows = store.list().expect("list");
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![a.id, c.id], "revoked row should be filtered out");
        // Each listed row has revoked_at == None.
        assert!(rows.iter().all(|r| r.revoked_at.is_none()));
    }
}
```

- [ ] **Step 2: Run the new tests**

```bash
cargo test -p medical-sharing --lib token_store::tests
```

Expected: 12 tests pass.

- [ ] **Step 3: Run the full crate to catch any regression**

```bash
cargo test -p medical-sharing --lib
```

Expected: pre-existing tests (e.g., in `mdns.rs`, `qr.rs`) plus 12 new = total increases by 12.

- [ ] **Step 4: Commit**

```bash
git add crates/sharing/src/token_store.rs
git commit -m "test(sharing): add unit tests for TokenStore CRUD + security envelope"
```

---

## Task 2: `tailscale.rs` — 5 tests for `parse_self_dns_name`

**Files:**
- Modify: `crates/sharing/src/tailscale.rs`

- [ ] **Step 1: Append the test module**

Append to `crates/sharing/src/tailscale.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_json_with_dnsname() {
        let json = br#"{"Self":{"DNSName":"clinic-laptop.tail-scale.ts.net."}}"#;
        assert_eq!(
            parse_self_dns_name(json),
            Some("clinic-laptop.tail-scale.ts.net".to_string()),
            "trailing dot should be stripped"
        );
    }

    #[test]
    fn parses_valid_json_without_trailing_dot() {
        let json = br#"{"Self":{"DNSName":"host.example"}}"#;
        assert_eq!(parse_self_dns_name(json), Some("host.example".to_string()));
    }

    #[test]
    fn returns_none_for_empty_input() {
        assert_eq!(parse_self_dns_name(b""), None);
    }

    #[test]
    fn returns_none_when_self_field_is_missing() {
        let json = br#"{"Other":{"DNSName":"x"}}"#;
        assert_eq!(parse_self_dns_name(json), None);
    }

    #[test]
    fn returns_none_when_dnsname_field_is_missing() {
        let json = br#"{"Self":{"OtherField":"x"}}"#;
        assert_eq!(parse_self_dns_name(json), None);
    }

    #[test]
    fn returns_none_for_malformed_json() {
        assert_eq!(parse_self_dns_name(b"not json at all"), None);
        assert_eq!(parse_self_dns_name(b"{\"Self\":"), None);
    }
}
```

(Note: this is 6 tests, not the spec's 5 — added a "malformed JSON" companion test that also covers truncated JSON in a single test function for compactness. Either form is fine; merging gives slightly stronger coverage.)

- [ ] **Step 2: Run the new tests**

```bash
cargo test -p medical-sharing --lib tailscale::tests
```

Expected: 6 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sharing/src/tailscale.rs
git commit -m "test(sharing): add unit tests for parse_self_dns_name"
```

---

## Task 3: `suggested_label.rs` — 7 tests for `sanitise`

**Files:**
- Modify: `crates/sharing/src/suggested_label.rs`

The implementation's contract: trim, strip trailing `.local.` then `.local`, trim again, fall back to `"laptop"` if the result is empty.

- [ ] **Step 1: Append the test module**

Append to `crates/sharing/src/suggested_label.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_passes_through_plain_ascii() {
        assert_eq!(sanitise("clinic-laptop"), "clinic-laptop");
        assert_eq!(sanitise("workstation42"), "workstation42");
    }

    #[test]
    fn sanitise_trims_surrounding_whitespace() {
        assert_eq!(sanitise("  host  "), "host");
        assert_eq!(sanitise("\t\nhost\n"), "host");
    }

    #[test]
    fn sanitise_strips_dot_local_suffix() {
        assert_eq!(sanitise("clinic.local"), "clinic");
    }

    #[test]
    fn sanitise_strips_dot_local_dot_suffix() {
        // ".local." is mDNS-style and takes priority over ".local".
        assert_eq!(sanitise("clinic.local."), "clinic");
    }

    #[test]
    fn sanitise_falls_back_to_laptop_for_empty_or_whitespace() {
        assert_eq!(sanitise(""), "laptop");
        assert_eq!(sanitise("   "), "laptop");
    }

    #[test]
    fn sanitise_falls_back_to_laptop_for_lone_dot_local() {
        // ".local" with nothing in front should collapse to fallback.
        assert_eq!(sanitise(".local"), "laptop");
        assert_eq!(sanitise(".local."), "laptop");
    }

    #[test]
    fn suggested_client_label_returns_non_empty_string() {
        // We can't predict the host's actual hostname, but the function
        // must always return at least one character (either real or "laptop").
        let label = suggested_client_label();
        assert!(!label.is_empty(), "label must never be empty; got {label:?}");
    }
}
```

- [ ] **Step 2: Run the new tests**

```bash
cargo test -p medical-sharing --lib suggested_label::tests
```

Expected: 7 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sharing/src/suggested_label.rs
git commit -m "test(sharing): add unit tests for suggested_label::sanitise"
```

---

## Task 4: `service_installer.rs` — 2 tests for `xml_escape`

**Files:**
- Modify: `crates/sharing/src/service_installer.rs`

`xml_escape` is private but reachable from the same file's `#[cfg(test)] mod tests`.

- [ ] **Step 1: Append the test module**

Append to `crates/sharing/src/service_installer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_replaces_special_chars() {
        assert_eq!(
            xml_escape(r#"a & b < c > d "e""#),
            "a &amp; b &lt; c &gt; d &quot;e&quot;"
        );
        assert_eq!(xml_escape("safe-string_123"), "safe-string_123");
    }

    #[test]
    fn xml_escape_empty_input_is_empty() {
        assert_eq!(xml_escape(""), "");
    }
}
```

- [ ] **Step 2: Run the new tests**

```bash
cargo test -p medical-sharing --lib service_installer::tests
```

Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sharing/src/service_installer.rs
git commit -m "test(sharing): add unit tests for service_installer::xml_escape"
```

---

## Task 5: `pairing.rs` — 3 tests for `generate_code`

**Files:**
- Modify: `crates/sharing/src/pairing.rs`

- [ ] **Step 1: Append the test module**

Append to `crates/sharing/src/pairing.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generate_code_is_six_digits() {
        let code = generate_code();
        assert_eq!(code.len(), 6, "got {:?}", code);
        assert!(
            code.chars().all(|c| c.is_ascii_digit()),
            "non-digit in {:?}",
            code
        );
    }

    #[test]
    fn generate_code_produces_distinct_outputs() {
        // 100 draws from a 1-million space → birthday collisions are astronomically
        // rare. A duplicate rate above 5% indicates a broken RNG.
        let mut seen = HashSet::new();
        for _ in 0..100 {
            seen.insert(generate_code());
        }
        assert!(
            seen.len() >= 95,
            "RNG looks weak: only {} unique codes out of 100",
            seen.len()
        );
    }

    #[test]
    fn generate_code_covers_the_full_digit_range() {
        // 1000 draws should hit every first-digit at least once if uniform.
        // Use a small set of leading digits as a sanity check.
        let mut first_digits = HashSet::new();
        for _ in 0..1000 {
            if let Some(c) = generate_code().chars().next() {
                first_digits.insert(c);
            }
        }
        assert!(
            first_digits.contains(&'0'),
            "1000 draws produced no leading-0 code — RNG distribution is suspect"
        );
        assert!(
            first_digits.contains(&'9'),
            "1000 draws produced no leading-9 code — RNG distribution is suspect"
        );
    }
}
```

- [ ] **Step 2: Run the new tests**

```bash
cargo test -p medical-sharing --lib pairing::tests
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/sharing/src/pairing.rs
git commit -m "test(sharing): add unit tests for pairing::generate_code"
```

---

## Task 6: Final verification

**Files:** none (verification only).

- [ ] **Step 1: Run the full sharing crate test suite**

```bash
cargo test -p medical-sharing --lib
```

Expected output (counts):
- Pre-existing tests in `mdns.rs` + `qr.rs` (from `master`)
- + 12 (token_store) + 6 (tailscale) + 7 (suggested_label) + 2 (service_installer) + 3 (pairing) = **+30 new tests** total.

The plan summary said "29" — task 2 added one extra test (the malformed-JSON companion). Either count is correct.

- [ ] **Step 2: Run the workspace tests to catch any regression**

```bash
cargo test --workspace --lib 2>&1 | grep -E "^test result"
```

Expected: every line says `ok`, no `FAILED` anywhere.

- [ ] **Step 3: Confirm `npm run check` still clean**

(Not strictly required since this is a Rust-only batch, but worth a smoke check.)

```bash
npm run check 2>&1 | tail -3
```

Expected: 0 errors, 0 warnings.

- [ ] **Step 4: If anything is red, fix and commit**

If a test reveals a real bug in production code, do NOT silently fix it. Flag it in the implementer's report. Per project TDD discipline, the failing test IS the bug report.

If nothing is red, no commit needed — the prior five commits ship the value.

---

## Self-review notes

- **Spec coverage:** every section of the spec maps to a task.
  - TokenStore (12 tests) → Task 1.
  - tailscale (5 → 6) → Task 2.
  - suggested_label (7) → Task 3.
  - service_installer (2) → Task 4.
  - pairing generate_code (3) → Task 5.
  - Final verification → Task 6.
- **No production code touched.** Every task strictly appends a `#[cfg(test)] mod tests` block; no signatures or implementations change. If a test reveals a bug, that's escalation territory, not a silent fix.
- **No new deps.** `tempfile` is already in `crates/sharing/Cargo.toml` `[dev-dependencies]`. Confirmed during brainstorming.
- **`xml_escape` visibility:** private but the `#[cfg(test)] mod tests` in the same file has full visibility — confirmed by Rust's standard module rules.
- **Tests are deterministic** except for `generate_code_produces_distinct_outputs` (statistical sanity, threshold of 95/100 absorbs astronomically rare collisions) and `generate_code_covers_the_full_digit_range` (1000 draws make missing-digit failure ~ probability 10^-46). No flakes expected.
- **TokenStore tests rely on SQLCipher behavior** — open with key A and reading with key B fails. The test asserts either `open()` fails outright OR the first `list()` fails. Both are valid SQLCipher behaviors depending on the wrapper.
- **`pairing::generate_code` test #3 (full digit range):** assumes the RNG is uniform across `0..1_000_000`. The test asserts only that both leading-0 and leading-9 appear in 1000 draws; this is a sanity check, not a serious statistical test.

## Implementation order

Token store first (most valuable, longest). The other four can be tackled in any order; suggested order in the plan minimises cognitive switching cost (alphabetical-ish).
