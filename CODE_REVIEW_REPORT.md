# FerriScribe — Code Review Report

> **⚠️ DEPRECATED (v0.30+):** This report is from v0.24 and is kept for historical context only. All items have been resolved or are tracked in AGENTS.md "Known deferred debt". Do not use this as a current work list.

**Date:** 2026-06-16
**Reviewer:** Automated code review (OCR + manual analysis)
**Repo:** rustMedicalAssistant
**Scope:** All 13 workspace crates + src-tauri, focusing on security, robustness, and code quality

---

## How to use this report

Each finding has a severity, file location, description, and a suggested fix. Work through them in priority order. Items marked 🔴 should be fixed before any public release. Items marked 🟡 are robustness improvements. Items marked 🟢 are code quality / polish.

---

## 🔴 Critical / Security

### 1. SQL injection in plaintext→encrypted DB migration

**File:** `crates/db/src/encryption.rs`, lines 121–126
**Also at:** line 175 (table name interpolation in `verify_row_counts`)

```rust
// Current code (line 121-126):
let plaintext_str = db_path.to_string_lossy().replace('\'', "''");
enc.execute_batch(&format!(
    "ATTACH DATABASE '{plaintext_str}' AS plaintext KEY '';
     SELECT sqlcipher_export('main', 'plaintext');
     DETACH DATABASE plaintext;"
))

// Current code (line 175):
.query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |row| row.get(0))
```

**Problem:** Raw string interpolation into SQL. The path escaping (`replace('\'', "''")`) is fragile — `to_string_lossy()` can produce replacement characters and doesn't handle all SQL-special characters. Table names from `list_user_tables` are double-quoted but come from a query, not a whitelist.

**Risk:** Low today (paths are internally controlled), but one copy-paste of this pattern into user-facing code creates a real injection.

**Fix:**
- For ATTACH: Validate `db_path` contains only alphanumeric, `/`, `.`, `-`, `_` characters before interpolation. Add a comment warning not to use this pattern with user input.
- For table names: Add a whitelist check — verify each table name matches `^[a-zA-Z_][a-zA-Z0-9_]*$` before interpolation.
- Consider extracting a safe `quote_identifier()` helper.

---

### 2. Pairing code brute-force — no lockout or rate limiting

**File:** `crates/sharing/src/pairing.rs`, lines 102–115

```rust
pub async fn enroll(&self, submitted: &str, label: &str) -> Result<String> {
    let mut guard = self.active.lock().await;
    let active = guard.as_ref().ok_or(PairingError::InvalidCode)?.clone();
    if active.issued_at.elapsed() > self.ttl {
        *guard = None;
        return Err(PairingError::Expired);
    }
    if active.code != submitted {
        return Err(PairingError::InvalidCode);  // No attempt counting!
    }
    // ...
}
```

**Problem:** 6-digit code (1M space), 10-minute TTL, zero rate limiting on failed attempts. The `RateLimiter` in `crates/security/src/rate_limiter.rs` exists but isn't wired to the enrollment endpoint. An attacker on the same LAN could brute-force ~1000 codes/second.

**Fix:**
- Add an `attempt_count: AtomicU32` to `PairingState`.
- Increment on each failed `enroll()` call.
- Lock out after 5 failed attempts (return `PairingError::LockedOut`).
- Reset on `issue_code()`.
- Alternatively, add a per-IP rate limiter on the HTTP pairing endpoint.

---

### 3. Whisper binary download skips SHA-256 verification

**File:** `crates/sharing/src/whisper_supervisor.rs`, lines 196–206

```rust
if let Some(expected) = expected_sha256 {
    let got = hex::encode(Sha256::digest(&bytes));
    if got != expected {
        return Err(WhisperError::HashMismatch { ... });
    }
} else {
    warn!("sha256 not set for binary {}; skipping verification", binary_name);
    // Continues without verification!
}
```

**Problem:** If the manifest entry lacks `sha256`, the downloaded binary is extracted and executed without any integrity check. A compromised manifest or MITM on the download path would execute arbitrary code.

**Fix:**
- Make `sha256` a required field in the `BinaryEntry` struct (remove `Option`).
- Or: refuse to download when `sha256` is `None`, returning `WhisperError::Manifest("sha256 required")`.
- Update `whisper-manifest.json` to ensure all entries have SHA-256 hashes.

