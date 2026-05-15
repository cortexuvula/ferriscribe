# Sharing Crate Test Backfill — Tier 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add 13 unit tests across `auth_proxy.rs` (6) and `whisper_supervisor.rs` (7) — the two HTTP-bound modules in `medical-sharing` that Tier 1 deferred. Plus one small `pub(crate)` extraction in `whisper_supervisor.rs` to make the SHA256-verify path testable.

**Architecture:** Add `wiremock` to dev-deps. Each module gets a `#[cfg(test)] mod tests` block. `auth_proxy` tests use a real `TokenStore` (via `tempfile::tempdir()`) plus a `MockServer` as the upstream backend, and discover an ephemeral port via the bind→port→drop→spawn pattern. `whisper_supervisor` gets a `download_and_verify` helper carved out so wiremock can serve fake binary archives.

**Tech Stack:** Rust 2024, `wiremock = "0.6"` (workspace dep), `tempfile`, `reqwest` (runtime dep, re-used in tests), `tokio::test`. `zip`, `flate2`, `tar` are already runtime deps accessible from tests.

**Spec:** [`docs/superpowers/specs/2026-05-15-sharing-tier2-test-design.md`](../specs/2026-05-15-sharing-tier2-test-design.md)

---

## File Structure

**Modified files:**
- `crates/sharing/Cargo.toml` — add `wiremock` to `[dev-dependencies]`
- `crates/sharing/src/auth_proxy.rs` — append `#[cfg(test)] mod tests` (6 tests)
- `crates/sharing/src/whisper_supervisor.rs` — extract `download_and_verify` helper + append `#[cfg(test)] mod tests` (7 tests)

**Worktree:** Use `superpowers:using-git-worktrees` to create `.worktrees/sharing-tier2-tests` from `master` at the spec commit (`53440c7`).

---

## Task 1: Add `wiremock` to dev-dependencies

**Files:**
- Modify: `crates/sharing/Cargo.toml`

- [ ] **Step 1: Append wiremock to `[dev-dependencies]`**

In `crates/sharing/Cargo.toml`, the `[dev-dependencies]` section is currently lines 36-38:

```toml
[dev-dependencies]
tempfile = { workspace = true }
tokio = { workspace = true }
```

Add `wiremock` so the section reads:

```toml
[dev-dependencies]
tempfile = { workspace = true }
tokio = { workspace = true }
wiremock = { workspace = true }
```

- [ ] **Step 2: Verify dep resolves**

Run:

```bash
cargo build -p medical-sharing --tests
```

Expected: compiles cleanly. No new tests yet, so nothing should run.

- [ ] **Step 3: Commit**

```bash
git add crates/sharing/Cargo.toml
git commit -m "chore(sharing): add wiremock to dev-dependencies"
```

---

## Task 2: `auth_proxy.rs` — 6 wiremock-backed integration tests

**Files:**
- Modify: `crates/sharing/src/auth_proxy.rs` (append `#[cfg(test)] mod tests` block)

The tests bind a TCP listener on an ephemeral port, read the port number, drop the listener, then call `spawn_auth_proxy(ProxyConfig { listen_port, ...}, store)` with that port. There is a microsecond-wide race window where another process could re-grab the port between drop and bind; in practice this never happens in CI but the test's panic message would point at "Address in use." Acceptable.

- [ ] **Step 1: Append the test module**

