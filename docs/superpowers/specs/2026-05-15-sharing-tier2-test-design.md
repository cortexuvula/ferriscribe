# Sharing Crate Test Backfill — Tier 2 Design

**Status:** Draft — pending user review
**Date:** 2026-05-15
**Author:** Brainstorming session with Claude Code

## Goal

Continue the sharing-crate test backfill into the two HTTP-bound modules: `auth_proxy.rs` (bearer-validated reverse proxy) and `whisper_supervisor.rs` (on-demand binary download + SHA256 verification). Use `wiremock` as the upstream/fake-server backend. Add 13 tests; ship one small `pub(crate)` extraction in `whisper_supervisor` so the SHA256-mismatch path the audit specifically called out becomes injectable.

## Problem

Tier 1 (commit `9174c8b`) brought the security-critical `TokenStore` to 13 tests and covered four pure-function helpers. Two modules at the office-server boundary are still untested:

- `auth_proxy::spawn_auth_proxy` — the bearer-auth gate between the LAN/Tailscale client and Ollama / whisper.cpp. Misbehavior here means either request rejection on valid bearers (downtime) or request acceptance on invalid bearers (security incident).
- `whisper_supervisor::ensure_binary` — downloads whisper-server prebuilt binaries on demand and verifies SHA256. The audit specifically called out the hash-verification path as worth testing; a supply-chain compromise on the GitHub asset would land here.

Together these two modules carry 505 LOC of zero-test surface.

## Non-goals

- **`supervise()` / `spawn_once_at()` / `start()` / `stop()`** — these manage a real subprocess. Would need a fake whisper-server binary, a process-mock layer, or a real CI runner with whisper installed. Defer to Tier 3.
- **`platform_key()`** — compile-time constants; the only assertion is "the current platform's key matches expectations," which is trivial. Skip.
- **`auth_proxy` 413 PAYLOAD_TOO_LARGE** — the limit is 256 MiB; constructing a body that large in a test is wasteful. The body-limit code path is one line (`axum::body::to_bytes` with `MAX_BODY_BYTES`); spot-check by reading. Skip the test.
- **Production behavior change in `whisper_supervisor`** — the refactor extracts a `pub(crate)` helper from `ensure_binary`; observable behavior of `ensure_binary` is identical.

## Hard constraints honored

- **No new workspace deps.** `wiremock` is already a workspace dep (`Cargo.toml:66`) used by `stt-providers` and `ai-providers`. Only add it to `crates/sharing/Cargo.toml`'s `[dev-dependencies]`.
- **No PHI in logs.** Test-only; no `tracing::*` introduced. The auth_proxy `warn!` lines already log only `error`/`client_id` — no token strings, no body content.
- **No telemetry.** Tests hit wiremock on `127.0.0.1`; no real network.

## Decisions captured

| Question | Choice |
|---|---|
| Scope | auth_proxy + whisper_supervisor (Tier 2 of the original 3-tier audit response) |
| `whisper_supervisor` testing strategy | Small refactor: extract `download_and_verify` as `pub(crate)` so wiremock can inject a fake binary URL |
| Test infrastructure | `wiremock` (real HTTP mock) + `tempfile::tempdir()` + real `TokenStore` |
| Production code refactor | One extraction in `whisper_supervisor.rs`; no observable behavior change |

## Refactor

`crates/sharing/src/whisper_supervisor.rs` — split `ensure_binary` so the download + verify + extract chain is callable with an arbitrary URL:

```rust
pub async fn ensure_binary(&self) -> Result<PathBuf> {
    let manifest: Manifest =
        serde_json::from_str(MANIFEST).map_err(|e| WhisperError::Manifest(e.to_string()))?;
    let key = platform_key();
    let entry = manifest
        .binaries
        .get(key)
        .ok_or(WhisperError::UnsupportedPlatform)?;
    let url = entry.url.as_deref().ok_or(WhisperError::UnsupportedPlatform)?;
    let archive = entry.archive.as_deref().ok_or(WhisperError::UnsupportedPlatform)?;

    let bin_path = self.binary_dir.join(&entry.binary_name);
    let lock_path = self.binary_dir.join(".whisper-manifest-version");

    // (existing cached-binary check unchanged — return early if cache hit)
    if bin_path.exists() {
        let cached = tokio::fs::read_to_string(&lock_path)
            .await
            .ok()
            .map(|s| s.trim().to_string());
        if cached.as_deref() == Some(manifest.version.trim()) {
            return Ok(bin_path);
        }
        warn!(
            "cached whisper-server was installed from manifest version {:?}; current is {:?}; replacing",
            cached.as_deref().unwrap_or("(none)"),
            manifest.version
        );
        let _ = tokio::fs::remove_file(&bin_path).await;
        let _ = tokio::fs::remove_file(&lock_path).await;
    }

    let bin_path = self
        .download_and_verify(url, archive, entry.sha256.as_deref(), &entry.binary_name)
        .await?;

    let _ = tokio::fs::write(&lock_path, manifest.version.trim()).await;
    Ok(bin_path)
}

pub(crate) async fn download_and_verify(
    &self,
    url: &str,
    archive: &str,
    expected_sha256: Option<&str>,
    binary_name: &str,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(&self.binary_dir).await?;
    let bytes = reqwest::get(url)
        .await
        .map_err(|e| WhisperError::Download(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| WhisperError::Download(e.to_string()))?;
    if let Some(expected) = expected_sha256 {
        let got = hex::encode(Sha256::digest(&bytes));
        if got != expected {
            return Err(WhisperError::HashMismatch {
                expected: expected.to_string(),
                got,
            });
        }
    } else {
        warn!("sha256 not set for binary {}; skipping verification", binary_name);
    }
    Self::extract_archive(&bytes, archive, &self.binary_dir, binary_name)?;
    let bin_path = self.binary_dir.join(binary_name);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&bin_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms)?;
    }
    Ok(bin_path)
}
```