---

### 4. Auth proxy buffers full request body in memory (up to 256 MiB), then clones it

**File:** `crates/sharing/src/auth_proxy.rs`, lines 180–195

```rust
const MAX_BODY_BYTES: usize = 256 * 1024 * 1024; // 256 MiB
let body_bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
    .await
    .map_err(|_| (StatusCode::PAYLOAD_TOO_LARGE, "").into_response())?;
// ...
.body(body_bytes.clone())  // Full clone of up to 256 MiB!
```

**Problem:** The entire request body is buffered into memory, then cloned before forwarding. For a 60-minute Whisper transcription at 16 kHz mono WAV (~190 MiB), peak memory is ~380 MiB per request. Multiple concurrent clinicians could exhaust server RAM.

**Fix:**
- Stream the body to the upstream instead of buffering. Use `reqwest::Body::wrap_stream()` or pipe axum's body stream directly to reqwest.
- If streaming is too complex for the initial fix, at least remove the `.clone()` — `body_bytes` is only used once after this point, so consume it directly (halves peak memory).
- Consider reducing `MAX_BODY_BYTES` to a more reasonable limit (e.g. 100 MiB for audio).

---

## 🟡 Robustness

### 5. HTML sanitizer bypass on crafted input

**File:** `crates/security/src/input_sanitizer.rs`, line 13

```rust
static ref HTML_TAG: Regex = Regex::new(r"<[^>]+>").expect("invalid HTML tag regex");
```

**Problem:** The regex `<[^>]+>` fails on crafted input like `<script src="x" title=">" onclick="alert(1)">` — the `>` inside the attribute terminates the regex early, leaving the `onclick` handler intact. The docstring acknowledges this is not a full parser.

**Fix:** Use a proper HTML sanitizer crate like `ammonia` or `bleach` (if available in Rust), or at minimum apply the regex in a loop until no more matches are found (catches nested cases). For the current threat model (user pasting snippets), this is low priority but worth hardening.

---

### 6. SSE stream silently drops malformed JSON

**File:** `crates/ai-providers/src/openai_compat/methods.rs`, lines 188–189

```rust
Ok(data) => {
    match serde_json::from_str::<ChatResponse>(&data) {
        Err(_) => vec![],  // silently dropped!
        Ok(resp) => { ... }
    }
}
```

**Problem:** Corrupted or truncated SSE events are silently ignored. In a medical context, silently losing parts of an AI response could lead to incomplete SOAP notes without the clinician noticing.

**Fix:** Add a `warn!` log on parse failures:
```rust
Err(e) => {
    warn!(error = %e, data_len = data.len(), "dropping malformed SSE event");
    vec![]
}
```
Consider also surfacing a "partial response" warning to the UI if >N events are dropped.

---

### 7. Auth proxy has no upstream request timeout

**File:** `crates/sharing/src/auth_proxy.rs`, lines 85–91

```rust
let client = Client::builder()
    .pool_max_idle_per_host(8)
    .connect_timeout(std::time::Duration::from_secs(10))
    // No overall timeout — Ollama generations can be arbitrarily long
    .build()
```

**Problem:** A hung upstream (Ollama crash, whisper-server hang) ties up the proxy connection indefinitely. Combined with the 256 MiB body buffer, this is a slow resource exhaustion vector.

**Fix:** Add a generous but bounded overall timeout (e.g. 30 minutes) or implement a progress/heartbeat check. The `remote_provider.rs` already uses `TRANSCRIBE_TIMEOUT = 600s` — apply a similar bound at the proxy level.

---

### 8. Vocabulary corrector recompiles all regexes per call

**File:** `crates/processing/src/vocabulary_corrector.rs`, line 61

```rust
let mut cache: HashMap<(String, bool), Option<Regex>> = HashMap::new();
```

**Problem:** The regex cache is created fresh inside each `apply_corrections()` call. For a vocabulary of 100+ entries, this means compiling 100+ regex patterns on every transcription. This is on the hot path (post-STT, before display).

**Fix:** Move the cache to a persistent structure. Options:
- `LazyLock<HashMap<...>>` if the vocabulary is static.
- `Arc<Mutex<HashMap<...>>>` passed in as a parameter.
- Pre-compile patterns at vocabulary load time and store `Vec<(Regex, &VocabularyEntry)>`.