Append the following to `crates/sharing/src/auth_proxy.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_store::TokenStore;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use wiremock::matchers::{header, method as http_method, path as http_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Open a TokenStore in a tempdir and return both (store, tempdir-guard).
    fn fresh_store() -> (Arc<TokenStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.db");
        let store = Arc::new(
            TokenStore::open(&path, &[7u8; 32]).expect("open token store"),
        );
        (store, dir)
    }

    /// Bind on 127.0.0.1:0, capture the kernel-assigned port, drop the
    /// listener so spawn_auth_proxy can re-bind. Rare race with other
    /// processes for the same port; acceptable for tests.
    async fn ephemeral_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);
        port
    }

    /// Start the proxy and return (port, JoinHandle). The TempDir from
    /// fresh_store() must outlive the test — keep it bound.
    async fn spawn_test_proxy(
        store: Arc<TokenStore>,
        backend_url: String,
        inject_api_key: Option<String>,
    ) -> (u16, tokio::task::JoinHandle<()>) {
        let port = ephemeral_port().await;
        let cfg = ProxyConfig {
            listen_port: port,
            backend_url,
            path_prefix: "/".into(),
            inject_api_key,
        };
        let handle = spawn_auth_proxy(cfg, store).await.expect("spawn");
        // Give the spawned task a moment to begin serving. axum::serve
        // is ready immediately after spawn but on slow CI a 50ms warmup
        // avoids flakes.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (port, handle)
    }

    #[tokio::test]
    async fn proxy_401_missing_bearer() {
        let (store, _dir) = fresh_store();
        let upstream = MockServer::start().await;
        let (port, handle) = spawn_test_proxy(store, upstream.uri(), None).await;

        let resp = reqwest::get(&format!("http://127.0.0.1:{port}/anything"))
            .await
            .expect("send");
        assert_eq!(resp.status(), 401);
        assert_eq!(
            resp.headers().get("x-auth-reason").and_then(|v| v.to_str().ok()),
            Some("missing-bearer"),
        );
        handle.abort();
    }

    #[tokio::test]
    async fn proxy_401_unknown_token() {
        let (store, _dir) = fresh_store();
        let _issued = store.issue("good-client").expect("issue");
        let upstream = MockServer::start().await;
        let (port, handle) = spawn_test_proxy(store, upstream.uri(), None).await;

        let resp = reqwest::Client::new()
            .get(&format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth("not-a-real-token")
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 401);
        assert_eq!(
            resp.headers().get("x-auth-reason").and_then(|v| v.to_str().ok()),
            Some("unknown-token"),
        );
        handle.abort();
    }

    #[tokio::test]
    async fn proxy_401_revoked_token() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("doomed").expect("issue");
        store.revoke(issued.id).expect("revoke");
        let upstream = MockServer::start().await;
        let (port, handle) = spawn_test_proxy(store, upstream.uri(), None).await;

        let resp = reqwest::Client::new()
            .get(&format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth(&issued.token)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 401);
        assert_eq!(
            resp.headers().get("x-auth-reason").and_then(|v| v.to_str().ok()),
            Some("unknown-token"),
            "revoked tokens classify as unknown-token because validate() filters revoked rows"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn proxy_200_proxies_to_backend_on_valid_bearer() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("clinic-laptop").expect("issue");
        let store_for_check = Arc::clone(&store);

        let upstream = MockServer::start().await;
        Mock::given(http_method("GET"))
            .and(http_path("/anything"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok-body"))
            .mount(&upstream)
            .await;

        let (port, handle) = spawn_test_proxy(store, upstream.uri(), None).await;

        let resp = reqwest::Client::new()
            .get(&format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth(&issued.token)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 200);
        let body = resp.text().await.expect("text");
        assert_eq!(body, "ok-body");

        // Validate that touch() fired (last_seen_at is now Some).
        let rows = store_for_check.list().expect("list");
        let row = rows.iter().find(|r| r.id == issued.id).expect("issued row");
        assert!(
            row.last_seen_at.is_some(),
            "successful proxy call should have fired touch()"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn proxy_strips_client_bearer_and_injects_api_key() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("a").expect("issue");

        let upstream = MockServer::start().await;
        // Only match requests where the upstream sees the INJECTED key —
        // wiremock returns 404 (default) for any other auth header. So a
        // 200 response proves the swap happened.
        Mock::given(http_method("GET"))
            .and(http_path("/anything"))
            .and(header("authorization", "Bearer server-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_string("authed"))
            .mount(&upstream)
            .await;

        let (port, handle) = spawn_test_proxy(
            store,
            upstream.uri(),
            Some("server-secret".to_string()),
        )
        .await;

        let resp = reqwest::Client::new()
            .get(&format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth(&issued.token) // client uses its own token
            .send()
            .await
            .expect("send");
        assert_eq!(
            resp.status(),
            200,
            "200 proves wiremock saw 'Bearer server-secret', not the client token"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn proxy_502_when_backend_unreachable() {
        let (store, _dir) = fresh_store();
        let issued = store.issue("a").expect("issue");
        // Port 1 is privileged on Unix and almost certainly not listening.
        // Even if it were, the connect_timeout would fire after 10s; tests
        // run in <1s for the typical "connection refused" case.
        let (port, handle) =
            spawn_test_proxy(store, "http://127.0.0.1:1".to_string(), None).await;

        let resp = reqwest::Client::new()
            .get(&format!("http://127.0.0.1:{port}/anything"))
            .bearer_auth(&issued.token)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 502);
        handle.abort();
    }
}
```

