# Orphaned-Pairing Diagnostics & Log Hygiene Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the misleading "Whisper server rejected authentication — check API key" failure mode into an actionable, debuggable state both server-side (logs distinguish the 401 reason) and client-side (error message tells the user to re-pair), and fix two unrelated logging-hygiene bugs surfaced during the same investigation.

**Architecture:** The auth proxy currently returns a bare 401 on two distinct paths (no `Authorization` header, or bearer doesn't validate) with no observable difference. Task 1 adds a `X-Auth-Reason` response header and a `warn!` log on each path. Task 2 teaches the remote STT provider to read that header and rewrite the error into a re-pair instruction. Tasks 3 and 4 fix two collateral issues found during root-cause: a misleading provider-init log line and a `cleanup_old_logs` function that never deletes anything because of a filename-pattern bug.

**Tech Stack:** Rust workspace (Tauri + axum + reqwest), Svelte 5 frontend (untouched here — backend-only changes). Tests use `tokio::test`, `wiremock`, `tempfile`. No new dependencies.

**Context:** The root cause was diagnosed in conversation. A remote (Windows) client was paired on 2026-05-06; the office server was later rebuilt, which orphaned the row in `sharing.db` while leaving the bearer in the client's Credential Manager. The client kept sending a token that no longer existed server-side. The user fix is "unpair + re-pair," but the error message led the user toward "check API key," which has no meaning in this app's auth model. This plan makes the same scenario diagnosable from logs and self-explanatory from the UI error.

---

## File Structure

**Modified:**
- `crates/sharing/src/auth_proxy.rs` — emit `X-Auth-Reason` header on 401, add `warn!` on each rejection path
- `crates/sharing/tests/auth_proxy.rs` — assertions for the new header values
- `crates/stt-providers/src/remote_provider.rs` — read `X-Auth-Reason` on 401/403 and rewrite the error message
- `src-tauri/src/state.rs` — rename misleading fields in the `Initializing remote STT provider` log line
- `src-tauri/src/lib.rs` — fix the filename-pattern comment and the `cleanup_old_logs` filter

**No new files.** No new crates. No frontend changes.

---

## Task 1: Distinguish the two 401 paths in the auth proxy

**Files:**
- Modify: `crates/sharing/src/auth_proxy.rs:72-82`
- Modify: `crates/sharing/tests/auth_proxy.rs:24-49, 81-109`

**Why:** Today both "no Authorization header" and "token doesn't match any non-revoked row" return a bare 401 with no log emission. There's no way for an operator to tell them apart from server logs or for a client to tell them apart from the response. We add a `X-Auth-Reason` header on each path (no PHI, no secret material — just a short tag) and a `warn!` log so the server's `ferri-scribe.log.*` files reveal which rejection path fired.

- [ ] **Step 1: Add a failing test for the `missing-bearer` header**

  Modify the existing `missing_bearer_returns_401` test at `crates/sharing/tests/auth_proxy.rs:24-49` to also assert the response carries `X-Auth-Reason: missing-bearer`. Replace the existing test body with:

  ```rust
  #[tokio::test]
  async fn missing_bearer_returns_401_with_reason_header() {
      let tmp = TempDir::new().unwrap();
      let store = Arc::new(TokenStore::open(tmp.path().join("t.db"), &[3u8; 32]).unwrap());

      let backend_port = next_free_port().await;
      tokio::spawn(fake_backend(backend_port));

      let proxy_port = next_free_port().await;
      let cfg = ProxyConfig {
          listen_port: proxy_port,
          backend_url: format!("http://127.0.0.1:{backend_port}"),
          path_prefix: "/api".to_string(),
          inject_api_key: None,
      };
      spawn_auth_proxy(cfg, store.clone()).await.unwrap();
      tokio::time::sleep(Duration::from_millis(150)).await;

      let resp = reqwest::Client::new()
          .post(format!("http://127.0.0.1:{proxy_port}/api/chat"))
          .body("hello")
          .send()
          .await
          .unwrap();
      assert_eq!(resp.status(), 401);
      assert_eq!(
          resp.headers().get("x-auth-reason").and_then(|v| v.to_str().ok()),
          Some("missing-bearer"),
          "401 from missing Authorization should be tagged"
      );
  }
  ```

- [ ] **Step 2: Run the test to verify it fails**

  Run: `cargo test -p medical-sharing --test auth_proxy missing_bearer_returns_401_with_reason_header -- --nocapture`

  Expected: FAIL with `assertion failed: ... left: None, right: Some("missing-bearer")` (header is absent in current code).

- [ ] **Step 3: Add a failing test for the `unknown-token` header**

  Modify the existing `revoked_bearer_returns_401` test at `crates/sharing/tests/auth_proxy.rs:81-109` similarly:

  ```rust
  #[tokio::test]
  async fn unknown_or_revoked_bearer_returns_401_with_reason_header() {
      let tmp = TempDir::new().unwrap();
      let store = Arc::new(TokenStore::open(tmp.path().join("t.db"), &[5u8; 32]).unwrap());
      let issued = store.issue("evil").unwrap();
      store.revoke(issued.id).unwrap();

      let backend_port = next_free_port().await;
      tokio::spawn(fake_backend(backend_port));

      let proxy_port = next_free_port().await;
      let cfg = ProxyConfig {
          listen_port: proxy_port,
          backend_url: format!("http://127.0.0.1:{backend_port}"),
          path_prefix: "/api".to_string(),
          inject_api_key: None,
      };
      spawn_auth_proxy(cfg, store.clone()).await.unwrap();
      tokio::time::sleep(Duration::from_millis(150)).await;

      let resp = reqwest::Client::new()
          .post(format!("http://127.0.0.1:{proxy_port}/api/chat"))
          .bearer_auth(&issued.token)
          .body("ping")
          .send()
          .await
          .unwrap();
      assert_eq!(resp.status(), 401);
      assert_eq!(
          resp.headers().get("x-auth-reason").and_then(|v| v.to_str().ok()),
          Some("unknown-token"),
          "401 from revoked/unknown token should be tagged"
      );
  }
  ```

- [ ] **Step 4: Run the test to verify it fails**

  Run: `cargo test -p medical-sharing --test auth_proxy unknown_or_revoked_bearer_returns_401_with_reason_header -- --nocapture`

  Expected: FAIL with `assertion failed: ... left: None, right: Some("unknown-token")`.

- [ ] **Step 5: Implement the header + warn! emissions**

  Modify `crates/sharing/src/auth_proxy.rs`. Replace the `handler` and `handle_inner` block (lines 65-135) so that the two 401 paths return a `Response` with the `X-Auth-Reason` header instead of bubbling a bare `StatusCode`. Change `handle_inner`'s error type from `StatusCode` to `Response`, and at each rejection site build the response explicitly.

  Replace the existing `handler` + `handle_inner` declarations with:

  ```rust
  async fn handler(State(state): State<AppState>, req: Request) -> Response {
      match handle_inner(state, req).await {
          Ok(resp) => resp,
          Err(resp) => resp,
      }
  }

  fn unauthorized_with_reason(reason: &'static str) -> Response {
      let mut resp = (StatusCode::UNAUTHORIZED, "").into_response();
      // HeaderValue from a 'static &str is infallible for short ASCII tags.
      resp.headers_mut()
          .insert("x-auth-reason", HeaderValue::from_static(reason));
      resp
  }

  async fn handle_inner(state: AppState, req: Request) -> Result<Response, Response> {
      let token = match extract_bearer(req.headers()) {
          Some(t) => t,
          None => {
              warn!("proxy: 401 missing-bearer (no Authorization header)");
              return Err(unauthorized_with_reason("missing-bearer"));
          }
      };

      let row = match state.store.validate(&token) {
          Ok(Some(row)) => row,
          Ok(None) => {
              warn!("proxy: 401 unknown-token (no matching non-revoked row)");
              return Err(unauthorized_with_reason("unknown-token"));
          }
          Err(e) => {
              warn!(error = %e, "proxy: 500 token store validation error");
              return Err((StatusCode::INTERNAL_SERVER_ERROR, "").into_response());
          }
      };
      let client_id = row.id;
      debug!(client_id, "proxy: validated bearer");
      let _ = state.store.touch(client_id);

      let (parts, body) = req.into_parts();

      const MAX_BODY_BYTES: usize = 256 * 1024 * 1024;
      let body_bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
          .await
          .map_err(|_| (StatusCode::PAYLOAD_TOO_LARGE, "").into_response())?;

      let path_query = parts
          .uri
          .path_and_query()
          .map(|p| p.as_str())
          .unwrap_or("/");
      let upstream_url = format!("{}{}", state.config.backend_url.trim_end_matches('/'), path_query);

      let mut req_builder = state
          .client
          .request(parts.method.clone(), &upstream_url)
          .body(body_bytes.clone());

      for (k, v) in parts.headers.iter() {
          if k == reqwest::header::HOST || k == reqwest::header::AUTHORIZATION {
              continue;
          }
          req_builder = req_builder.header(k.clone(), v.clone());
      }
      if let Some(api_key) = &state.config.inject_api_key {
          req_builder = req_builder.bearer_auth(api_key);
      }

      let upstream = req_builder.send().await.map_err(|e| {
          warn!("proxy upstream error: {e}");
          (StatusCode::BAD_GATEWAY, "").into_response()
      })?;

      let status = upstream.status();
      let mut resp_headers = HeaderMap::new();
      for (k, v) in upstream.headers() {
          if let Ok(hv) = HeaderValue::from_bytes(v.as_bytes()) {
              resp_headers.insert(k.clone(), hv);
          }
      }

      use futures_util::TryStreamExt;
      let stream = upstream
          .bytes_stream()
          .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
      let body = Body::from_stream(stream);
      let mut resp = Response::new(body);
      *resp.status_mut() = status;
      *resp.headers_mut() = resp_headers;
      Ok(resp)
  }
  ```

  No other imports needed — `HeaderValue` and `IntoResponse` are already imported at the top of the file.

- [ ] **Step 6: Run both new tests to verify they pass**

  Run: `cargo test -p medical-sharing --test auth_proxy -- --nocapture`

  Expected: all four tests in `auth_proxy.rs` pass (including the two new assertions and the unchanged `valid_bearer_forwards_body`).

- [ ] **Step 7: Run the full sharing crate test suite to confirm nothing regressed**

  Run: `cargo test -p medical-sharing`

  Expected: all tests pass.

- [ ] **Step 8: Commit**

  ```bash
  git add crates/sharing/src/auth_proxy.rs crates/sharing/tests/auth_proxy.rs
  git commit -m "feat(sharing): distinguish 401 reasons via X-Auth-Reason header + warn logs

  Auth proxy previously returned a bare 401 on two distinct paths (missing
  Authorization header vs. unknown/revoked token) with no observable
  difference. Add a short response header (no PHI, no secrets) and a warn!
  log on each rejection so server logs reveal which path fired and clients
  can produce a specific error message."
  ```

---

## Task 2: Map 401/403 to an actionable re-pair instruction in the remote STT provider

**Files:**
- Modify: `crates/stt-providers/src/remote_provider.rs:253-258, 524-543`

**Why:** The current error text — "Whisper server rejected authentication — check API key" — is misleading. This app's clients don't have API keys; they have bearers issued by pairing. When a bearer becomes orphaned (server rebuilt, row revoked), the user needs to *re-pair*, not check a setting. We branch on the new `X-Auth-Reason: unknown-token` header so the message is precise when we have evidence, and falls back to a generic auth-failure message otherwise.

- [ ] **Step 1: Modify the existing failing-test for the new message wording**

  Replace the existing `http_401_maps_to_auth_error` test at `crates/stt-providers/src/remote_provider.rs:524-543` with two tests covering both branches:

  ```rust
  #[tokio::test]
  async fn http_401_with_unknown_token_reason_maps_to_repair_message() {
      let server = MockServer::start().await;
      Mock::given(method("POST"))
          .and(path("/v1/audio/transcriptions"))
          .respond_with(
              ResponseTemplate::new(401).insert_header("x-auth-reason", "unknown-token"),
          )
          .mount(&server)
          .await;

      let provider = provider_at(&server.uri(), Some("stale".into()));
      let err = provider
          .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
          .await
          .unwrap_err()
          .to_string();
      assert!(
          err.contains("re-pair") || err.contains("no longer recognises"),
          "expected re-pair guidance, got: {err}"
      );
  }

  #[tokio::test]
  async fn http_401_without_reason_header_maps_to_generic_auth_error() {
      let server = MockServer::start().await;
      Mock::given(method("POST"))
          .and(path("/v1/audio/transcriptions"))
          .respond_with(ResponseTemplate::new(401))
          .mount(&server)
          .await;

      let provider = provider_at(&server.uri(), Some("bad".into()));
      let err = provider
          .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
          .await
          .unwrap_err()
          .to_string();
      assert!(
          err.contains("authentication"),
          "expected generic auth error, got: {err}"
      );
  }
  ```

- [ ] **Step 2: Run both tests to verify they fail**

  Run: `cargo test -p medical-stt-providers --lib remote_provider::tests::http_401`

  Expected: `http_401_with_unknown_token_reason_maps_to_repair_message` FAILS (current code says "check API key", not "re-pair"). `http_401_without_reason_header_maps_to_generic_auth_error` may pass coincidentally (the current message contains "authentication"); confirm by reading output. Both names should appear in the test runner output.

- [ ] **Step 3: Implement the branching error message**

  Replace the 401/403 block in `crates/stt-providers/src/remote_provider.rs:253-258` with a check that reads the response header before consuming the body:

  ```rust
          let status = resp.status();
          if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
              // The auth proxy tags its 401s with `x-auth-reason: unknown-token`
              // when the bearer doesn't match any non-revoked row. That's the
              // orphaned-pairing case (e.g. office server rebuilt after pair).
              // Surface a specific instruction; fall back to a generic message
              // otherwise.
              let reason = resp
                  .headers()
                  .get("x-auth-reason")
                  .and_then(|v| v.to_str().ok())
                  .map(|s| s.to_string());
              let msg = match reason.as_deref() {
                  Some("unknown-token") => {
                      "Office server no longer recognises this client \
                       \u{2014} please re-pair (Settings \u{2192} Sharing \u{2192} Unpair, \
                       then scan a fresh code from the office machine)."
                          .to_string()
                  }
                  _ => "Whisper server rejected authentication \u{2014} re-pair the client if the office server was reinstalled.".to_string(),
              };
              return Err(AppError::SttProvider(msg));
          }
  ```

  (Use unicode escapes `\u{2014}` for em-dash and `\u{2192}` for the arrow — keeps the source file ASCII-safe and matches the existing project style for that character in the same file.)

- [ ] **Step 4: Run both tests to verify they pass**

  Run: `cargo test -p medical-stt-providers --lib remote_provider::tests::http_401`

  Expected: both tests PASS.

- [ ] **Step 5: Run the full stt-providers crate test suite to confirm nothing regressed**

  Run: `cargo test -p medical-stt-providers`

  Expected: all tests pass. (Pay particular attention that `authorization_header_sent_when_api_key_present` and `happy_path_returns_segments_without_diarization` still pass — those exercise the 200-OK path.)

- [ ] **Step 6: Commit**

  ```bash
  git add crates/stt-providers/src/remote_provider.rs
  git commit -m "feat(stt): rewrite 401 message to point at re-pair, not API key

  The previous error 'Whisper server rejected authentication — check API
  key' misled users: this app's clients don't have API keys, they have
  bearers issued by pairing. When the auth proxy tags a 401 with the
  unknown-token reason header (set when sharing.db has no matching row
  — e.g. office server rebuilt), surface a specific re-pair instruction.
  Fall back to a generic auth-failure message when no header is present."
  ```

---

## Task 3: Clarify the misleading "Initializing remote STT provider" log line

**Files:**
- Modify: `src-tauri/src/state.rs:340-346`

**Why:** Today the line logs `host=…` and `port=…` from `config.stt_remote_host` and `config.stt_remote_port` — but those are the *static fallback* used only when no paired endpoint is set. At request time, the actual URL comes from `RemoteEndpoint::resolve_base_url()`. In root-cause investigation, this misleading line cost us several minutes (we thought traffic was going to port 8080 when it was actually going to the paired proxy at port 8081). Rename the fields and add the paired-endpoint summary so the line is self-explanatory.

- [ ] **Step 1: Modify the log line**

  In `src-tauri/src/state.rs`, replace the existing `info!` block at lines 340-346 with:

  ```rust
              // Logged fields document the *fallback* (used only when no
              // paired endpoint is configured). The actual request URL at
              // transcribe-time comes from `whisper_ep.resolve_base_url()`.
              // Renaming `host`/`port` to `fallback_host`/`fallback_port`
              // prevents the line from being read as "the URL we're hitting"
              // (a real source of confusion during a 2026-05-11 debugging
              // session).
              let paired = whisper_ep.as_ref().map(|ep| {
                  format!(
                      "lan={} tailscale={} port={}",
                      ep.lan.as_deref().unwrap_or("-"),
                      ep.tailscale.as_deref().unwrap_or("-"),
                      ep.port
                  )
              });
              info!(
                  fallback_host = %config.stt_remote_host,
                  fallback_port = config.stt_remote_port,
                  model = %config.stt_remote_model,
                  has_bearer = bearer.is_some(),
                  paired_endpoint = paired.as_deref().unwrap_or("none"),
                  "Initializing remote STT provider"
              );
  ```

  Drop the explanatory comment block from the suggested code above before saving — only the working code stays; the *reason* for the change lives in the commit message.

- [ ] **Step 2: Run the workspace build to confirm it compiles**

  Run: `cargo build -p rust-medical-assistant`

  Expected: clean build (no warnings about unused imports — `RemoteEndpoint` is already imported at `state.rs:15`).

- [ ] **Step 3: Run the existing state tests to confirm no regression**

  Run: `cargo test -p rust-medical-assistant --lib state`

  Expected: all tests in `state.rs` continue to pass (in particular `init_stt_providers_remote_mode_builds_remote_provider`).

- [ ] **Step 4: Commit**

  ```bash
  git add src-tauri/src/state.rs
  git commit -m "log(stt): rename misleading host/port fields, add paired_endpoint

  The 'Initializing remote STT provider' line previously logged
  config.stt_remote_host / stt_remote_port as 'host' / 'port' — but those
  are the static fallback, used only when no paired endpoint is set. The
  actual request URL comes from RemoteEndpoint::resolve_base_url(). Rename
  to fallback_host / fallback_port and add a paired_endpoint summary so
  the line reflects the resolution it documents."
  ```

---

## Task 4: Fix log filename comment and `cleanup_old_logs` filter

**Files:**
- Modify: `src-tauri/src/lib.rs:50, 266-287`
- Test: new unit test inside the existing `lib.rs` (no separate file)

**Why:** Two related bugs in the same file:
1. The comment at `lib.rs:50` says `ferri-scribe.YYYY-MM-DD.log` — but `tracing_appender::rolling::daily` with prefix `"ferri-scribe.log"` produces files named `ferri-scribe.log.YYYY-MM-DD` (date appended *after* the prefix). The wrong comment misled a diagnostic step in this same investigation.
2. `cleanup_old_logs` at `lib.rs:277` only deletes files whose extension is `"log"`. The rolled files have extension `2026-05-11` (the date string), so the filter never matches them. Old logs are never cleaned up — `cleanup_old_logs` is currently a no-op for the file pattern this app actually writes.

- [ ] **Step 1: Add a failing test for `cleanup_old_logs`**

  Append the following to `src-tauri/src/lib.rs` (at the end of the file, inside a new `#[cfg(test)] mod tests` block if one doesn't exist — search the file first; if `mod tests` already exists, add the test inside it):

  ```rust
  #[cfg(test)]
  mod cleanup_old_logs_tests {
      use super::cleanup_old_logs;
      use std::fs;
      use std::time::{Duration, SystemTime};

      fn touch(path: &std::path::Path, age: Duration) {
          fs::write(path, b"x").unwrap();
          let mtime = SystemTime::now() - age;
          let ft = filetime::FileTime::from_system_time(mtime);
          filetime::set_file_mtime(path, ft).unwrap();
      }

      #[test]
      fn deletes_rolled_files_older_than_cutoff() {
          let tmp = tempfile::tempdir().unwrap();
          let old = tmp.path().join("ferri-scribe.log.2025-01-01");
          let new = tmp.path().join("ferri-scribe.log.2026-05-11");
          let unrelated = tmp.path().join("other.txt");

          touch(&old, Duration::from_secs(30 * 24 * 3600));
          touch(&new, Duration::from_secs(60));
          touch(&unrelated, Duration::from_secs(30 * 24 * 3600));

          cleanup_old_logs(tmp.path(), 7);

          assert!(!old.exists(), "old rolled log should be deleted");
          assert!(new.exists(), "recent rolled log should remain");
          assert!(unrelated.exists(), "unrelated files must not be touched");
      }
  }
  ```

  This test needs `filetime` and `tempfile`. `tempfile` is already a dev-dep in `src-tauri/Cargo.toml` via `tempfile = { workspace = true }`. `filetime` is NOT yet in the workspace — add it to `src-tauri/Cargo.toml` under `[dev-dependencies]`:

  ```toml
  filetime = "0.2"
  ```

  No workspace-level entry change is required; this is a leaf-package dev-dep only.

- [ ] **Step 2: Run the test to verify it fails**

  Run: `cargo test -p rust-medical-assistant --lib cleanup_old_logs_tests`

  Expected: FAIL. `old` file still exists after `cleanup_old_logs` because the current filter checks `path.extension() == Some("log")`, but the file's extension is `"2025-01-01"`.

- [ ] **Step 3: Fix the filter in `cleanup_old_logs`**

  Modify `src-tauri/src/lib.rs:275-286`. Replace the inner loop body so it matches the actual filename convention:

  ```rust
      for entry in entries.flatten() {
          let path = entry.path();
          // Rolled log files are named `ferri-scribe.log.YYYY-MM-DD` (the
          // date is appended after the prefix by tracing_appender::rolling::daily).
          // The current file (no rotation suffix yet) is `ferri-scribe.log`.
          // Match by filename prefix rather than extension — extension is the
          // date string, not `.log`.
          let is_log = path
              .file_name()
              .and_then(|n| n.to_str())
              .map(|n| n == "ferri-scribe.log" || n.starts_with("ferri-scribe.log."))
              .unwrap_or(false);
          if !is_log {
              continue;
          }
          if let Ok(meta) = path.metadata()
              && let Ok(modified) = meta.modified()
                  && modified < cutoff {
                      tracing::debug!(file = %path.display(), "Removing old log file");
                      let _ = std::fs::remove_file(&path);
                  }
      }
  ```

- [ ] **Step 4: Fix the misleading filename comment**

  In `src-tauri/src/lib.rs`, change line 50 from:

  ```rust
      // Rolling daily log file: ferri-scribe.YYYY-MM-DD.log
  ```

  to:

  ```rust
      // Rolling daily log file: ferri-scribe.log.YYYY-MM-DD
      // (tracing_appender appends the date AFTER the prefix; current
      // file is `ferri-scribe.log` without suffix until rotation.)
  ```

- [ ] **Step 5: Run the test to verify it passes**

  Run: `cargo test -p rust-medical-assistant --lib cleanup_old_logs_tests`

  Expected: PASS.

- [ ] **Step 6: Run the full tauri-app test suite to confirm nothing regressed**

  Run: `cargo test -p rust-medical-assistant --lib`

  Expected: all tests pass.

- [ ] **Step 7: Commit**

  ```bash
  git add src-tauri/src/lib.rs src-tauri/Cargo.toml
  git commit -m "fix(logs): correctly match rolled log files in cleanup_old_logs

  tracing_appender::rolling::daily produces files named
  ferri-scribe.log.YYYY-MM-DD (date appended after prefix), not
  ferri-scribe.YYYY-MM-DD.log as the comment claimed. The cleanup function
  filtered by path.extension() == 'log', so it never matched any rolled
  file — old logs were never deleted. Match by filename prefix instead.
  Fix the comment to reflect the actual filename pattern."
  ```

---

## Final verification

After all four tasks are committed, run the full workspace test suite once as a sanity check:

- [ ] **Final Step: Workspace test sweep**

  Run: `cargo test --workspace --lib && cargo test -p medical-sharing --test auth_proxy && cargo test -p medical-stt-providers --lib`

  Expected: all tests pass.

- [ ] **Final Step: Verify no PHI/log-policy regressions**

  Run: `grep -nR "tracing::debug\\|tracing::info\\|tracing::warn\\|println!\\|eprintln!" crates/sharing/src/auth_proxy.rs crates/stt-providers/src/remote_provider.rs src-tauri/src/state.rs src-tauri/src/lib.rs | grep -v "// "`

  Expected: only log lines that contain identifiers, counts, statuses, URLs, or static strings — no patient transcript content, no medication/condition names, no bearer values. The per-CLAUDE.md rule "Log counts, lengths, IDs — never content" must hold. (This is a manual visual check; nothing logged in these files touches PHI today and the changes in this plan only add structural information.)