---

### 9. SQLCipher migration doesn't fsync before rename

**File:** `crates/db/src/encryption.rs`, line 134

```rust
std::fs::rename(&encrypting_path, db_path)
```

**Problem:** No `fsync` on the encrypted file before the atomic rename. On a crash between rename and backup deletion, the renamed file could be incomplete on some filesystems (ext4 with delayed allocation).

**Fix:** Open the encrypted file and call `file.sync_all()` before the rename:
```rust
let f = std::fs::File::open(&encrypting_path)?;
f.sync_all()?;
std::fs::rename(&encrypting_path, db_path)?;
```

---

### 10. `strip_html` doesn't handle HTML entities

**File:** `crates/security/src/input_sanitizer.rs`

**Problem:** `strip_html` removes tags but leaves HTML entities intact (`&amp;`, `&lt;`, `&#x27;`, etc.). If the stripped text is rendered in a context that decodes entities, this could be a secondary injection vector.

**Fix:** After stripping tags, run a basic entity decode pass (`&amp;` → `&`, `&lt;` → `<`, `&gt;` → `>`, `&#x27;` → `'`, `&quot;` → `"`). Use a crate like `htmlescape` or a simple lookup table.

---

## 🟢 Code Quality

### 11. Unnecessary body clone in auth proxy

**File:** `crates/sharing/src/auth_proxy.rs`, line 195

```rust
.body(body_bytes.clone())
```

**Fix:** Remove `.clone()` — `body_bytes` is consumed once and never used again. This halves peak memory per request.

---

### 12. `SharingError` loses structured context

**File:** `crates/sharing/src/lib.rs`, lines 62–79

All `SharingError` variants except `Io` use `String` payloads (`.to_string()`), losing the ability to programmatically branch on error types downstream.

**Fix:** Consider wrapping concrete error types (e.g., `TokenStore(#[from] TokenStoreError)`) instead of converting to strings.

---

### 13. Redundant macOS machine-id parser logic

**File:** `crates/security/src/machine_id.rs`, lines 86–103

The `rfind('"')` approach and the `split('"').collect()` approach are both present — the second is a fallback for the same case the first handles. Dead code.

**Fix:** Remove one of the two approaches. The `split('"')` approach is simpler and handles all cases.

---

### 14. `allow_public` not runtime-invalidated

**File:** `crates/stt-providers/src/remote_provider.rs`, `crates/ai-providers/src/ollama.rs`

The `allow_public` bool is read from `AppConfig` at provider construction time. If the user changes the setting in the UI, the provider must be reconstructed — there's no runtime invalidation path.

**Fix:** Document this behavior, or add a `set_allow_public()` method that re-validates the current endpoint against the new policy.

---

### 15. Pairing code comparison is not constant-time

**File:** `crates/sharing/src/pairing.rs`, line 109

```rust
if active.code != submitted {
    return Err(PairingError::InvalidCode);
}
```

**Problem:** String comparison short-circuits on first mismatch byte. While the 6-digit code space is small enough that timing attacks are impractical, constant-time comparison is a security best practice.

**Fix:** Use `constant_time_eq` or `subtle` crate for comparison, or hash both sides before comparing.

---

## Summary

| Severity | Count | Effort |
|---|---|---|
| 🔴 Critical/Security | 4 | Medium |
| 🟡 Robustness | 6 | Low–Medium |
| 🟢 Code Quality | 5 | Low |

## Recommended fix order

1. **#2 — Pairing brute-force lockout** (immediate exploit risk, small change)
2. **#3 — SHA-256 verification mandatory** (one-line fix, big security win)
3. **#11 + #4 — Auth proxy body clone removal** (halves memory, trivial change)
4. **#1 — SQL injection hardening** (defensive, small change)
5. **#8 — Vocabulary regex cache** (performance on hot path)
6. **#6 — SSE parse failure logging** (observability)
7. **#9 — fsync before rename** (data safety)
8. Remaining items as time permits

## Testing notes

- After fixing #2, test the pairing flow with >5 wrong codes to verify lockout.
- After fixing #3, test with a manifest that has `sha256: null` to verify the download is rejected.
- After fixing #4, monitor memory usage during a 30+ minute transcription with multiple clients.
- After fixing #8, benchmark `apply_corrections` with a 200+ entry vocabulary before/after.