- [ ] **Step 2: Run the new tests**

```bash
cargo test -p medical-sharing --lib auth_proxy::tests
```

Expected: 6 tests pass. (The `tokio::time::sleep(50ms)` warmup makes total run ~300-500ms.)

- [ ] **Step 3: Run the full sharing crate**

```bash
cargo test -p medical-sharing --lib
```

Expected: existing 34 tests + 6 new = 40 total.

- [ ] **Step 4: Commit**

```bash
git add crates/sharing/src/auth_proxy.rs
git commit -m "test(sharing): add wiremock-backed integration tests for auth_proxy"
```

---

## Task 3: Refactor `whisper_supervisor` — extract `download_and_verify`

**Files:**
- Modify: `crates/sharing/src/whisper_supervisor.rs`

This task is a refactor with NO new tests. The goal is to make the SHA256-verify path callable from a unit test with an injected URL.

- [ ] **Step 1: Replace `ensure_binary` body and add `download_and_verify`**

In `crates/sharing/src/whisper_supervisor.rs`, the current `ensure_binary` spans lines 103–165. Replace those lines with:

```rust
    pub async fn ensure_binary(&self) -> Result<PathBuf> {
        let manifest: Manifest =
            serde_json::from_str(MANIFEST).map_err(|e| WhisperError::Manifest(e.to_string()))?;
        let key = platform_key();
        let entry = manifest
            .binaries
            .get(key)
            .ok_or(WhisperError::UnsupportedPlatform)?;

        // A null `url` means whisper.cpp does not publish a prebuilt server binary
        // for this platform. Office-server admins must build from source:
        // https://github.com/ggml-org/whisper.cpp#server
        let url = entry.url.as_deref().ok_or(WhisperError::UnsupportedPlatform)?;
        let archive = entry.archive.as_deref().ok_or(WhisperError::UnsupportedPlatform)?;

        let bin_path = self.binary_dir.join(&entry.binary_name);
        let lock_path = self.binary_dir.join(".whisper-manifest-version");

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

    /// Download an archive from `url`, optionally verify its SHA-256 against
    /// `expected_sha256`, extract `binary_name` into `self.binary_dir`, and
    /// (on Unix) chmod 0755. Returns the path to the extracted binary.
    ///
    /// Extracted into a `pub(crate)` helper so unit tests can supply a
    /// wiremock URL + a controlled archive body. The lock-file write that
    /// records the manifest version stays in `ensure_binary` — this helper
    /// is unaware of the manifest.
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

- [ ] **Step 2: Verify build is clean**

Run:

```bash
cargo build -p medical-sharing
cargo build -p medical-sharing --tests
```

Expected: both compile without errors.

- [ ] **Step 3: Run the existing sharing tests to confirm no regression**

```bash
cargo test -p medical-sharing --lib
```

Expected: 40 tests pass (34 Tier 1 + 6 auth_proxy from Task 2). The `whisper_supervisor` module still has 0 tests at this point — the refactor is silent.

- [ ] **Step 4: Commit**

```bash
git add crates/sharing/src/whisper_supervisor.rs
git commit -m "refactor(whisper_supervisor): extract download_and_verify helper"
```

---

## Task 4: `whisper_supervisor.rs` — 7 archive + SHA256 tests

**Files:**
- Modify: `crates/sharing/src/whisper_supervisor.rs` (append `#[cfg(test)] mod tests` block)

- [ ] **Step 1: Append the test module**