Observable changes from `master`:
- `ensure_binary` still does the same work; just delegates the inner chain.
- The lock-file write moves from "inside download" to "after download succeeds, in `ensure_binary`." Semantically identical (both run only on the success path).
- The `warn!` for missing sha256 changes slightly (different message format) — fine, it's a log line, not part of behavior.

No public API change. `download_and_verify` is `pub(crate)` so unit tests in the same crate can call it.

## Test plan

### `auth_proxy.rs` — 6 tests

Common scaffolding inside `#[cfg(test)] mod tests`:

```rust
async fn setup() -> (
    std::sync::Arc<crate::token_store::TokenStore>,
    wiremock::MockServer,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = std::sync::Arc::new(
        crate::token_store::TokenStore::open(dir.path().join("tokens.db"), &[7u8; 32])
            .expect("open store"),
    );
    let upstream = wiremock::MockServer::start().await;
    (store, upstream, dir)
}

async fn spawn_test_proxy(
    store: std::sync::Arc<crate::token_store::TokenStore>,
    backend_url: String,
    inject_api_key: Option<String>,
) -> (u16, tokio::task::JoinHandle<()>) {
    // bind on port 0 — kernel chooses an ephemeral free port
    let cfg = ProxyConfig {
        listen_port: 0,
        backend_url,
        path_prefix: "/".into(),
        inject_api_key,
    };
    // … (the current spawn_auth_proxy binds to 0.0.0.0:listen_port; to discover
    // the actual port when listen_port = 0, the test needs the bound port back.
    // Easiest path: call tokio::net::TcpListener::bind directly with port 0,
    // grab .local_addr().port(), then construct an alternate spawn helper that
    // accepts a pre-bound listener. See implementation note below.)
}
```

**Implementation note for port 0:** `spawn_auth_proxy` currently does `TcpListener::bind((0.0.0.0, config.listen_port))` internally and discards the result. To capture the kernel-assigned port, the tests need either:
- (a) An ephemeral-port helper that binds to `127.0.0.1:0`, captures the port, drops the listener, then calls `spawn_auth_proxy(ProxyConfig { listen_port, ...}, store)`. Race condition (port may be re-used before bind) but rare.
- (b) Refactor `spawn_auth_proxy` to optionally accept a pre-bound listener.

Recommend (a) — it's a 3-line pattern that the `stt-providers` tests already use; no refactor needed.

| # | Test | Setup | Assert |
|---|---|---|---|
| 1 | `proxy_401_missing_bearer` | Issue token, start proxy → backend wiremock | GET without `Authorization` → 401 + `x-auth-reason: missing-bearer` |
| 2 | `proxy_401_unknown_token` | Issue token A, start proxy | GET with `Authorization: Bearer not-a-real-token` → 401 + `x-auth-reason: unknown-token` |
| 3 | `proxy_401_revoked_token` | Issue token A → revoke A → start proxy | GET with `Authorization: Bearer <token-A>` → 401 + `x-auth-reason: unknown-token` |
| 4 | `proxy_200_proxies_to_backend_on_valid_bearer` | Issue, wiremock returns 200 body `"ok-body"` | GET with valid bearer → 200, body = `"ok-body"`. Verify `store.list()[0].last_seen_at.is_some()` (touch fired). |
| 5 | `proxy_strips_client_bearer_and_injects_api_key` | wiremock `Mock::given(method).and(matchers::header("authorization", "Bearer server-secret"))` | Client sends `Authorization: Bearer <client-token>`; proxy must forward `Authorization: Bearer server-secret` instead |
| 6 | `proxy_502_when_backend_unreachable` | Set `backend_url` to `http://127.0.0.1:1` (port 1, almost certainly unbound) | GET with valid bearer → 502 BAD_GATEWAY |

### `whisper_supervisor.rs` — 7 tests

Helpers needed:

