# `medical-security` — Encrypted Key Storage, PHI Redaction & Safety Primitives

`medical-security` (~1,310 LOC) is the HIPAA-compliance backstop for FerriScribe.
It owns every operation that touches secrets or patient-identifiable information
at the boundary between "trusted in-process memory" and "anything persistent or
observable" (disk files, log sinks, export archives).

---

## How It Fits in the Workspace

```
medical-core ─────────► medical-security ◄──── medical-db (no dep)
                              │
                 ┌────────────┼────────────┐
                 ▼            ▼            ▼
           sharing       src-tauri     corpus_export
         (auth proxy   (settings UI,  (training-corpus
          tokens)       API key mgmt,  PHI scrubbing)
                        DB encryption)
```

- **Depends on:** `medical-core` (currently only for shared error conventions;
  the coupling is intentionally thin so this crate stays close to standalone).
- **Used by:**
  - `src-tauri` — stores/retrieves remote STT, Ollama and LM Studio API keys
    via `KeyStorage`; derives the SQLCipher encryption key via the `keychain`
    module; wipes keys during the "Wipe and start fresh" recovery path; redacts
    PHI from training-corpus exports.
  - `sharing` — reuses the SQLCipher keychain entry as the sharing-store
    encryption key so there is only one secret to manage per install.
  - `src-tauri/corpus_export` — builds per-recording `Extension`s (patient name,
    datetime) and runs `PhiRedactor::redact_with` over every transcript pair
    before writing the JSONL corpus.

---

## Module Map

| Module | Purpose |
|---|---|
| `key_storage` | AES-256-GCM encrypted key store (JSON on disk, PBKDF2 master key) |
| `keychain` | Cross-platform OS keychain wrapper for the SQLCipher DB key |
| `machine_id` | Stable per-machine identifier used as a key-derivation password |
| `phi_redactor` | Regex-based PHI/PII redaction with per-recording extensions |
| `audit_logger` | Thin wrapper that runs `PhiRedactor` over log payloads |
| `input_sanitizer` | HTML stripping and UTF-8-safe truncation |
| `rate_limiter` | In-process token-bucket limiter (requests/minute) |

---

## Key Types

- **`KeyStorage`** (`key_storage`) — encrypts API keys with AES-256-GCM, a
  per-entry random 12-byte nonce, and a master cipher key derived via
  PBKDF2-HMAC-SHA256 (600,000 iterations). Keys are stored as JSON keyed by
  provider name.
- **`PhiRedactor`** (`phi_redactor`) — stateless struct whose associated
  functions (`redact`, `contains_phi`, and the `_with` variants) run a fixed
  regex pipeline (SSN → PHONE → EMAIL → DOB → MRN → ADDRESS → ZIP).
- **`Extension`** (`phi_redactor`) — compiled per-recording pattern
  (e.g. patient name, datetime) that runs *before* the static patterns so
  "John Smith" is replaced before the EMAIL regex can match an email
  containing "smith".
- **`SecurityError` / `SecurityResult<T>`** — crate-wide error enum, with the
  notable `MasterKeyUnavailable { reason }` variant that signals the
  "neither env var nor machine ID worked" bootstrap failure.
- **`KeychainError`** (`keychain`) — separate error enum for OS keychain
  access problems (kept distinct so Tauri recovery paths can pattern-match
  on the failure mode).

---

## How Master-Key Derivation Works

```
┌──────────────────────────────────────────────────────┐
│                 KeyStorage::open(config_dir)         │
│                                                      │
│  1. load_or_create_salt(config_dir)                  │
│       └─ reads  salt.bin  OR  writes 32 random bytes │
│                                                      │
│  2. derive_master_key(salt)                          │
│       password = env("MEDICAL_ASSISTANT_MASTER_KEY") │
│                  ?? machine_id::get_machine_id()     │
│       key      = PBKDF2-HMAC-SHA256(                 │
│                     password, salt,                  │
│                     600_000 rounds, 32 bytes)        │
│                                                      │
│  3. Aes256Gcm::new(&key)                             │
└──────────────────────────────────────────────────────┘
```

The env var override exists for **CI and headless testing** — production
installations always fall through to `machine_id`, which hashes a platform-
specific hardware identifier (IOPlatformUUID on macOS, MachineGuid on Windows,
`/etc/machine-id` on Linux) with SHA-256. A username+home fallback is used
only when the platform-specific lookup fails.

Each `store_key` call generates a fresh 12-byte nonce from `OsRng`, encrypts
the plaintext, and writes `base64(nonce || ciphertext)` to disk. This means
the *same* API key stored twice yields different ciphertexts — important
because the JSON file is world-readable on some filesystems and we do not
want equality of ciphertexts to leak equality of keys.

---

## Examples

### Storing and retrieving an API key (as `src-tauri` does)