Append the following to the end of `crates/sharing/src/whisper_supervisor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{method as http_method, path as http_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use zip::write::SimpleFileOptions;

    /// Build an in-memory zip archive containing exactly one file named
    /// `binary_name` with the given body.
    fn build_zip_with(binary_name: &str, body: &[u8]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file(binary_name, SimpleFileOptions::default())
                .expect("start_file");
            std::io::Write::write_all(&mut w, body).expect("write");
            w.finish().expect("finish");
        }
        buf.into_inner()
    }

    /// Same but with only `other.txt` — used for the "binary missing" tests.
    fn build_zip_without_target() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file("other.txt", SimpleFileOptions::default())
                .expect("start_file");
            std::io::Write::write_all(&mut w, b"decoy").expect("write");
            w.finish().expect("finish");
        }
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
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, body).expect("append");
        let gz = tar.into_inner().expect("into_inner");
        gz.finish().expect("finish")
    }

    fn build_tar_gz_without_target() -> Vec<u8> {
        build_tar_gz_with("other.txt", b"decoy")
    }

    fn fresh_supervisor() -> (Arc<WhisperSupervisor>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let supervisor = Arc::new(WhisperSupervisor::new(
            dir.path().to_path_buf(),
            dir.path().join("model.bin"),
            0,
        ));
        (supervisor, dir)
    }

    #[test]
    fn extract_zip_extracts_named_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = build_zip_with("whisper-server", b"fake-binary-content");
        extract_zip(&bytes, dir.path(), "whisper-server").expect("extract_zip");
        let out = std::fs::read(dir.path().join("whisper-server")).expect("read");
        assert_eq!(out, b"fake-binary-content");
    }

    #[test]
    fn extract_zip_errors_when_binary_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = build_zip_without_target();
        let r = extract_zip(&bytes, dir.path(), "whisper-server");
        assert!(matches!(r, Err(WhisperError::Manifest(_))));
    }

    #[test]
    fn extract_tar_gz_extracts_named_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = build_tar_gz_with("whisper-server", b"tar-content");
        extract_tar_gz(&bytes, dir.path(), "whisper-server").expect("extract_tar_gz");
        let out = std::fs::read(dir.path().join("whisper-server")).expect("read");
        assert_eq!(out, b"tar-content");
    }

    #[test]
    fn extract_tar_gz_errors_when_binary_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bytes = build_tar_gz_without_target();
        let r = extract_tar_gz(&bytes, dir.path(), "whisper-server");
        assert!(matches!(r, Err(WhisperError::Manifest(_))));
    }

    #[test]
    fn extract_archive_unknown_kind_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let r = WhisperSupervisor::extract_archive(b"", "rar", dir.path(), "whisper-server");
        match r {
            Err(WhisperError::Manifest(msg)) => {
                assert!(
                    msg.contains("unsupported archive"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Err(Manifest); got {other:?}"),
        }
    }

    #[tokio::test]
    async fn download_and_verify_succeeds_with_correct_sha256() {
        let (supervisor, _dir) = fresh_supervisor();
        let zip_bytes = build_zip_with("whisper-server", b"hello-binary");
        let expected = hex::encode(Sha256::digest(&zip_bytes));

        let server = MockServer::start().await;
        Mock::given(http_method("GET"))
            .and(http_path("/binary.zip"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(zip_bytes.clone()),
            )
            .mount(&server)
            .await;

        let url = format!("{}/binary.zip", server.uri());
        let bin = supervisor
            .download_and_verify(&url, "zip", Some(&expected), "whisper-server")
            .await
            .expect("download_and_verify");
        let out = std::fs::read(&bin).expect("read");
        assert_eq!(out, b"hello-binary");
    }

    #[tokio::test]
    async fn download_and_verify_rejects_hash_mismatch() {
        let (supervisor, _dir) = fresh_supervisor();
        let zip_bytes = build_zip_with("whisper-server", b"actual-content");
        let wrong_sha = "0".repeat(64); // 64-char zero hash — guaranteed mismatch

        let server = MockServer::start().await;
        Mock::given(http_method("GET"))
            .and(http_path("/binary.zip"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(zip_bytes.clone()),
            )
            .mount(&server)
            .await;

        let url = format!("{}/binary.zip", server.uri());
        let r = supervisor
            .download_and_verify(&url, "zip", Some(&wrong_sha), "whisper-server")
            .await;
        match r {
            Err(WhisperError::HashMismatch { expected, got }) => {
                assert_eq!(expected, wrong_sha);
                assert_eq!(got, hex::encode(Sha256::digest(&zip_bytes)));
            }
            other => panic!("expected Err(HashMismatch); got {other:?}"),
        }
    }
}
```