```rust
fn build_zip_with(binary_name: &str, body: &[u8]) -> Vec<u8> {
    use zip::write::FileOptions;
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut w = zip::ZipWriter::new(&mut buf);
    w.start_file(binary_name, FileOptions::<()>::default())
        .expect("start_file");
    std::io::Write::write_all(&mut w, body).expect("write");
    w.finish().expect("finish");
    buf.into_inner()
}

fn build_tar_gz_with(binary_name: &str, body: &[u8]) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let gz = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = tar::Builder::new(gz);
    let mut header = tar::Header::new_gnu();
    header.set_path(binary_name).expect("set_path");
    header.set_size(body.len() as u64);
    header.set_cksum();
    tar.append(&header, body).expect("append");
    let gz = tar.into_inner().expect("into_inner");
    gz.finish().expect("finish")
}
```

| # | Test | Strategy |
|---|---|---|
| 1 | `extract_zip_extracts_named_binary` | Build a zip with `["other.txt", "whisper-server"]`; assert `extract_zip(&bytes, &out_dir, "whisper-server")` produces `out_dir/whisper-server` with the right body |
| 2 | `extract_zip_errors_when_binary_missing` | Build a zip with only `"other.txt"`; assert `Err(WhisperError::Manifest)` |
| 3 | `extract_tar_gz_extracts_named_binary` | Build a tar.gz with `whisper-server`; assert extraction |
| 4 | `extract_tar_gz_errors_when_binary_missing` | tar.gz without the binary → `Err(WhisperError::Manifest)` |
| 5 | `extract_archive_unknown_kind_errors` | Call `WhisperSupervisor::extract_archive(b"", "rar", &dir, "x")` → `Err(WhisperError::Manifest("unsupported archive: rar"))` (Note: `extract_archive` is a private static method on the type; tests in the same module have access.) |
| 6 | `download_and_verify_succeeds_with_correct_sha256` | wiremock serves a `zip` body; compute `sha256(bytes)` in the test, pass as `expected_sha256`; assert `Ok(path)` and the file exists |
| 7 | `download_and_verify_rejects_hash_mismatch` | wiremock serves the zip; pass `expected_sha256 = "0000...0000"` (deliberately wrong); assert `Err(WhisperError::HashMismatch { expected, got })` where `expected == "0000...0000"` and `got` is the actual hash |

For tests 6 and 7, the helper accepts an empty-body file as the "binary" — content doesn't matter because the test asserts the SHA path, not the binary's executability. Use `b"fake-binary-content"` for body.

## Architecture overview

```
crates/sharing/
├── Cargo.toml                MODIFIED — add wiremock to [dev-dependencies]
└── src/
    ├── auth_proxy.rs         MODIFIED — append `#[cfg(test)] mod tests` (6 tests + helpers)
    └── whisper_supervisor.rs MODIFIED — extract `download_and_verify` + append `#[cfg(test)] mod tests` (7 tests + 2 helpers)
```

## Data flow

Not applicable — test-only batch + one internal extraction.

## Error handling

| Scenario | Behavior |
|---|---|
| wiremock not running | `tokio::net::TcpListener::bind` in wiremock setup fails; test panics. Expected — surfaces broken test harness. |
| Port 0 race condition | The 3-line bind→port→drop→spawn pattern has a window where the port might be reused. Tests run sequentially within a crate by default; the race is theoretical, not practical. |
| Zip / tar.gz build helpers fail | Test panics with `expect("..")` — surfaces test infrastructure breakage. |
| `download_and_verify` test sees a real download (network ON in CI) | Tests use wiremock URL; no real network reachable. CI sandboxing irrelevant. |

## Testing

```bash
cargo test -p medical-sharing --lib
```

Expected: 34 (Tier 1) + 13 (Tier 2) = **47 total** in `medical-sharing`.

```bash
cargo test --workspace --lib
```

Expected: all 14 cargo lib suites still `ok`, no regressions.

## Open questions

None blocking.

**Future iterations (Tier 3):**
- `WhisperSupervisor::start` / `supervise` / `spawn_once_at` / `stop` — process supervision; needs a fake whisper-server binary or a syscall mocking layer.
- `orchestrator::SharingService` integration tests — composes everything; needs a fixture harness with a real `TokenStore`, an in-process `auth_proxy` listener, and a stubbed `WhisperSupervisor`.
- Per-platform `install_persistent_ollama` (launchctl / systemctl / sc.exe) — would need a syscall trait.

## Implementation order

1. **Add `wiremock` to `crates/sharing/Cargo.toml` `[dev-dependencies]`.** Trivial.
2. **`auth_proxy` tests** — 6 tests with the wiremock + TokenStore + ephemeral-port pattern. Each test is independent.
3. **`whisper_supervisor` refactor** — extract `download_and_verify` from `ensure_binary`. Run the existing 0 tests (no regression possible since the module is currently untested) and confirm `ensure_binary` still compiles.
4. **`whisper_supervisor` tests** — 7 tests using the zip/tar.gz builder helpers + wiremock.

Each commit is one focused change. The refactor lands as its own commit before the tests so test failures can be attributed unambiguously.