```rust
use medical_security::key_storage::KeyStorage;
use std::path::PathBuf;

let config_dir: PathBuf = /* data_dir.join("config") */ todo!();
let keys = KeyStorage::open(&config_dir)?;

// User pastes a new API key in Settings → AI Providers
keys.store_key("openai", "sk-...")?;

// Later: a provider module needs the key to build a request
let maybe_key: Option<String> = keys.get_key("openai")?;
```

`src-tauri` holds a single `Arc<KeyStorage>` in `AppState` (field `keys`) and
reuses it across Tauri commands: `get_api_key`, `set_api_key`,
`list_api_keys`, and the sharing-pairing autofill path (which writes the
paired bearer token into the `stt_remote_api_key`, `ollama_api_key`, and
`lmstudio_api_key` slots in one pass).

### Redacting PHI from a training-corpus export

```rust
use medical_security::phi_redactor::{PhiRedactor, Extension};
use medical_security::phi_redactor::names::build_patient_name_extension;
use medical_security::phi_redactor::datetime::build_datetime_extension;

let mut extensions: Vec<Extension> = Vec::new();
if let Some(ext) = build_patient_name_extension("Jane Smith") {
    extensions.push(ext);
}
extensions.push(build_datetime_extension());

let scrubbed = PhiRedactor::redact_with(transcript, &extensions);
assert!(!PhiRedactor::contains_phi_with(&scrubbed, &extensions));
```

The export pipeline in `src-tauri/src/corpus_export/mod.rs` runs this on
every `(user_input, final_text)` pair and emits a manifest warning when
residual PHI is detected after redaction — a defense-in-depth check in case
a new PHI shape slips past the regex set.

---

## Cross-Crate Contracts

- **Sharing token auth.** The sharing auth proxy
  (`crates/sharing/src/auth_proxy.rs`) returns HTTP 401 with an
  `x-auth-reason: unknown-token` header when the presented bearer is not in
  the token store. The STT provider client (`crates/stt-providers/src/client.rs`)
  pattern-matches on that header to surface a "please re-pair" message
  instead of a generic auth failure. The header name and value are a contract
  between those two crates — do not change one without the other.
- **Keychain slot names.** The keychain uses service
  `rustMedicalAssistant` / account `db-key` for the SQLCipher encryption key.
  `KeyStorage` uses a different mechanism (its own JSON file under
  `config_dir`) — do not conflate the two. The sharing crate intentionally
  reuses the DB keychain entry rather than minting a second secret.
- **Provider slot names.** `KeyStorage` keys are free-form strings chosen by
  callers: `stt_remote_api_key`, `openai`, `anthropic`, `ollama_api_key`,
  `lmstudio_api_key`, etc. Renaming a slot silently orphans the previously
  stored key.

---

## Gotchas

1. **Losing the master key is unrecoverable.** If the user's machine ID
   changes (hardware replacement, OS reinstall, VM cloning) *and* the
   `MEDICAL_ASSISTANT_MASTER_KEY` env var is not set, the ciphertext in
   `keys.json` cannot be decrypted. There is deliberately no backdoor or
   reset path — the user must re-enter their API keys. Document this in
   any user-facing support flow.
2. **Salt file = the keystore's identity.** `salt.bin` must persist next to
   `keys.json`. Deleting it causes `KeyStorage::open` to generate a fresh
   salt, after which every previously stored key returns
   `SecurityError::Decryption` (GCM auth-tag mismatch). Backups must include
   both files.
3. **PHI regex ordering matters.** Patterns are applied in a fixed order
   (SSN → PHONE → EMAIL → DOB → MRN → ADDRESS → ZIP). The SSN and MRN
   patterns require a keyword prefix to avoid false positives on lab values
   and reference numbers; the ZIP pattern requires either a `zip`/`zip code`
   keyword or a two-letter US state abbreviation. Adding a new pattern that
   matches 9-digit numbers without a keyword prefix will redact lab values.
4. **Extensions run *before* static patterns.** This is intentional —
   patient-name extensions must catch "John Smith" before the EMAIL regex
   matches `john.smith@example.com` with the name still in it. If you add a
   new extension, test that it does not shadow a static pattern you wanted
   to fire.
5. **`AuditLogger::redact_for_log` is a thin wrapper.** It delegates to
   `PhiRedactor::redact` with no extensions. If you need per-call
   extensions in a log path, call `PhiRedactor::redact_with` directly and
   log the result.
6. **`RateLimiter` is `!Sync`.** It holds an `Instant` and a `f64` token
   count with interior mutation on `try_acquire`. Wrap it in a `Mutex` if
   you need to share it across tasks.
7. **Keychain mock in tests.** The `keyring` v3 mock backend is
   `EntryOnly` — every `Entry::new()` returns a fresh empty credential.
   Cross-call persistence cannot be tested at the unit level; real
   persistence is covered by integration tests and manual smoke testing on
   each platform.

---

## Testing

```bash
cargo test -p medical-security --lib
```

Tests use `tempfile::TempDir` for `KeyStorage` and the `keyring` mock
backend for `keychain`, so they do not touch the real OS keychain or the
user's config directory.