**Note on visibility:** `extract_zip` and `extract_tar_gz` are free functions at module scope (private to the module). `WhisperSupervisor::extract_archive` is a private associated function. All three are callable from the same module's `#[cfg(test)] mod tests` block.

- [ ] **Step 2: Run the new tests**

```bash
cargo test -p medical-sharing --lib whisper_supervisor::tests
```

Expected: 7 tests pass.

- [ ] **Step 3: Run the full sharing crate**

```bash
cargo test -p medical-sharing --lib
```

Expected: existing 40 + 7 new = 47 total.

- [ ] **Step 4: Commit**

```bash
git add crates/sharing/src/whisper_supervisor.rs
git commit -m "test(sharing): add unit tests for whisper_supervisor extract + verify"
```

---

## Task 5: Final verification

**Files:** none (verification only).

- [ ] **Step 1: Run the full sharing crate**

```bash
cargo test -p medical-sharing --lib
```

Expected: 47 total tests pass (3 baseline `qr`+`mdns` + 31 Tier 1 + 6 auth_proxy + 7 whisper_supervisor).

- [ ] **Step 2: Run the workspace tests to catch any regression**

```bash
cargo test --workspace --lib 2>&1 | grep -E "^test result"
```

Expected: every line says `ok`, no `FAILED` anywhere.

- [ ] **Step 3: Verify `npm run check` still clean**

(Not strictly required since this is a Rust-only batch, but worth a smoke check.)

```bash
npm run check 2>&1 | tail -3
```

Expected: 0 errors, 0 warnings.

- [ ] **Step 4: If anything red, escalate**

If a test failure reveals a bug in production code, do NOT silently fix it. Per project TDD discipline, the failing test IS the bug report — escalate to the controller.

If nothing is red, no commit needed.

---

## Self-review notes

- **Spec coverage:** every section of the spec maps to a task.
  - wiremock dep → Task 1.
  - auth_proxy 6 tests → Task 2.
  - whisper_supervisor refactor → Task 3.
  - whisper_supervisor 7 tests → Task 4.
  - Verification → Task 5.
- **Refactor isolation:** Task 3 lands the refactor *before* Task 4's tests so a test failure can't be conflated with a refactor regression.
- **No production behavior change:** `ensure_binary`'s observable behavior is identical after Task 3. The lock-file write happens in the success path only, just as before. The `warn!` text changed slightly (now says "binary X" instead of "platform Y") — this is a log-line cosmetic and not part of behavior.
- **No new workspace deps:** `wiremock` is already a workspace dep (`Cargo.toml:66`), just added to `crates/sharing/Cargo.toml`'s `[dev-dependencies]`.
- **Ephemeral-port race:** acknowledged in the spec. The pattern is bind→port→drop→spawn; the race window between drop and spawn is microseconds and the OS rarely re-issues the same port in that interval. Acceptable for tests.
- **`zip` API:** newer versions of the `zip` crate use `SimpleFileOptions` (replacing the older `FileOptions<()>` API). The plan uses `SimpleFileOptions::default()` — confirmed against `zip = "2"` which is the version pinned in `crates/sharing/Cargo.toml:31`.
- **Test naming consistency:** all new test functions use snake_case starting with the module's behavior verb (`proxy_*`, `extract_*`, `download_and_verify_*`).
- **No PHI in logs:** no new `tracing::*` calls. The pre-existing `warn!` for cached-binary replacement is unchanged.
- **No regression in existing tests:** the Tier 1 commit (`9174c8b` + its 5 sub-commits) is base; this plan adds tests only and refactors only the inner of `ensure_binary` without changing its public signature or observable effect.

## Implementation order

1. Add `wiremock` dep (Task 1).
2. auth_proxy tests (Task 2) — independent of whisper changes.
3. whisper_supervisor refactor (Task 3).
4. whisper_supervisor tests (Task 4).
5. Final verification (Task 5).

Each task is its own commit. Failures at any stage can be rolled back without disturbing the earlier ones.
