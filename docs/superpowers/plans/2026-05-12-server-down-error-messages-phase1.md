# Server-Down Error Messages (Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace cryptic connection-failure errors (`"Connection refused — is Ollama running at 192.168.1.10:11434?"`, `"Cannot reach Whisper server at …"`, `"Office server unreachable on LAN or Tailscale (Ollama)."`) with a structured `AppError::EndpointOffline` variant that the frontend renders as a plain-language dialog (title, body, Retry / Open Settings / Cancel) — and prevent the failure entirely with a 3-second pre-flight probe at the top of every Tauri command that hits a remote endpoint.

**Architecture:** Three layers. (1) A new error variant in `crates/core/src/error.rs` carries `service`, `endpoint`, `reason`, `provider_name`. (2) A new module `crates/core/src/preflight.rs` provides `probe_endpoint` (one HTTP GET, 3s timeout, classifies reqwest errors) and `preflight_for_command` (picks endpoints from settings, parallel-probes, skips loopback). (3) Tauri commands call pre-flight first; existing reqwest-error mappers in the three providers (`ollama.rs`, `lmstudio.rs`, `stt-providers/remote_provider.rs`) get rewritten to produce the new variant. On the frontend, a Svelte store with `openAndWait(payload): Promise<Decision>`, a dialog component, and an `invokeWithOfflineHandling` helper that loops on Retry so the original action resumes on success.

**Tech Stack:** Rust workspace (Tauri, reqwest, tokio, thiserror, tracing); Svelte 5 + TypeScript frontend (Vitest for unit tests); `wiremock` for HTTP test mocking (new dev-dependency on `medical-core`).

**Spec:** [`docs/superpowers/specs/2026-05-12-server-down-error-messages-design.md`](../specs/2026-05-12-server-down-error-messages-design.md)

---

## File Structure

**New files:**
- `crates/core/src/preflight.rs` — `CommandKind`, `probe_endpoint`, `preflight_for_command`, `classify_reqwest_error`
- `src/lib/stores/endpointOffline.ts` — Svelte store with `openAndWait(payload): Promise<Decision>`
- `src/lib/api/invokeWithOfflineHandling.ts` — helper + `OfflineCancelled` sentinel
- `src/lib/components/EndpointOfflineDialog.svelte` — modal dialog component
- `src/lib/stores/endpointOffline.test.ts` — store tests
- `src/lib/api/invokeWithOfflineHandling.test.ts` — helper tests
- `src/lib/components/EndpointOfflineDialog.test.ts` — component tests

**Modified files:**
- `crates/core/src/error.rs` — add `ServiceKind`, `OfflineReason`, `EndpointOffline` variant; extend `Serialize` impl
- `crates/core/src/lib.rs` — `pub mod preflight;`
- `crates/core/Cargo.toml` — add `wiremock` dev-dep
- `crates/ai-providers/src/ollama.rs` — replace `"Office server unreachable …"` and HTTP-send connect/timeout branches with `EndpointOffline` constructors
- `crates/ai-providers/src/lmstudio.rs` — same as ollama
- `crates/stt-providers/src/remote_provider.rs:236-251` — replace `is_connect()` / `is_timeout()` branches with `EndpointOffline`
- `src-tauri/src/commands/providers.rs` — refactor `test_*_connection` to use the shared `classify_reqwest_error` (keeps existing string-return shape)
- `src-tauri/src/commands/generation/soap.rs` — pre-flight at top of `generate_soap_inner`
- `src-tauri/src/commands/generation/referral.rs`, `letter.rs`, `synopsis.rs` — same
- `src-tauri/src/commands/chat.rs` — pre-flight at top
- `src-tauri/src/commands/transcription/inner.rs` — pre-flight at top (when STT provider is remote)
- `src/lib/stores/pipeline.ts:163` — switch `processRecording(...).catch(...)` to `invokeWithOfflineHandling`
- `src/App.svelte` (or main layout) — mount `<EndpointOfflineDialog/>` at app root

**No changes to:** `AppError::AiProvider(String)` / `AppError::SttProvider(String)` paths for non-connection errors (5xx, malformed JSON, auth failures); existing Settings UI; existing retry / circuit-breaker logic.

---

## Task 1: Add `ServiceKind`, `OfflineReason`, and the `EndpointOffline` variant

**Files:**
- Modify: `crates/core/src/error.rs`

**Why:** Every other piece of the design depends on this variant existing and serializing into the structured shape the frontend reads. Build it first; the rest of the plan references its fields.

- [ ] **Step 1: Write the failing serialization test**

  Append to `crates/core/src/error.rs` inside the `tests` module:

  ```rust
  #[test]
  fn endpoint_offline_serializes_with_structured_fields() {
      let err = AppError::EndpointOffline {
          service: ServiceKind::AiProvider,
          endpoint: "http://192.168.1.10:11434".into(),
          reason: OfflineReason::ConnectionRefused,
          provider_name: "Ollama".into(),
      };
      let json = serde_json::to_value(&err).expect("serialize");
      assert_eq!(json["kind"], "EndpointOffline");
      assert_eq!(json["service"], "AiProvider");
      assert_eq!(json["endpoint"], "http://192.168.1.10:11434");
      assert_eq!(json["reason"], "ConnectionRefused");
      assert_eq!(json["provider_name"], "Ollama");
      assert!(
          json["message"].as_str().unwrap().contains("Ollama"),
          "message should contain provider_name for log readability"
      );
  }

  #[test]
  fn endpoint_offline_kind_str_is_stable() {
      let err = AppError::EndpointOffline {
          service: ServiceKind::RemoteStt,
          endpoint: "http://x:1".into(),
          reason: OfflineReason::Timeout,
          provider_name: "Whisper STT".into(),
      };
      assert_eq!(err.kind_str(), "EndpointOffline");
  }

  #[test]
  fn service_kind_serializes_as_pascalcase() {
      let json = serde_json::to_value(ServiceKind::RemoteStt).unwrap();
      assert_eq!(json, serde_json::json!("RemoteStt"));
  }

  #[test]
  fn offline_reason_serializes_as_pascalcase() {
      let json = serde_json::to_value(OfflineReason::DnsFailure).unwrap();
      assert_eq!(json, serde_json::json!("DnsFailure"));
  }
  ```

- [ ] **Step 2: Run the tests; they should fail to compile**

  Run: `cargo test -p medical-core --lib error::tests::endpoint_offline_serializes_with_structured_fields`

  Expected: build error — `ServiceKind`, `OfflineReason`, and `EndpointOffline` are not defined.

- [ ] **Step 3: Add the new types**

  In `crates/core/src/error.rs`, add immediately above the `AppError` enum:

  ```rust
  /// Which remote service produced an `EndpointOffline` error.
  /// Serialized as PascalCase strings (`"AiProvider"`, `"RemoteStt"`) so
  /// the frontend can pattern-match without depending on Rust's
  /// internal naming.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
  pub enum ServiceKind {
      AiProvider,
      RemoteStt,
  }

  /// Why a remote endpoint appears offline. Each variant corresponds to a
  /// distinct user-visible dialog message in
  /// `src/lib/components/EndpointOfflineDialog.svelte`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
  pub enum OfflineReason {
      ConnectionRefused,
      Timeout,
      DnsFailure,
      TlsFailure,
  }
  ```

  Then add a new variant to `AppError` (alphabetical-ish placement — between `Database` and `Security` is fine; keep the file readable):

  ```rust
  #[error("{provider_name} at {endpoint} is offline ({reason:?})")]
  EndpointOffline {
      service: ServiceKind,
      endpoint: String,
      reason: OfflineReason,
      provider_name: String,
  },
  ```

- [ ] **Step 4: Update `kind_str()` and the `Serialize` impl**

  Add the new arm to `kind_str()`:

  ```rust
  AppError::EndpointOffline { .. } => "EndpointOffline",
  ```

  Replace the existing `impl serde::Serialize for AppError` block with one that special-cases `EndpointOffline`:

  ```rust
  impl serde::Serialize for AppError {
      fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
      where
          S: serde::Serializer,
      {
          use serde::ser::SerializeStruct;
          match self {
              AppError::EndpointOffline {
                  service,
                  endpoint,
                  reason,
                  provider_name,
              } => {
                  let mut s = serializer.serialize_struct("AppError", 6)?;
                  s.serialize_field("kind", self.kind_str())?;
                  s.serialize_field("message", &self.to_string())?;
                  s.serialize_field("service", service)?;
                  s.serialize_field("endpoint", endpoint)?;
                  s.serialize_field("reason", reason)?;
                  s.serialize_field("provider_name", provider_name)?;
                  s.end()
              }
              _ => {
                  let mut s = serializer.serialize_struct("AppError", 2)?;
                  s.serialize_field("kind", self.kind_str())?;
                  s.serialize_field("message", &self.to_string())?;
                  s.end()
              }
          }
      }
  }
  ```

- [ ] **Step 5: Run the tests; they should pass**

  Run: `cargo test -p medical-core --lib error::tests`

  Expected: PASS (all 4 new tests plus the existing 7 tests pass; the existing `app_error_serializes_with_kind_and_message` etc. still pass because the `_` arm preserves the old behavior).

- [ ] **Step 6: Run the whole core test suite**

  Run: `cargo test -p medical-core`

  Expected: all tests pass.

- [ ] **Step 7: Commit**

  ```bash
  git add crates/core/src/error.rs
  git commit -m "feat(core): add EndpointOffline error variant with structured fields

Adds ServiceKind, OfflineReason, and AppError::EndpointOffline. The
Serialize impl emits the structured fields alongside the existing
kind/message so the frontend can render a plain-language dialog
(per the Phase 1 spec)."
  ```

---

## Task 2: Add `probe_endpoint` and `classify_reqwest_error` in `crates/core/src/preflight.rs`

**Files:**
- Create: `crates/core/src/preflight.rs`
- Modify: `crates/core/src/lib.rs`
- Modify: `crates/core/Cargo.toml`

**Why:** The probe is the leaf function every higher-level pre-flight call composes on top of. Building it before the orchestrator means we test it in isolation, with deterministic HTTP behavior, before any settings-shape complexity enters the picture.

- [ ] **Step 1: Add `wiremock` to dev-dependencies**

  Modify `crates/core/Cargo.toml`. Locate the `[dev-dependencies]` table and add:

  ```toml
  wiremock = "0.6"
  tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time"] }
  ```

  (If `tokio` is already in dev-deps, just ensure the features `macros` and `rt-multi-thread` are listed. If `wiremock = "0.6"` doesn't resolve to a published version at install time, run `cargo search wiremock` and pin the latest 0.x compatible release.)

  Run: `cargo build -p medical-core --tests`

  Expected: builds successfully; the new dep is fetched.

- [ ] **Step 2: Create the preflight module skeleton**

  Create `crates/core/src/preflight.rs`:

  ```rust
  //! Pre-flight connectivity probes for remote AI / STT endpoints.
  //!
  //! `probe_endpoint` issues a single short-timeout GET. `classify_reqwest_error`
  //! maps a `reqwest::Error` into an `OfflineReason`. `preflight_for_command`
  //! (added in Task 3) composes these with settings.

  use std::time::Duration;

  use reqwest::Client;
  use tracing::{debug, warn};

  use crate::error::{AppError, OfflineReason, ServiceKind};

  /// Cap on a single probe's wall time. Chosen to be long enough for a
  /// healthy LAN round-trip plus TLS handshake (~200ms typical) but short
  /// enough that an offline server doesn't visibly stall the UI.
  pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

  /// Classify a failed reqwest send into the user-facing `OfflineReason`.
  /// Returns `None` if the error is something other than a connection /
  /// timeout / DNS / TLS failure (e.g. URL parse error, body decode) —
  /// caller should treat that as a genuine bug rather than an offline
  /// endpoint.
  pub fn classify_reqwest_error(err: &reqwest::Error) -> Option<OfflineReason> {
      if err.is_timeout() {
          return Some(OfflineReason::Timeout);
      }
      if err.is_connect() {
          // Walk the error source chain looking for hyper / std::io
          // signals that distinguish DNS failures from plain connection
          // refusals. `is_connect()` is true for both.
          let mut source: Option<&(dyn std::error::Error + 'static)> = err.source();
          while let Some(s) = source {
              let s_str = s.to_string().to_lowercase();
              if s_str.contains("dns") || s_str.contains("failed to lookup") {
                  return Some(OfflineReason::DnsFailure);
              }
              if s_str.contains("tls") || s_str.contains("handshake") || s_str.contains("certificate") {
                  return Some(OfflineReason::TlsFailure);
              }
              source = s.source();
          }
          return Some(OfflineReason::ConnectionRefused);
      }
      None
  }

  /// Probe a single endpoint. Returns `Ok(())` if the server responded
  /// with *any* HTTP status (including 4xx / 5xx) — auth / API errors
  /// are not connectivity errors and are handled by the real call.
  pub async fn probe_endpoint(
      service: ServiceKind,
      provider_name: &str,
      base_url: &str,
      probe_path: &str,
      bearer: Option<&str>,
  ) -> Result<(), AppError> {
      let client = Client::builder()
          .timeout(PROBE_TIMEOUT)
          .build()
          .map_err(|e| AppError::Config(format!("preflight client build failed: {e}")))?;

      let url = format!("{}{}", base_url.trim_end_matches('/'), probe_path);
      let mut req = client.get(&url);
      if let Some(b) = bearer {
          req = req.header("Authorization", format!("Bearer {b}"));
      }

      let start = std::time::Instant::now();
      let result = req.send().await;
      let elapsed_ms = start.elapsed().as_millis();

      match result {
          Ok(_response) => {
              // Any HTTP status counts as "reachable" for our purposes.
              debug!(
                  provider = provider_name,
                  url = %url,
                  elapsed_ms,
                  "preflight probe reachable"
              );
              Ok(())
          }
          Err(e) => {
              let reason = classify_reqwest_error(&e)
                  .unwrap_or(OfflineReason::ConnectionRefused);
              warn!(
                  provider = provider_name,
                  url = %url,
                  elapsed_ms,
                  reason = ?reason,
                  "preflight probe failed"
              );
              Err(AppError::EndpointOffline {
                  service,
                  endpoint: base_url.to_string(),
                  reason,
                  provider_name: provider_name.to_string(),
              })
          }
      }
  }
  ```

- [ ] **Step 3: Wire the module into `crates/core/src/lib.rs`**

  Open `crates/core/src/lib.rs` and add the new module declaration alongside the others (alphabetical placement is fine):

  ```rust
  pub mod preflight;
  ```

- [ ] **Step 4: Verify it compiles**

  Run: `cargo build -p medical-core`

  Expected: clean build (warnings are acceptable; errors are not).

- [ ] **Step 5: Write the `probe_endpoint` happy-path test**

  Append a `tests` module to `crates/core/src/preflight.rs`:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use wiremock::matchers::{method, path};
      use wiremock::{Mock, MockServer, ResponseTemplate};

      #[tokio::test]
      async fn probe_returns_ok_on_2xx() {
          let server = MockServer::start().await;
          Mock::given(method("GET"))
              .and(path("/api/tags"))
              .respond_with(ResponseTemplate::new(200).set_body_string("{\"models\":[]}"))
              .mount(&server)
              .await;

          let result = probe_endpoint(
              ServiceKind::AiProvider,
              "Ollama",
              &server.uri(),
              "/api/tags",
              None,
          )
          .await;

          assert!(result.is_ok(), "200 response should be Ok; got {result:?}");
      }

      #[tokio::test]
      async fn probe_returns_ok_on_5xx() {
          let server = MockServer::start().await;
          Mock::given(method("GET"))
              .and(path("/api/tags"))
              .respond_with(ResponseTemplate::new(503))
              .mount(&server)
              .await;

          let result = probe_endpoint(
              ServiceKind::AiProvider,
              "Ollama",
              &server.uri(),
              "/api/tags",
              None,
          )
          .await;

          assert!(
              result.is_ok(),
              "5xx response means server is reachable; got {result:?}"
          );
      }
  }
  ```

- [ ] **Step 6: Run the happy-path tests**

  Run: `cargo test -p medical-core --lib preflight::tests::probe_returns_ok_on_2xx preflight::tests::probe_returns_ok_on_5xx`

  Expected: PASS.

- [ ] **Step 7: Add the connection-refused test**

  Append to the `tests` module:

  ```rust
  #[tokio::test]
  async fn probe_returns_connection_refused_when_no_server() {
      // Bind a TcpListener to get a free port, then drop it. The OS will
      // refuse any subsequent connection on that port (until something
      // else binds it). This is the canonical wiremock-free "connection
      // refused" pattern.
      let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
      let port = listener.local_addr().unwrap().port();
      drop(listener);
      let base = format!("http://127.0.0.1:{port}");

      let result = probe_endpoint(
          ServiceKind::AiProvider,
          "Ollama",
          &base,
          "/api/tags",
          None,
      )
      .await;

      let err = result.expect_err("must error when port is closed");
      match err {
          AppError::EndpointOffline {
              service,
              reason,
              provider_name,
              ..
          } => {
              assert_eq!(service, ServiceKind::AiProvider);
              assert_eq!(reason, OfflineReason::ConnectionRefused);
              assert_eq!(provider_name, "Ollama");
          }
          other => panic!("expected EndpointOffline, got {other:?}"),
      }
  }
  ```

- [ ] **Step 8: Run it**

  Run: `cargo test -p medical-core --lib preflight::tests::probe_returns_connection_refused_when_no_server`

  Expected: PASS.

- [ ] **Step 9: Add the timeout test**

  Append:

  ```rust
  #[tokio::test]
  async fn probe_returns_timeout_when_server_hangs() {
      let server = MockServer::start().await;
      Mock::given(method("GET"))
          .and(path("/api/tags"))
          .respond_with(
              ResponseTemplate::new(200)
                  .set_delay(Duration::from_secs(10)),
          )
          .mount(&server)
          .await;

      let result = probe_endpoint(
          ServiceKind::AiProvider,
          "Ollama",
          &server.uri(),
          "/api/tags",
          None,
      )
      .await;

      let err = result.expect_err("must error when server exceeds timeout");
      match err {
          AppError::EndpointOffline { reason, .. } => {
              assert_eq!(reason, OfflineReason::Timeout);
          }
          other => panic!("expected EndpointOffline, got {other:?}"),
      }
  }
  ```

  Run: `cargo test -p medical-core --lib preflight::tests::probe_returns_timeout_when_server_hangs`

  Expected: PASS (test takes ~3s — that's the probe timeout firing).

- [ ] **Step 10: Add the DNS-failure test**

  Append:

  ```rust
  #[tokio::test]
  async fn probe_returns_dns_failure_for_nonexistent_host() {
      // .invalid is reserved by RFC 2606 to always fail DNS.
      let result = probe_endpoint(
          ServiceKind::AiProvider,
          "Ollama",
          "http://nonexistent.invalid:11434",
          "/api/tags",
          None,
      )
      .await;

      let err = result.expect_err("must error for unresolvable host");
      match err {
          AppError::EndpointOffline { reason, .. } => {
              assert!(
                  matches!(reason, OfflineReason::DnsFailure | OfflineReason::ConnectionRefused),
                  "DNS failure preferred, but ConnectionRefused acceptable if the platform's \
                   resolver error doesn't include 'dns' in its source chain; got {reason:?}"
              );
          }
          other => panic!("expected EndpointOffline, got {other:?}"),
      }
  }
  ```

  Run: `cargo test -p medical-core --lib preflight::tests::probe_returns_dns_failure_for_nonexistent_host`

  Expected: PASS (the assertion is permissive about DNS-vs-ConnectionRefused because the source-chain string varies by platform/runtime).

- [ ] **Step 11: Add the bearer-header test**

  Append:

  ```rust
  #[tokio::test]
  async fn probe_sends_bearer_when_provided() {
      let server = MockServer::start().await;
      Mock::given(method("GET"))
          .and(path("/v1/models"))
          .and(wiremock::matchers::header("authorization", "Bearer s3cret"))
          .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
          .mount(&server)
          .await;

      let result = probe_endpoint(
          ServiceKind::RemoteStt,
          "Whisper STT",
          &server.uri(),
          "/v1/models",
          Some("s3cret"),
      )
      .await;

      assert!(result.is_ok(), "bearer-protected 200 should be Ok; got {result:?}");
  }
  ```

  Run: `cargo test -p medical-core --lib preflight::tests::probe_sends_bearer_when_provided`

  Expected: PASS.

- [ ] **Step 12: Run the full preflight test module**

  Run: `cargo test -p medical-core --lib preflight`

  Expected: 5 tests pass.

- [ ] **Step 13: Commit**

  ```bash
  git add crates/core/Cargo.toml crates/core/src/lib.rs crates/core/src/preflight.rs
  git commit -m "feat(core): add probe_endpoint and classify_reqwest_error

Single-shot HTTP probe with 3s timeout, classifying reqwest errors
into OfflineReason (ConnectionRefused / Timeout / DnsFailure /
TlsFailure). Any HTTP status counts as reachable — auth and 5xx
are handled by the real call.

Adds wiremock as a dev-dep on medical-core."
  ```

---

## Task 3: Add `CommandKind` and `preflight_for_command` with loopback skip

**Files:**
- Modify: `crates/core/src/preflight.rs`

**Why:** Each Tauri command has a fixed set of remote endpoints it depends on (e.g. `generate_soap` → active AI provider; `transcribe_recording` → remote STT *iff configured*). The orchestrator picks the right probe(s) from settings, runs them in parallel, and short-circuits on the first failure. Loopback endpoints (localhost / 127.0.0.1) are skipped because the dialog's wording ("your Mac is asleep") doesn't apply and the call-site mapper still produces `EndpointOffline` if the local server is actually down.

- [ ] **Step 1: Write the failing test for `CommandKind` and the loopback skip**

  Append to `crates/core/src/preflight.rs`'s test module:

  ```rust
  use crate::types::settings::AppConfig;

  fn settings_pointing_at(ai_provider: &str, host: &str, port: u16) -> AppConfig {
      let mut cfg = AppConfig::default();
      cfg.ai_provider = ai_provider.into();
      match ai_provider {
          "ollama" => {
              cfg.ollama_host = host.into();
              cfg.ollama_port = port;
          }
          "lmstudio" => {
              cfg.lmstudio_host = host.into();
              cfg.lmstudio_port = port;
          }
          _ => panic!("unknown ai_provider: {ai_provider}"),
      }
      cfg
  }

  #[tokio::test]
  async fn preflight_skips_loopback_ollama() {
      // 127.0.0.1:1 is definitely unreachable; if preflight tried to probe it
      // we'd see EndpointOffline. The skip rule means it never tries, so we
      // get Ok.
      let cfg = settings_pointing_at("ollama", "127.0.0.1", 1);
      let result = preflight_for_command(CommandKind::GenerateSoap, &cfg).await;
      assert!(result.is_ok(), "loopback should be skipped; got {result:?}");
  }

  #[tokio::test]
  async fn preflight_skips_localhost_lmstudio() {
      let cfg = settings_pointing_at("lmstudio", "localhost", 1);
      let result = preflight_for_command(CommandKind::GenerateSoap, &cfg).await;
      assert!(result.is_ok(), "localhost should be skipped; got {result:?}");
  }

  #[tokio::test]
  async fn preflight_skips_empty_host_lmstudio() {
      // The Settings UI uses empty-host to mean "use the default (localhost)".
      // We treat empty host as loopback for skip purposes.
      let cfg = settings_pointing_at("lmstudio", "", 1);
      let result = preflight_for_command(CommandKind::GenerateSoap, &cfg).await;
      assert!(result.is_ok(), "empty host should be skipped; got {result:?}");
  }

  #[tokio::test]
  async fn preflight_returns_endpoint_offline_for_unreachable_remote_ollama() {
      let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
      let port = listener.local_addr().unwrap().port();
      drop(listener);

      // Pick a non-loopback IP that routes to the local machine on most
      // platforms but isn't 127.0.0.1 / localhost. macOS / Linux assign
      // 127.0.0.2 et al. to loopback; we want a non-skipped host. Use
      // 192.0.2.1 (TEST-NET-1, RFC 5737) — guaranteed unroutable, which
      // gives us a clean ConnectionRefused or Timeout result.
      let mut cfg = AppConfig::default();
      cfg.ai_provider = "ollama".into();
      cfg.ollama_host = "192.0.2.1".into();
      cfg.ollama_port = port; // any port; the host is unrouteable

      let result = preflight_for_command(CommandKind::GenerateSoap, &cfg).await;
      let err = result.expect_err("unrouteable host must fail preflight");
      assert!(matches!(err, AppError::EndpointOffline { .. }));
  }

  #[tokio::test]
  async fn preflight_transcribe_skips_when_no_stt_remote_configured() {
      let mut cfg = AppConfig::default();
      cfg.stt_remote_host = "".into(); // not configured → use local whisper
      cfg.stt_remote_port = 8080;
      let result = preflight_for_command(CommandKind::Transcribe, &cfg).await;
      assert!(
          result.is_ok(),
          "transcribe with no remote STT configured should skip preflight; got {result:?}"
      );
  }
  ```

- [ ] **Step 2: Run the tests to confirm compile failure**

  Run: `cargo test -p medical-core --lib preflight::tests::preflight_skips_loopback_ollama`

  Expected: build error — `CommandKind`, `preflight_for_command` not defined.

- [ ] **Step 3: Add `CommandKind` and `preflight_for_command`**

  In `crates/core/src/preflight.rs`, add above the `tests` module:

  ```rust
  use crate::types::settings::AppConfig;

  /// Which Tauri command is about to run. Drives which endpoint(s) are
  /// probed by `preflight_for_command`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum CommandKind {
      Transcribe,
      GenerateSoap,
      GenerateReferral,
      GenerateLetter,
      GenerateSynopsis,
      Chat,
  }

  /// Inspect settings, decide which remote endpoints this command needs,
  /// probe each in parallel with a 3s timeout, return Ok(()) if all are
  /// reachable (or skipped) and the first `EndpointOffline` error otherwise.
  ///
  /// Endpoints whose host is loopback (127.0.0.1, ::1, localhost, "")
  /// are skipped entirely — failures from local servers surface via the
  /// real call's error mapper using the same `EndpointOffline` variant.
  pub async fn preflight_for_command(
      kind: CommandKind,
      settings: &AppConfig,
  ) -> Result<(), AppError> {
      let mut futures: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AppError>> + Send>>> = Vec::new();

      let ai_needed = matches!(
          kind,
          CommandKind::GenerateSoap
              | CommandKind::GenerateReferral
              | CommandKind::GenerateLetter
              | CommandKind::GenerateSynopsis
              | CommandKind::Chat,
      );
      let stt_needed = matches!(kind, CommandKind::Transcribe);

      if ai_needed {
          if let Some(probe) = build_ai_probe(settings) {
              futures.push(Box::pin(probe));
          }
      }
      if stt_needed {
          if let Some(probe) = build_stt_probe(settings) {
              futures.push(Box::pin(probe));
          }
      }

      if futures.is_empty() {
          return Ok(());
      }

      // Run all probes concurrently; return the first error if any.
      let results = futures::future::join_all(futures).await;
      for r in results {
          r?;
      }
      Ok(())
  }

  /// Returns `Some(future)` if the active AI provider has a non-loopback
  /// host worth probing; `None` if it's local or empty.
  fn build_ai_probe(
      settings: &AppConfig,
  ) -> Option<impl std::future::Future<Output = Result<(), AppError>> + Send + 'static> {
      let (provider_name, host, port, probe_path) = match settings.ai_provider.as_str() {
          "ollama" => (
              "Ollama",
              settings.ollama_host.clone(),
              settings.ollama_port,
              "/api/tags",
          ),
          "lmstudio" => (
              "LM Studio",
              settings.lmstudio_host.clone(),
              settings.lmstudio_port,
              "/v1/models",
          ),
          _ => return None, // unknown provider: skip; caller will surface a config error
      };
      if is_loopback_host(&host) {
          return None;
      }
      let base_url = format!("http://{host}:{port}");
      Some(async move {
          probe_endpoint(
              ServiceKind::AiProvider,
              provider_name,
              &base_url,
              probe_path,
              None,
          )
          .await
      })
  }

  /// Returns `Some(future)` only if the user has configured a remote STT
  /// endpoint (non-empty host); `None` if STT is fully local (default).
  fn build_stt_probe(
      settings: &AppConfig,
  ) -> Option<impl std::future::Future<Output = Result<(), AppError>> + Send + 'static> {
      let host = settings.stt_remote_host.clone();
      if host.is_empty() || is_loopback_host(&host) {
          return None;
      }
      let port = settings.stt_remote_port;
      let base_url = format!("http://{host}:{port}");
      Some(async move {
          probe_endpoint(
              ServiceKind::RemoteStt,
              "Whisper STT",
              &base_url,
              "/v1/models",
              None, // bearer is added by the real call site if configured
          )
          .await
      })
  }

  /// True for loopback / empty hosts that should bypass preflight.
  fn is_loopback_host(host: &str) -> bool {
      if host.is_empty() {
          return true;
      }
      let h = host.trim().to_ascii_lowercase();
      if h == "localhost" || h == "::1" {
          return true;
      }
      // Parse as IP; treat any 127.0.0.0/8 address as loopback.
      h.parse::<std::net::IpAddr>()
          .map(|ip| ip.is_loopback())
          .unwrap_or(false)
  }
  ```

  Add a dependency on `futures` to `crates/core/Cargo.toml` if it isn't already there. Check first:

  Run: `grep '^futures' crates/core/Cargo.toml`

  If absent, add to `[dependencies]`:

  ```toml
  futures = { workspace = true }
  ```

  (The workspace already exports `futures` per the existing codebase patterns. If `cargo build` fails with "no such workspace dependency", inspect `Cargo.toml` at the workspace root and pin the version from there.)

- [ ] **Step 4: Build it**

  Run: `cargo build -p medical-core --tests`

  Expected: clean build.

- [ ] **Step 5: Run the new tests**

  Run: `cargo test -p medical-core --lib preflight::tests`

  Expected: 10 tests pass (5 from Task 2 + 5 new).

  If the `preflight_returns_endpoint_offline_for_unreachable_remote_ollama` test takes ~3s, that's expected (probe timeout). If it takes much longer, the test platform's networking is the cause — consider increasing the probe timeout there only by setting a per-test value (TBD only if the test is flaky in practice).

- [ ] **Step 6: Add a loopback unit test**

  Append:

  ```rust
  #[test]
  fn is_loopback_host_recognizes_common_forms() {
      assert!(is_loopback_host(""));
      assert!(is_loopback_host("localhost"));
      assert!(is_loopback_host("LOCALHOST"));
      assert!(is_loopback_host("127.0.0.1"));
      assert!(is_loopback_host("127.42.0.1")); // 127/8
      assert!(is_loopback_host("::1"));
      assert!(!is_loopback_host("192.168.1.10"));
      assert!(!is_loopback_host("ollama.local"));
      assert!(!is_loopback_host("10.0.0.1"));
  }
  ```

  Run: `cargo test -p medical-core --lib preflight::tests::is_loopback_host_recognizes_common_forms`

  Expected: PASS.

- [ ] **Step 7: Commit**

  ```bash
  git add crates/core/src/preflight.rs crates/core/Cargo.toml
  git commit -m "feat(core): add CommandKind and preflight_for_command orchestrator

Pre-flight picks endpoints from AppConfig per command, runs probes in
parallel, returns the first EndpointOffline on failure. Loopback /
empty / 127/8 hosts are skipped — local-server failures still surface
via the call-site mapper (Task 5+)."
  ```

---

## Task 4: Refactor `providers.rs` test_*_connection to use `classify_reqwest_error`

**Files:**
- Modify: `src-tauri/src/commands/providers.rs:96-110, 161-175, 231-244`

**Why:** The Settings "Test Connection" buttons keep their existing string-return shape (per spec acceptance criteria 4), but the inline `is_connect()` / `is_timeout()` mapping in three nearly-identical blocks should call `classify_reqwest_error` for consistency. This is a small DRY win that also pins the providers commands to the same classification logic the pre-flight uses — so a probe failure and a test-connection failure can never disagree on what counts as "offline."

- [ ] **Step 1: Confirm the existing tests pass**

  Run: `cargo test -p rust-medical-assistant --lib test_lmstudio_connection test_ollama_connection test_stt_remote_connection`

  Expected: existing tests pass. (If there are no existing tests for these commands, that's fine — the refactor is behavior-preserving and Step 4 adds a new unit test.)

- [ ] **Step 2: Replace the LM Studio inline mapping**

  In `src-tauri/src/commands/providers.rs`, locate the LM Studio mapping at lines 96–110 and replace the entire `.map_err(|e| { … })?` closure with:

  ```rust
  .map_err(|e| {
      use medical_core::error::OfflineReason;
      use medical_core::preflight::classify_reqwest_error;
      match classify_reqwest_error(&e) {
          Some(OfflineReason::ConnectionRefused) => AppError::AiProvider(format!(
              "Connection refused — is LM Studio running at {}:{}?",
              effective_host, port
          )),
          Some(OfflineReason::Timeout) => AppError::AiProvider(format!(
              "Connection timed out — check that {}:{} is reachable",
              effective_host, port
          )),
          Some(OfflineReason::DnsFailure) => AppError::AiProvider(format!(
              "Cannot resolve hostname '{}'", effective_host
          )),
          Some(OfflineReason::TlsFailure) => AppError::AiProvider(format!(
              "TLS handshake failed at {}:{}", effective_host, port
          )),
          None => AppError::AiProvider(format!("Connection failed: {e}")),
      }
  })?;
  ```

- [ ] **Step 3: Replace the STT remote inline mapping**

  At lines 161–175 (STT remote), replace the closure with the same pattern, swapping `AppError::AiProvider` → `AppError::SttProvider` and "LM Studio" → "the Whisper server":

  ```rust
  .map_err(|e| {
      use medical_core::error::OfflineReason;
      use medical_core::preflight::classify_reqwest_error;
      match classify_reqwest_error(&e) {
          Some(OfflineReason::ConnectionRefused) => AppError::SttProvider(format!(
              "Connection refused — is the Whisper server running at {}:{}?",
              effective_host, port
          )),
          Some(OfflineReason::Timeout) => AppError::SttProvider(format!(
              "Connection timed out — check that {}:{} is reachable",
              effective_host, port
          )),
          Some(OfflineReason::DnsFailure) => AppError::SttProvider(format!(
              "Cannot resolve hostname '{}'", effective_host
          )),
          Some(OfflineReason::TlsFailure) => AppError::SttProvider(format!(
              "TLS handshake failed at {}:{}", effective_host, port
          )),
          None => AppError::SttProvider(format!("Connection failed: {e}")),
      }
  })?;
  ```

- [ ] **Step 4: Replace the Ollama inline mapping**

  At lines 231–244 (Ollama), apply the same pattern, "is Ollama running at" wording:

  ```rust
  .map_err(|e| {
      use medical_core::error::OfflineReason;
      use medical_core::preflight::classify_reqwest_error;
      match classify_reqwest_error(&e) {
          Some(OfflineReason::ConnectionRefused) => AppError::AiProvider(format!(
              "Connection refused — is Ollama running at {}:{}?",
              effective_host, port
          )),
          Some(OfflineReason::Timeout) => AppError::AiProvider(format!(
              "Connection timed out — check that {}:{} is reachable",
              effective_host, port
          )),
          Some(OfflineReason::DnsFailure) => AppError::AiProvider(format!(
              "Cannot resolve hostname '{}'", effective_host
          )),
          Some(OfflineReason::TlsFailure) => AppError::AiProvider(format!(
              "TLS handshake failed at {}:{}", effective_host, port
          )),
          None => AppError::AiProvider(format!("Connection failed: {e}")),
      }
  })?;
  ```

- [ ] **Step 5: Build and run all providers tests**

  Run: `cargo test -p rust-medical-assistant --lib providers`

  Expected: existing tests pass. Behavior is preserved — the user-visible strings are unchanged, only the source of truth for classification moved.

- [ ] **Step 6: Commit**

  ```bash
  git add src-tauri/src/commands/providers.rs
  git commit -m "refactor(providers): use shared classify_reqwest_error in test_* commands

Inline is_connect / is_timeout matching is replaced with calls to
medical_core::preflight::classify_reqwest_error. User-visible
strings unchanged. Ensures test-connection and pre-flight cannot
disagree on what counts as 'offline'."
  ```

---

## Task 5: Map `remote_provider.rs` reqwest errors to `EndpointOffline`

**Files:**
- Modify: `crates/stt-providers/src/remote_provider.rs:236-251`

**Why:** This is the only call site in the STT path that touches `is_connect()` / `is_timeout()` today. The `base` variable is already in scope; we just swap the error variant. Other STT error paths (5xx, auth) are correctly the user's problem and keep their existing wording.

- [ ] **Step 1: Write a failing integration test**

  Append to `crates/stt-providers/src/remote_provider.rs`'s test module (or create one at the bottom if none exists; check first with `grep -n "^#\[cfg(test)\]" crates/stt-providers/src/remote_provider.rs`):

  ```rust
  #[cfg(test)]
  mod offline_tests {
      use super::*;
      use medical_core::error::{OfflineReason, ServiceKind};

      #[tokio::test]
      async fn transcribe_returns_endpoint_offline_when_remote_unreachable() {
          // Bind+drop to get a guaranteed-refused port.
          let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
          let port = listener.local_addr().unwrap().port();
          drop(listener);
          let base = format!("http://127.0.0.1:{port}");

          // Construct a RemoteWhisperProvider pointed at the dead port.
          // The exact constructor signature lives in remote_provider.rs;
          // use whatever the existing tests / production callers use.
          // (If the constructor wants a config, build the minimal one.)
          let provider = RemoteWhisperProvider::new(/* see existing tests for arg shape */);
          // [TODO replaced during implementation: call provider.transcribe(...)
          //  with a tiny dummy audio payload. The point is to exercise the
          //  error-mapping arm at lines 236-251 of remote_provider.rs.]

          // Pseudocode of the assertion:
          // let err = provider.transcribe(&dummy_audio).await.unwrap_err();
          // match err {
          //     AppError::EndpointOffline { service, reason, .. } => {
          //         assert_eq!(service, ServiceKind::RemoteStt);
          //         assert!(matches!(reason, OfflineReason::ConnectionRefused));
          //     }
          //     other => panic!("expected EndpointOffline, got {other:?}"),
          // }

          // The implementor must fill in the constructor + transcribe-call
          // shape by reading the existing tests in this file. The structure
          // above is the contract this task delivers.
          let _ = (base, provider);
      }
  }
  ```

  **Note:** the test stub uses placeholders for the constructor call shape because the exact signature isn't visible in the file excerpt the plan was built from. Read `crates/stt-providers/src/remote_provider.rs` end-to-end before writing the test body; copy the construction pattern from any existing test in the same file, or from the call site in `src-tauri/src/state.rs` if no test exists.

- [ ] **Step 2: Replace the error-mapping arms**

  Locate lines 236–251 in `crates/stt-providers/src/remote_provider.rs`. The current shape is:

  ```rust
  result.map_err(|e| {
      if e.is_timeout() {
          AppError::SttProvider(format!(
              "Transcription timed out after {}s",
              TRANSCRIBE_TIMEOUT.as_secs()
          ))
      } else if e.is_connect() {
          AppError::SttProvider(format!(
              "Cannot reach Whisper server at {base}: {e}"
          ))
      } else {
          AppError::SttProvider(format!("Whisper request failed: {e}"))
      }
  })?
  ```

  Replace with:

  ```rust
  result.map_err(|e| {
      use medical_core::error::{OfflineReason, ServiceKind};
      use medical_core::preflight::classify_reqwest_error;
      match classify_reqwest_error(&e) {
          Some(reason) => AppError::EndpointOffline {
              service: ServiceKind::RemoteStt,
              endpoint: base.clone(),
              reason,
              provider_name: "Whisper STT".into(),
          },
          None => AppError::SttProvider(format!("Whisper request failed: {e}")),
      }
  })?
  ```

  This collapses the timeout, connect-refused, DNS, and TLS arms into a single `EndpointOffline` (the `reason` field carries the distinction). The fall-through for non-connectivity errors keeps the existing `SttProvider(String)` shape.

- [ ] **Step 3: Build and run the integration test**

  Run: `cargo test -p medical-stt-providers --lib offline_tests`

  Expected: the integration test passes (after the implementor fills in the constructor + transcribe-call shape per Step 1's note). If the existing tests in this file break, that means another call site references the removed string format — read the failure and adjust.

- [ ] **Step 4: Verify the old strings are gone**

  Run: `grep -n "Cannot reach Whisper server\|Transcription timed out after" crates/stt-providers/src/remote_provider.rs`

  Expected: zero matches (the strings now live only in the test snapshots, if any).

- [ ] **Step 5: Commit**

  ```bash
  git add crates/stt-providers/src/remote_provider.rs
  git commit -m "feat(stt-providers): map remote_provider errors to EndpointOffline

The is_connect / is_timeout branches in the multipart send no longer
produce string-formatted SttProvider errors. They now build
AppError::EndpointOffline { service: RemoteStt, endpoint, reason,
provider_name: 'Whisper STT' } so the frontend can render the
plain-language dialog. Non-connectivity errors retain the existing
SttProvider(String) shape."
  ```

---

## Task 6: Map `ollama.rs` and `lmstudio.rs` errors to `EndpointOffline`

**Files:**
- Modify: `crates/ai-providers/src/ollama.rs:119-126` (and any downstream HTTP send site)
- Modify: `crates/ai-providers/src/lmstudio.rs:120-127` (same)

**Why:** Two sites per file:
1. `current_base_url()` returns `"Office server unreachable on LAN or Tailscale (Ollama|LM Studio)."` when the `RemoteEndpoint` LAN/Tailscale probe finds no reachable URL. This case has no single endpoint string, so we synthesise one from the RemoteEndpoint's primary LAN address.
2. The downstream HTTP call inside `openai_compat::OpenAiCompatibleClient` — if it returns a reqwest error with `is_connect()` / `is_timeout()`, that should also become `EndpointOffline`. Read `crates/ai-providers/src/openai_compat.rs` (or wherever the shared client lives) to find the exact mapping site.

- [ ] **Step 1: Locate the second mapping site**

  Run: `grep -n 'is_connect\|is_timeout\|AppError::AiProvider' crates/ai-providers/src/`

  Find the call site inside the shared HTTP client that maps reqwest errors into `AppError`. If it doesn't currently classify connect/timeout (the current code may bubble a raw reqwest error string), the mapping needs to be added — make a note of the file and approximate line.

- [ ] **Step 2: Replace the RemoteEndpoint-resolution error in `ollama.rs`**

  In `crates/ai-providers/src/ollama.rs:119-126`, the current code is:

  ```rust
  let resolved = ep
      .resolve_base_url()
      .await
      .ok_or_else(|| {
          AppError::AiProvider(
              "Office server unreachable on LAN or Tailscale (Ollama).".to_string(),
          )
      })?;
  ```

  Replace with:

  ```rust
  let resolved = ep
      .resolve_base_url()
      .await
      .ok_or_else(|| AppError::EndpointOffline {
          service: medical_core::error::ServiceKind::AiProvider,
          // For the RemoteEndpoint case, we don't have a single URL — the
          // probe tried LAN then Tailscale. Use the LAN URL as the canonical
          // "endpoint" the dialog shows; the user fix is the same either way.
          endpoint: ep.lan_url().unwrap_or_else(|| "unknown".into()),
          reason: medical_core::error::OfflineReason::ConnectionRefused,
          provider_name: "Ollama".into(),
      })?;
  ```

  **If `ep.lan_url()` doesn't exist as a method:** read `crates/core/src/types/endpoint.rs` (where `RemoteEndpoint` is defined) and use whichever accessor returns the primary LAN URL. If no accessor exists, add one — a one-line `pub fn` is fine. Update the call site here once it does.

- [ ] **Step 3: Same change in `lmstudio.rs:120-127`**

  Identical structure, swap `"Ollama"` → `"LM Studio"`.

- [ ] **Step 4: Map the shared HTTP client's reqwest errors (if applicable)**

  Inside the shared client identified in Step 1, find the `.send().await.map_err(...)` site and add an early classification:

  ```rust
  .map_err(|e| {
      use medical_core::error::{OfflineReason, ServiceKind};
      use medical_core::preflight::classify_reqwest_error;
      if let Some(reason) = classify_reqwest_error(&e) {
          AppError::EndpointOffline {
              service: ServiceKind::AiProvider,
              endpoint: self.base_url.clone(),
              reason,
              // provider_name carried in via constructor; if it isn't,
              // pipe it in. If the shared client serves both Ollama and
              // LM Studio, store provider_name as a String field on the
              // client struct (one-line addition).
              provider_name: self.provider_name.clone(),
          }
      } else {
          AppError::AiProvider(format!("{e}"))
      }
  })?
  ```

  **If `self.provider_name` doesn't exist:** add it as a `String` field to the client struct and populate it from the constructor calls in `ollama.rs` / `lmstudio.rs` (both pass `"Ollama"` / `"LM Studio"` respectively).

- [ ] **Step 5: Write tests for both mapping paths**

  Add to `crates/ai-providers/src/ollama.rs` (or a sibling test file):

  ```rust
  #[cfg(test)]
  mod offline_tests {
      use super::*;
      use medical_core::error::{AppError, OfflineReason, ServiceKind};

      #[tokio::test]
      async fn ollama_complete_returns_endpoint_offline_when_host_refused() {
          let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
          let port = listener.local_addr().unwrap().port();
          drop(listener);

          // Construct OllamaProvider with the dead port. Use whatever the
          // existing test code uses; if none, copy from the production call
          // site in src-tauri/src/state.rs.
          let provider = OllamaProvider::new_for_test(format!("http://127.0.0.1:{port}"));

          let req = /* minimal CompletionRequest the provider accepts */;
          let err = provider.complete(req).await.unwrap_err();
          match err {
              AppError::EndpointOffline { service, reason, provider_name, .. } => {
                  assert_eq!(service, ServiceKind::AiProvider);
                  assert_eq!(reason, OfflineReason::ConnectionRefused);
                  assert_eq!(provider_name, "Ollama");
              }
              other => panic!("expected EndpointOffline, got {other:?}"),
          }
      }
  }
  ```

  Add the analogous test in `lmstudio.rs`.

  **Constructor placeholder:** `OllamaProvider::new_for_test` may not exist. If the production constructor is `OllamaProvider::new(model: String, endpoint: Option<RemoteEndpoint>, base_url: String, ...)`, call that directly; otherwise add a small `#[cfg(test)] pub fn new_for_test(...)` that bypasses the parts of the real constructor that need an AppState.

- [ ] **Step 6: Build and run the AI-provider tests**

  Run: `cargo test -p medical-ai-providers`

  Expected: both new tests pass; existing tests pass; old "Office server unreachable on LAN or Tailscale" strings no longer appear in any error output.

- [ ] **Step 7: Verify the old strings are gone**

  Run: `grep -rn "Office server unreachable on LAN or Tailscale" crates/ai-providers/`

  Expected: zero matches.

- [ ] **Step 8: Commit**

  ```bash
  git add crates/ai-providers/src/
  git commit -m "feat(ai-providers): map ollama/lmstudio connect errors to EndpointOffline

Both providers' RemoteEndpoint-resolution failures and downstream
HTTP send connect/timeout errors now build AppError::EndpointOffline
with service: AiProvider and provider_name 'Ollama' / 'LM Studio'.
Non-connectivity errors (5xx, malformed JSON, auth) retain the
existing AiProvider(String) shape."
  ```

---

## Task 7: Wire pre-flight into `generate_soap`

**Files:**
- Modify: `src-tauri/src/commands/generation/soap.rs`

**Why:** First Tauri-command integration — the rest of the generation commands follow the exact same one-line change. We do this one first, prove out the test pattern, then fan out.

- [ ] **Step 1: Write the failing integration test**

  Append a `tests` module to `src-tauri/src/commands/generation/soap.rs` (or wherever the existing tests for this command live; check with `grep -rn "generate_soap_inner\|#\[tokio::test\]" src-tauri/src/commands/generation/`):

  ```rust
  #[cfg(test)]
  mod preflight_tests {
      use super::*;
      use medical_core::error::{AppError, OfflineReason, ServiceKind};

      #[tokio::test]
      async fn generate_soap_returns_endpoint_offline_when_ai_unreachable() {
          // [Implementor: build an AppState whose AppConfig has
          //  ai_provider="ollama", ollama_host="192.0.2.1" (RFC 5737
          //  TEST-NET-1, guaranteed unrouteable), ollama_port=11434.
          //  Insert a recording row with a non-empty transcript. The
          //  test's expectation: generate_soap_inner returns
          //  Err(AppError::EndpointOffline { .. }) WITHOUT invoking
          //  the provider's complete() — the easiest way to assert this
          //  is to measure elapsed time and assert < probe_timeout +
          //  ~500ms, since attempting the real call would take much
          //  longer.]

          // Pseudocode:
          // let state = build_test_state_with_unreachable_ollama().await;
          // let recording_id = insert_recording_with_transcript(&state).await;
          // let start = std::time::Instant::now();
          // let result = generate_soap_inner(&state, &recording_id, None, None, None).await;
          // let elapsed = start.elapsed();
          // let err = result.expect_err("must fail with offline error");
          // match err {
          //     AppError::EndpointOffline { service, reason, provider_name, .. } => {
          //         assert_eq!(service, ServiceKind::AiProvider);
          //         assert!(matches!(reason, OfflineReason::ConnectionRefused | OfflineReason::Timeout));
          //         assert_eq!(provider_name, "Ollama");
          //     }
          //     other => panic!("expected EndpointOffline, got {other:?}"),
          // }
          // assert!(elapsed < Duration::from_secs(5), "should have short-circuited; took {elapsed:?}");
      }
  }
  ```

  Fill in the helper functions by reading the existing generation tests in this file. The key invariant: pre-flight must short-circuit *before* the real call's `provider.complete()` runs.

- [ ] **Step 2: Add the pre-flight call at the top of `generate_soap_inner`**

  Open `src-tauri/src/commands/generation/soap.rs`. Locate `generate_soap_inner` (around line 93). After `let (mut recording, settings) = load_recording_and_settings(...).await?;` (line 100), add:

  ```rust
      // Pre-flight: probe the remote AI endpoint before doing any work.
      // Skipped for loopback hosts; returns EndpointOffline on failure
      // without ever invoking the provider.
      medical_core::preflight::preflight_for_command(
          medical_core::preflight::CommandKind::GenerateSoap,
          &settings,
      )
      .await?;
  ```

- [ ] **Step 3: Build and run the new test**

  Run: `cargo test -p rust-medical-assistant --lib generate_soap_returns_endpoint_offline_when_ai_unreachable`

  Expected: PASS — short-circuits in <5s and returns the structured error.

- [ ] **Step 4: Run the existing SOAP tests**

  Run: `cargo test -p rust-medical-assistant --lib generate_soap`

  Expected: all existing SOAP tests still pass. (Pre-flight is a no-op when the test's settings point at a reachable mock or use a localhost provider that's skipped.)

- [ ] **Step 5: Commit**

  ```bash
  git add src-tauri/src/commands/generation/soap.rs
  git commit -m "feat(generation): pre-flight check before SOAP generation

generate_soap_inner now calls preflight_for_command at the top.
Failures short-circuit with AppError::EndpointOffline before any
audio upload or provider.complete() call, capping the user-visible
latency on a dead server at ~3s instead of the full reqwest timeout."
  ```

---

## Task 8: Wire pre-flight into the remaining generation/chat/transcription commands

**Files:**
- Modify: `src-tauri/src/commands/generation/referral.rs`
- Modify: `src-tauri/src/commands/generation/letter.rs`
- Modify: `src-tauri/src/commands/generation/synopsis.rs`
- Modify: `src-tauri/src/commands/chat.rs`
- Modify: `src-tauri/src/commands/transcription/inner.rs`

**Why:** Same one-line pattern as Task 7, repeated per command. We don't merge the commands into a shared wrapper because they each have different signatures and different stages where settings are loaded — explicitness at each call site is clearer than another macro.

- [ ] **Step 1: Pre-flight in `generate_referral`**

  Open `src-tauri/src/commands/generation/referral.rs`. Locate the inner function (named like `generate_referral_inner`). Immediately after the settings are loaded and before any provider call:

  ```rust
      medical_core::preflight::preflight_for_command(
          medical_core::preflight::CommandKind::GenerateReferral,
          &settings,
      )
      .await?;
  ```

  Add a test mirroring the one in Task 7 (substitute `generate_referral_inner` for `generate_soap_inner` and load whatever fixture data the existing referral tests use).

- [ ] **Step 2: Same change in `letter.rs`**

  Use `CommandKind::GenerateLetter`.

- [ ] **Step 3: Same change in `synopsis.rs`**

  Use `CommandKind::GenerateSynopsis`.

- [ ] **Step 4: Same change in `chat.rs`**

  Use `CommandKind::Chat`. The chat command's structure may differ from generation (e.g. it may stream); add pre-flight before the first provider call regardless.

- [ ] **Step 5: Pre-flight in transcription**

  Open `src-tauri/src/commands/transcription/inner.rs`. Add the pre-flight at the top of the function that orchestrates the transcription pipeline (after settings load, before any STT-provider call):

  ```rust
      medical_core::preflight::preflight_for_command(
          medical_core::preflight::CommandKind::Transcribe,
          &settings,
      )
      .await?;
  ```

  Note: pre-flight is a no-op when STT is local (the `stt_remote_host` is empty), so this doesn't add latency for the local-whisper path.

- [ ] **Step 6: Run the full Tauri-app test suite**

  Run: `cargo test -p rust-medical-assistant --lib`

  Expected: all tests pass, including the new per-command pre-flight tests.

- [ ] **Step 7: Commit**

  ```bash
  git add src-tauri/src/commands/generation/ src-tauri/src/commands/chat.rs src-tauri/src/commands/transcription/inner.rs
  git commit -m "feat(commands): pre-flight check before referral/letter/synopsis/chat/transcribe

Same one-line preflight_for_command(...) at the top of each command's
inner function. Failures short-circuit with EndpointOffline; local
endpoints (or no remote STT configured) skip the probe entirely."
  ```

---

## Task 9: Frontend — `OfflineCancelled` sentinel and `endpointOffline` store

**Files:**
- Create: `src/lib/api/invokeWithOfflineHandling.ts`  *(skeleton — types + sentinel only)*
- Create: `src/lib/stores/endpointOffline.ts`
- Create: `src/lib/stores/endpointOffline.test.ts`

**Why:** Foundation: the types the dialog component reads and the store-API the helper composes on top of. Implementing the store first means the dialog (Task 10) and helper (Task 11) each have a tested integration point.

- [ ] **Step 1: Create the shared types file**

  Create `src/lib/api/invokeWithOfflineHandling.ts` with the minimum types needed by the store and dialog:

  ```ts
  /** Discriminant strings emitted by AppError::EndpointOffline. */
  export type ServiceKind = 'AiProvider' | 'RemoteStt';
  export type OfflineReason = 'ConnectionRefused' | 'Timeout' | 'DnsFailure' | 'TlsFailure';

  /** Decoded payload of an `EndpointOffline` rejection from a Tauri invoke. */
  export interface EndpointOfflinePayload {
    kind: 'EndpointOffline';
    service: ServiceKind;
    endpoint: string;
    reason: OfflineReason;
    provider_name: string;
    message: string;
  }

  /** User's choice in the offline dialog. */
  export type EndpointOfflineDecision = 'retry' | 'cancel' | 'opened_settings';

  /** Sentinel error thrown by `invokeWithOfflineHandling` when the user
   *  dismisses the offline dialog. Callers should `instanceof`-check
   *  and early-return silently — the dialog already explained the situation. */
  export class OfflineCancelled extends Error {
    constructor(public reason: 'cancel' | 'opened_settings') {
      super(`User dismissed offline dialog: ${reason}`);
      this.name = 'OfflineCancelled';
      // restore prototype chain for instanceof to work across module boundaries
      Object.setPrototypeOf(this, OfflineCancelled.prototype);
    }
  }

  /** Type guard for the EndpointOffline rejection shape. */
  export function isEndpointOffline(err: unknown): err is EndpointOfflinePayload {
    return (
      typeof err === 'object' &&
      err !== null &&
      (err as { kind?: unknown }).kind === 'EndpointOffline'
    );
  }

  // invokeWithOfflineHandling is implemented in Task 11.
  ```

- [ ] **Step 2: Create the store**

  Create `src/lib/stores/endpointOffline.ts`:

  ```ts
  import { writable, type Readable } from 'svelte/store';
  import type {
    EndpointOfflineDecision,
    EndpointOfflinePayload,
  } from '$lib/api/invokeWithOfflineHandling';

  interface OpenState {
    payload: EndpointOfflinePayload;
    resolve: (decision: EndpointOfflineDecision) => void;
  }

  function createStore() {
    const state = writable<OpenState | null>(null);
    let current: OpenState | null = null;
    state.subscribe((s) => (current = s));

    return {
      subscribe: state.subscribe as Readable<OpenState | null>['subscribe'],

      /** Open the dialog with `payload`; resolves when the user picks an
       *  action (retry / cancel / opened_settings). If `openAndWait` is
       *  called while another dialog is pending, the prior promise resolves
       *  with the new decision — matches the "modal at most one" rule. */
      openAndWait(payload: EndpointOfflinePayload): Promise<EndpointOfflineDecision> {
        return new Promise((resolve) => {
          // If a prior dialog is already pending, resolve it with the new
          // decision once the user picks. We do this by chaining: the new
          // resolver is the one we register; when it fires, we also resolve
          // the prior promise. (In practice this concurrency shouldn't
          // happen — only the helper opens dialogs and it awaits each one.)
          const priorResolve = current?.resolve;
          state.set({
            payload,
            resolve: (decision) => {
              priorResolve?.(decision);
              resolve(decision);
            },
          });
        });
      },

      /** Internal: dialog component calls this when the user picks an action. */
      _resolve(decision: EndpointOfflineDecision): void {
        const s = current;
        if (s) {
          state.set(null);
          s.resolve(decision);
        }
      },

      /** Imperative close without resolving — used in teardown / tests. */
      close(): void {
        state.set(null);
      },
    };
  }

  export const endpointOfflineStore = createStore();
  export type EndpointOfflineStore = typeof endpointOfflineStore;
  ```

- [ ] **Step 3: Write the store tests**

  Create `src/lib/stores/endpointOffline.test.ts`:

  ```ts
  import { describe, it, expect, beforeEach } from 'vitest';
  import { get } from 'svelte/store';
  import { endpointOfflineStore } from './endpointOffline';
  import type { EndpointOfflinePayload } from '$lib/api/invokeWithOfflineHandling';

  const samplePayload: EndpointOfflinePayload = {
    kind: 'EndpointOffline',
    service: 'AiProvider',
    endpoint: 'http://192.168.1.10:11434',
    reason: 'ConnectionRefused',
    provider_name: 'Ollama',
    message: 'Ollama at http://192.168.1.10:11434 is offline (ConnectionRefused)',
  };

  describe('endpointOfflineStore', () => {
    beforeEach(() => {
      endpointOfflineStore.close();
    });

    it('starts in a closed state', () => {
      expect(get(endpointOfflineStore)).toBeNull();
    });

    it('openAndWait populates state with the payload', () => {
      void endpointOfflineStore.openAndWait(samplePayload);
      const s = get(endpointOfflineStore);
      expect(s).not.toBeNull();
      expect(s?.payload).toEqual(samplePayload);
    });

    it('openAndWait resolves with retry when _resolve("retry") is called', async () => {
      const pending = endpointOfflineStore.openAndWait(samplePayload);
      endpointOfflineStore._resolve('retry');
      await expect(pending).resolves.toBe('retry');
      expect(get(endpointOfflineStore)).toBeNull();
    });

    it('openAndWait resolves with cancel', async () => {
      const pending = endpointOfflineStore.openAndWait(samplePayload);
      endpointOfflineStore._resolve('cancel');
      await expect(pending).resolves.toBe('cancel');
    });

    it('openAndWait resolves with opened_settings', async () => {
      const pending = endpointOfflineStore.openAndWait(samplePayload);
      endpointOfflineStore._resolve('opened_settings');
      await expect(pending).resolves.toBe('opened_settings');
    });

    it('concurrent open resolves prior promise with the new decision', async () => {
      const first = endpointOfflineStore.openAndWait(samplePayload);
      const second = endpointOfflineStore.openAndWait({
        ...samplePayload,
        provider_name: 'LM Studio',
      });
      endpointOfflineStore._resolve('cancel');
      await expect(first).resolves.toBe('cancel');
      await expect(second).resolves.toBe('cancel');
    });

    it('close() clears state without resolving', () => {
      void endpointOfflineStore.openAndWait(samplePayload);
      endpointOfflineStore.close();
      expect(get(endpointOfflineStore)).toBeNull();
    });
  });
  ```

- [ ] **Step 4: Run the tests**

  Run: `npx vitest run src/lib/stores/endpointOffline.test.ts`

  Expected: 7 tests pass.

- [ ] **Step 5: Commit**

  ```bash
  git add src/lib/api/invokeWithOfflineHandling.ts src/lib/stores/endpointOffline.ts src/lib/stores/endpointOffline.test.ts
  git commit -m "feat(frontend): endpointOffline store + types + OfflineCancelled sentinel

Svelte store with openAndWait(payload): Promise<Decision>. The
EndpointOfflinePayload + OfflineCancelled + isEndpointOffline types
live in invokeWithOfflineHandling.ts (the helper itself lands in
Task 11)."
  ```

---

## Task 10: Frontend — `EndpointOfflineDialog.svelte`

**Files:**
- Create: `src/lib/components/EndpointOfflineDialog.svelte`
- Create: `src/lib/components/EndpointOfflineDialog.test.ts`

**Why:** Pure presentation: subscribe to the store, render the dialog when populated, dispatch user actions back via the store's `_resolve` API. Matches the recent `ExportDialog.svelte` a11y pattern (commit `5596608`).

- [ ] **Step 1: Read the existing ExportDialog for a11y patterns**

  Run: `cat src/lib/components/ExportDialog.svelte`

  Note the focus-trap, Escape handler, backdrop click, default-focus on primary action, and aria roles. The new dialog should mirror these exactly.

- [ ] **Step 2: Create the dialog component**

  Create `src/lib/components/EndpointOfflineDialog.svelte`:

  ```svelte
  <script lang="ts">
    import { onDestroy } from 'svelte';
    import { endpointOfflineStore } from '$lib/stores/endpointOffline';
    import type { OfflineReason, EndpointOfflinePayload } from '$lib/api/invokeWithOfflineHandling';
    import { createEventDispatcher } from 'svelte';

    /** Fired when the user clicks "Open Settings". The parent should
     *  navigate to the correct Settings pane (Models for AiProvider,
     *  Audio for RemoteStt). */
    const dispatch = createEventDispatcher<{
      openSettings: { service: 'AiProvider' | 'RemoteStt' };
    }>();

    let state: { payload: EndpointOfflinePayload } | null = null;
    const unsub = endpointOfflineStore.subscribe((s) => (state = s));
    onDestroy(unsub);

    let dialogEl: HTMLDivElement | null = null;
    let retryBtn: HTMLButtonElement | null = null;

    $: if (state && retryBtn) {
      // Focus the primary action when the dialog opens.
      setTimeout(() => retryBtn?.focus(), 0);
    }

    function reasonSentence(payload: EndpointOfflinePayload): string {
      const { reason, provider_name, endpoint } = payload;
      switch (reason as OfflineReason) {
        case 'ConnectionRefused':
          return `The ${provider_name} server at ${endpoint} didn't respond.`;
        case 'Timeout':
          return `The ${provider_name} server at ${endpoint} took too long to respond.`;
        case 'DnsFailure':
          return `The address "${endpoint}" couldn't be found on the network.`;
        case 'TlsFailure':
          return `Couldn't establish a secure connection to ${provider_name} at ${endpoint}.`;
      }
    }

    function onRetry() {
      endpointOfflineStore._resolve('retry');
    }
    function onCancel() {
      endpointOfflineStore._resolve('cancel');
    }
    function onOpenSettings() {
      if (state) {
        dispatch('openSettings', { service: state.payload.service });
      }
      endpointOfflineStore._resolve('opened_settings');
    }

    function onKeydown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        e.preventDefault();
        onCancel();
      } else if (e.key === 'Tab' && dialogEl) {
        // Focus trap — keep tab cycling inside the dialog.
        const focusable = dialogEl.querySelectorAll<HTMLElement>(
          'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
        );
        if (focusable.length === 0) return;
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    }

    function onBackdrop(e: MouseEvent) {
      // Click outside the dialog box (i.e. on the backdrop) → cancel.
      if (e.target === e.currentTarget) {
        onCancel();
      }
    }
  </script>

  {#if state}
    <div
      class="backdrop"
      role="presentation"
      on:click={onBackdrop}
      on:keydown={onKeydown}
    >
      <div
        bind:this={dialogEl}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="endpoint-offline-title"
        aria-describedby="endpoint-offline-body"
        class="dialog"
      >
        <h2 id="endpoint-offline-title">Office server isn't responding</h2>
        <div id="endpoint-offline-body">
          <p>{reasonSentence(state.payload)}</p>
          <p>Common causes:</p>
          <ul>
            <li>The server app isn't running on your Mac</li>
            <li>Your Mac is asleep or has lost network</li>
            <li>The address in Settings has changed</li>
          </ul>
          <p><strong>Your recording is saved.</strong> You can process it once the server is back online.</p>
        </div>
        <div class="actions">
          <button type="button" class="secondary" on:click={onOpenSettings}>
            Open Settings
          </button>
          <button type="button" class="secondary" on:click={onCancel}>
            Cancel
          </button>
          <button type="button" class="primary" bind:this={retryBtn} on:click={onRetry}>
            Retry
          </button>
        </div>
      </div>
    </div>
  {/if}

  <style>
    .backdrop {
      position: fixed;
      inset: 0;
      background: rgba(0, 0, 0, 0.4);
      display: flex;
      align-items: center;
      justify-content: center;
      z-index: 1000;
    }
    .dialog {
      background: var(--color-bg, white);
      color: var(--color-text, #111);
      padding: 1.5rem;
      border-radius: 8px;
      max-width: 32rem;
      width: 90%;
      box-shadow: 0 8px 32px rgba(0, 0, 0, 0.2);
    }
    h2 { margin: 0 0 1rem 0; font-size: 1.25rem; }
    .actions {
      display: flex;
      gap: 0.5rem;
      justify-content: flex-end;
      margin-top: 1.5rem;
    }
    button {
      padding: 0.5rem 1rem;
      border: 1px solid var(--color-border, #ccc);
      border-radius: 4px;
      cursor: pointer;
      background: var(--color-bg, white);
      color: inherit;
    }
    button.primary {
      background: var(--color-primary, #2563eb);
      color: white;
      border-color: var(--color-primary, #2563eb);
    }
  </style>
  ```

  **Note on CSS variables:** if the codebase doesn't define `--color-bg`, `--color-primary` etc., either inline static colors or use whatever theme tokens the existing ExportDialog uses (read `src/lib/components/ExportDialog.svelte` and copy its style approach).

- [ ] **Step 3: Write the component tests**

  Create `src/lib/components/EndpointOfflineDialog.test.ts`:

  ```ts
  import { describe, it, expect, beforeEach } from 'vitest';
  import { render, fireEvent, screen } from '@testing-library/svelte';
  import EndpointOfflineDialog from './EndpointOfflineDialog.svelte';
  import { endpointOfflineStore } from '$lib/stores/endpointOffline';
  import type { EndpointOfflinePayload, OfflineReason } from '$lib/api/invokeWithOfflineHandling';

  function payload(overrides: Partial<EndpointOfflinePayload> = {}): EndpointOfflinePayload {
    return {
      kind: 'EndpointOffline',
      service: 'AiProvider',
      endpoint: 'http://192.168.1.10:11434',
      reason: 'ConnectionRefused',
      provider_name: 'Ollama',
      message: 'mock',
      ...overrides,
    };
  }

  describe('EndpointOfflineDialog', () => {
    beforeEach(() => endpointOfflineStore.close());

    it('does not render when store is empty', () => {
      const { container } = render(EndpointOfflineDialog);
      expect(container.querySelector('.dialog')).toBeNull();
    });

    it('renders title and the reassurance line when store is populated', async () => {
      render(EndpointOfflineDialog);
      void endpointOfflineStore.openAndWait(payload());
      await screen.findByText("Office server isn't responding");
      expect(screen.getByText(/Your recording is saved\./)).toBeInTheDocument();
    });

    const reasons: Array<[OfflineReason, RegExp]> = [
      ['ConnectionRefused', /didn't respond/],
      ['Timeout', /took too long to respond/],
      ['DnsFailure', /couldn't be found on the network/],
      ['TlsFailure', /Couldn't establish a secure connection/],
    ];

    it.each(reasons)('renders the right body for %s', async (reason, pattern) => {
      render(EndpointOfflineDialog);
      void endpointOfflineStore.openAndWait(payload({ reason }));
      await screen.findByText(pattern);
    });

    it('Retry button resolves openAndWait with retry', async () => {
      render(EndpointOfflineDialog);
      const pending = endpointOfflineStore.openAndWait(payload());
      const retryBtn = await screen.findByRole('button', { name: 'Retry' });
      await fireEvent.click(retryBtn);
      await expect(pending).resolves.toBe('retry');
    });

    it('Cancel button resolves with cancel', async () => {
      render(EndpointOfflineDialog);
      const pending = endpointOfflineStore.openAndWait(payload());
      const cancelBtn = await screen.findByRole('button', { name: 'Cancel' });
      await fireEvent.click(cancelBtn);
      await expect(pending).resolves.toBe('cancel');
    });

    it('Open Settings dispatches event and resolves with opened_settings', async () => {
      const handler = vi.fn();
      const { component } = render(EndpointOfflineDialog);
      component.$on('openSettings', handler);
      const pending = endpointOfflineStore.openAndWait(payload({ service: 'RemoteStt' }));
      const settingsBtn = await screen.findByRole('button', { name: 'Open Settings' });
      await fireEvent.click(settingsBtn);
      await expect(pending).resolves.toBe('opened_settings');
      expect(handler).toHaveBeenCalledWith(
        expect.objectContaining({ detail: { service: 'RemoteStt' } }),
      );
    });

    it('Escape key resolves with cancel', async () => {
      render(EndpointOfflineDialog);
      const pending = endpointOfflineStore.openAndWait(payload());
      await screen.findByText("Office server isn't responding");
      await fireEvent.keyDown(document.body.querySelector('.backdrop')!, { key: 'Escape' });
      await expect(pending).resolves.toBe('cancel');
    });

    it('backdrop click resolves with cancel', async () => {
      const { container } = render(EndpointOfflineDialog);
      const pending = endpointOfflineStore.openAndWait(payload());
      const backdrop = await screen.findByRole('presentation');
      await fireEvent.click(backdrop);
      await expect(pending).resolves.toBe('cancel');
    });
  });
  ```

  Add `import { vi } from 'vitest';` at the top if it isn't auto-imported by the test config.

- [ ] **Step 4: Run the tests**

  Run: `npx vitest run src/lib/components/EndpointOfflineDialog.test.ts`

  Expected: 11 tests pass (1 empty-render + 1 title + 4 reasons + 3 buttons + 2 dismissal).

  If `@testing-library/svelte` isn't installed, run `npm install --save-dev @testing-library/svelte` and add it to `vitest.config.ts` if any global setup is required (consult an existing component test in the codebase for the exact setup; `ExportDialog.test.ts` is a good reference if it exists).

- [ ] **Step 5: Commit**

  ```bash
  git add src/lib/components/EndpointOfflineDialog.svelte src/lib/components/EndpointOfflineDialog.test.ts
  git commit -m "feat(frontend): EndpointOfflineDialog component with a11y

Modal dialog that subscribes to endpointOfflineStore and dispatches
user decisions back via _resolve. Per-reason copy (ConnectionRefused
/ Timeout / DnsFailure / TlsFailure). Mirrors ExportDialog a11y:
focus trap, Escape closes, backdrop click closes, primary-action
default focus."
  ```

---

## Task 11: Frontend — `invokeWithOfflineHandling` helper

**Files:**
- Modify: `src/lib/api/invokeWithOfflineHandling.ts` (add the helper implementation)
- Create: `src/lib/api/invokeWithOfflineHandling.test.ts`

**Why:** The helper closes the loop: it catches `EndpointOffline` rejections from `invoke()`, opens the dialog via the store, awaits the user's decision, and on Retry loops back to re-invoke the original command. On success the loop exits and the caller's `await` resumes — that's how Section 4 of the spec's "click Retry → recording proceeds" promise is delivered.

- [ ] **Step 1: Write the failing helper tests**

  Create `src/lib/api/invokeWithOfflineHandling.test.ts`:

  ```ts
  import { describe, it, expect, beforeEach, vi } from 'vitest';
  import {
    invokeWithOfflineHandling,
    OfflineCancelled,
    type EndpointOfflinePayload,
  } from './invokeWithOfflineHandling';
  import { endpointOfflineStore } from '$lib/stores/endpointOffline';

  // Mock the Tauri invoke. The path depends on the codebase — read an
  // existing test that mocks invoke and copy the import path.
  vi.mock('@tauri-apps/api/core', () => ({
    invoke: vi.fn(),
  }));
  import { invoke } from '@tauri-apps/api/core';
  const mockInvoke = invoke as unknown as ReturnType<typeof vi.fn>;

  function offlinePayload(): EndpointOfflinePayload {
    return {
      kind: 'EndpointOffline',
      service: 'AiProvider',
      endpoint: 'http://x:1',
      reason: 'ConnectionRefused',
      provider_name: 'Ollama',
      message: 'mock',
    };
  }

  describe('invokeWithOfflineHandling', () => {
    beforeEach(() => {
      mockInvoke.mockReset();
      endpointOfflineStore.close();
    });

    it('resolves normally on first-attempt success', async () => {
      mockInvoke.mockResolvedValueOnce('the result');
      await expect(
        invokeWithOfflineHandling<string>('do_thing', { x: 1 }),
      ).resolves.toBe('the result');
      expect(mockInvoke).toHaveBeenCalledTimes(1);
    });

    it('passes through non-offline errors unchanged', async () => {
      const err = { kind: 'AiProvider', message: 'rate limit' };
      mockInvoke.mockRejectedValueOnce(err);
      await expect(invokeWithOfflineHandling('do_thing', {})).rejects.toBe(err);
    });

    it('opens dialog on EndpointOffline and resolves with retry result on success', async () => {
      mockInvoke
        .mockRejectedValueOnce(offlinePayload())
        .mockResolvedValueOnce('after retry');

      const pending = invokeWithOfflineHandling<string>('do_thing', { y: 2 });
      // Wait a microtask so the helper has time to open the dialog.
      await new Promise((r) => setTimeout(r, 0));
      endpointOfflineStore._resolve('retry');

      await expect(pending).resolves.toBe('after retry');
      expect(mockInvoke).toHaveBeenCalledTimes(2);
    });

    it('re-opens dialog if retry fails again', async () => {
      mockInvoke
        .mockRejectedValueOnce(offlinePayload())
        .mockRejectedValueOnce(offlinePayload());

      const pending = invokeWithOfflineHandling<string>('do_thing', {});
      await new Promise((r) => setTimeout(r, 0));
      endpointOfflineStore._resolve('retry');
      await new Promise((r) => setTimeout(r, 0));
      endpointOfflineStore._resolve('cancel');

      await expect(pending).rejects.toBeInstanceOf(OfflineCancelled);
      expect(mockInvoke).toHaveBeenCalledTimes(2);
    });

    it('throws OfflineCancelled on cancel', async () => {
      mockInvoke.mockRejectedValueOnce(offlinePayload());
      const pending = invokeWithOfflineHandling('do_thing', {});
      await new Promise((r) => setTimeout(r, 0));
      endpointOfflineStore._resolve('cancel');
      const err = await pending.catch((e) => e);
      expect(err).toBeInstanceOf(OfflineCancelled);
      expect((err as OfflineCancelled).reason).toBe('cancel');
    });

    it('throws OfflineCancelled on opened_settings', async () => {
      mockInvoke.mockRejectedValueOnce(offlinePayload());
      const pending = invokeWithOfflineHandling('do_thing', {});
      await new Promise((r) => setTimeout(r, 0));
      endpointOfflineStore._resolve('opened_settings');
      const err = await pending.catch((e) => e);
      expect(err).toBeInstanceOf(OfflineCancelled);
      expect((err as OfflineCancelled).reason).toBe('opened_settings');
    });
  });
  ```

- [ ] **Step 2: Run the tests to verify they fail**

  Run: `npx vitest run src/lib/api/invokeWithOfflineHandling.test.ts`

  Expected: all 6 tests fail (the helper function isn't implemented yet).

- [ ] **Step 3: Implement the helper**

  Open `src/lib/api/invokeWithOfflineHandling.ts` (created in Task 9). Append:

  ```ts
  import { invoke } from '@tauri-apps/api/core';
  import { endpointOfflineStore } from '$lib/stores/endpointOffline';

  /** Wraps Tauri `invoke`. On `EndpointOffline` rejection, opens the
   *  shared dialog and awaits the user's decision:
   *    - Retry      → loops back to re-invoke `cmd` with `args`.
   *    - Cancel     → throws OfflineCancelled('cancel').
   *    - OpenSettings → throws OfflineCancelled('opened_settings').
   *  Any other rejection passes through verbatim.
   *
   *  Successful retry resumes the original `await` with the new result —
   *  callers don't need to re-trigger their action.
   */
  export async function invokeWithOfflineHandling<T>(
    cmd: string,
    args: Record<string, unknown>,
  ): Promise<T> {
    for (;;) {
      try {
        return await invoke<T>(cmd, args);
      } catch (err) {
        if (!isEndpointOffline(err)) {
          throw err;
        }
        const decision = await endpointOfflineStore.openAndWait(err);
        if (decision === 'retry') {
          continue;
        }
        throw new OfflineCancelled(decision);
      }
    }
  }
  ```

- [ ] **Step 4: Run the tests**

  Run: `npx vitest run src/lib/api/invokeWithOfflineHandling.test.ts`

  Expected: 6 tests pass.

- [ ] **Step 5: Commit**

  ```bash
  git add src/lib/api/invokeWithOfflineHandling.ts src/lib/api/invokeWithOfflineHandling.test.ts
  git commit -m "feat(frontend): invokeWithOfflineHandling with retry-loop semantics

The helper wraps Tauri invoke. On EndpointOffline rejections it opens
the dialog via the store and awaits the user's decision; Retry loops,
Cancel / Open Settings throw OfflineCancelled. Successful retry
resumes the original await — caller doesn't re-trigger the action."
  ```

---

## Task 12: Frontend — mount the dialog, wire navigation, migrate call sites

**Files:**
- Modify: `src/App.svelte` (or the root layout — whichever Svelte file owns the app shell)
- Modify: `src/lib/stores/pipeline.ts:163-181`
- Modify: any other invoke call sites for `generate_soap`, `generate_referral`, `generate_letter`, `generate_synopsis`, `chat`, `transcribe`

**Why:** Until the dialog is mounted at the app root, all the work in Tasks 9–11 is invisible. Once mounted plus call sites migrated, the user actually sees the new UX.

- [ ] **Step 1: Identify the root component**

  Run: `grep -ln '<EditView\|<Recording\|<Sidebar\|<App\|<Layout' src/App.svelte src/routes/+layout.svelte 2>/dev/null || ls src/`

  The root mount point is whichever component is rendered once for the whole app (likely `src/App.svelte`). Verify by reading the first 50 lines.

- [ ] **Step 2: Mount the dialog and handle the openSettings event**

  In the root component's `<script>` block, import the dialog and add a handler:

  ```svelte
  <script lang="ts">
    import EndpointOfflineDialog from '$lib/components/EndpointOfflineDialog.svelte';
    // …existing imports…

    function onEndpointOfflineOpenSettings(
      e: CustomEvent<{ service: 'AiProvider' | 'RemoteStt' }>,
    ) {
      // Navigate to the matching Settings pane. The exact navigation
      // mechanism depends on the existing settings router — read
      // src/lib/components/Settings*.svelte or the store that drives
      // pane selection, and replicate the pattern.
      if (e.detail.service === 'AiProvider') {
        // settingsStore.navigateTo('models');
      } else {
        // settingsStore.navigateTo('audio');
      }
    }
  </script>

  <!-- …existing markup… -->

  <EndpointOfflineDialog on:openSettings={onEndpointOfflineOpenSettings} />
  ```

  **Implementor:** read the existing Settings navigation (likely a `settingsStore` or query-param mechanism) and replace the two commented lines with the real call. If no settings router exists, opening Settings → Models means showing the Settings overlay and selecting the Models tab — find how the user does that today and replicate it programmatically.

- [ ] **Step 3: Migrate `pipeline.ts:163-181`**

  In `src/lib/stores/pipeline.ts`, the existing code at line 163 is:

  ```ts
  processRecording(recordingId, context, template, patientContext).catch((err) => {
    const message = formatError(err);
    log.error('Pipeline command failed', { recordingId, error: message });
    update((s) => {
      // …error-state update…
    });
  });
  ```

  Note: `processRecording` is presumably itself an `invoke('process_recording', …)` wrapper. The migration depends on whether the pipeline is *one* invoke or several. Two strategies:

  - **If `processRecording` is one invoke:** replace its body with `invokeWithOfflineHandling('process_recording', { … })` and update the catch to filter OfflineCancelled:

    ```ts
    import { invokeWithOfflineHandling, OfflineCancelled } from '$lib/api/invokeWithOfflineHandling';

    invokeWithOfflineHandling('process_recording', { recordingId, context, template, patientContext })
      .then(() => { /* …existing success-path UI… */ })
      .catch((err) => {
        if (err instanceof OfflineCancelled) {
          // Restore the pre-pipeline UI state (clear the "transcribing" entry).
          update((s) => ({ ...s, current: null, active: { ...s.active, [recordingId]: undefined } }));
          return;
        }
        const message = formatError(err);
        log.error('Pipeline command failed', { recordingId, error: message });
        // …existing error-state update…
      });
    ```

  - **If `processRecording` orchestrates multiple invokes:** read the implementation and migrate each individual invoke call site that hits the backend. This is the more thorough migration and yields better UX (each step in the pipeline can independently trigger the offline dialog).

  Pick the strategy that matches the actual code shape. Either way, the OfflineCancelled filter is the migration's centrepiece.

- [ ] **Step 4: Migrate any direct invoke call sites for generation commands**

  Run: `grep -rn "invoke<.*>(\\s*['\"]generate_soap\\|invoke(['\"]generate_soap\\|invoke<.*>(\\s*['\"]generate_referral\\|invoke<.*>(\\s*['\"]generate_letter\\|invoke<.*>(\\s*['\"]generate_synopsis\\|invoke<.*>(\\s*['\"]chat\\|invoke<.*>(\\s*['\"]transcribe" src/`

  For each match, replace `invoke` with `invokeWithOfflineHandling` and update the surrounding catch to filter `OfflineCancelled`. (If the call is inside a thin API wrapper file like `src/lib/api/recordings.ts`, the migration is one-per-file rather than one-per-component.)

- [ ] **Step 5: Build and verify the frontend compiles**

  Run: `npm run check`

  Expected: `svelte-check` passes with no new errors.

- [ ] **Step 6: Run all frontend tests**

  Run: `npx vitest run`

  Expected: all tests pass — including the existing ones whose call sites you migrated.

- [ ] **Step 7: Run all backend tests one more time**

  Run: `cargo test --workspace --lib`

  Expected: green across the workspace.

- [ ] **Step 8: Commit**

  ```bash
  git add src/
  git commit -m "feat(frontend): mount EndpointOfflineDialog at app root and migrate call sites

App-root mount with navigation handler for Open Settings. Migrated
pipeline.ts and direct generation/transcribe invoke call sites to
invokeWithOfflineHandling. OfflineCancelled is filtered out of error
toasts since the dialog has already informed the user."
  ```

---

## Task 13: Manual QA and final version bump

**Files:**
- Modify: `src-tauri/Cargo.toml` (version bump)
- Modify: `package.json` (version bump)
- Modify: `src-tauri/tauri.conf.json` (version bump)

**Why:** Per CLAUDE.md, versions are synchronised across these three files. The QA pass exercises every dialog state on a real Windows-client / Mac-server pair.

- [ ] **Step 1: Run the full manual QA checklist from the spec**

  Reference: spec lines under "Manual QA checklist". Execute each step on a Windows machine + Mac server pair. Record results.

  1. Stop Ollama on the Mac. From Windows, click **Generate SOAP** on an existing recording. **Expected:** dialog appears within ~3s, title "Office server isn't responding," body names "Ollama" and the endpoint, three buttons, recording-saved reassurance.
  2. Click **Retry** without restarting Ollama → dialog re-opens identically.
  3. Restart Ollama → click **Retry** → dialog closes, SOAP generation proceeds, finished SOAP appears.
  4. Click **Open Settings** → app navigates to Settings → Models.
  5. Stop the Whisper STT server. Click **Transcribe** → dialog appears naming "Whisper STT" and the STT endpoint.
  6. Click **Open Settings** from the STT dialog → app navigates to Settings → Audio.
  7. Switch to local Ollama (host = `localhost`). Stop local Ollama. Click **Generate SOAP** → dialog appears (no 3s pre-flight delay; failure surfaces via the call-site mapper).
  8. With a remote endpoint pointing at `http://192.0.2.1:11434` (TEST-NET-1): click **Generate SOAP** → dialog with `Timeout` copy after 3s.
  9. With `http://nonexistent.invalid:11434`: click **Generate SOAP** → dialog with `DnsFailure` (or `ConnectionRefused`, platform-dependent) copy.

- [ ] **Step 2: Verify the old strings are gone in user-visible surfaces**

  Run:

  ```bash
  grep -rn "Connection refused — is Ollama running\|Connection refused — is LM Studio running\|Cannot reach Whisper server\|Office server unreachable on LAN or Tailscale" src/ crates/
  ```

  Expected: only test files and the `providers.rs` `test_*_connection` strings (those are user-clicked Settings → Test Connection, not pipeline failures — kept per spec). No matches inside provider runtime paths or in `pipeline.ts`'s toast path.

- [ ] **Step 3: Bump the version**

  Look up the current version with `grep '^version' src-tauri/Cargo.toml`. Increment patch (e.g. `0.10.56` → `0.10.57`). Apply the same version to:
  - `src-tauri/Cargo.toml` `version = "X.Y.Z"`
  - `package.json` `"version": "X.Y.Z"`
  - `src-tauri/tauri.conf.json` `"version": "X.Y.Z"`

- [ ] **Step 4: Final test sweep**

  Run in parallel:

  ```bash
  cargo test --workspace --lib
  ```

  ```bash
  npx vitest run
  ```

  ```bash
  npm run check
  ```

  Expected: all three green.

- [ ] **Step 5: Commit and tag**

  ```bash
  git add src-tauri/Cargo.toml package.json src-tauri/tauri.conf.json
  git commit -m "chore: bump X.Y.Z — server-down error messages (Phase 1)

Pre-flight check at processing time + structured EndpointOffline
error + plain-language dialog (Retry / Open Settings / Cancel).
Phase 2 (ambient status pill, record-time banner) deferred."
  ```

  ```bash
  git tag vX.Y.Z
  ```

  The release workflow (`release.yml`) builds installers on tag push.

---

## Self-Review

**Spec coverage:**
- Acceptance criterion 1 (new variant + serialization tests) → Task 1 ✓
- Criterion 2 (preflight module + reason classifications + skip rule) → Tasks 2–3 ✓
- Criterion 3 (every command invokes pre-flight) → Tasks 7–8 ✓
- Criterion 4 (three call-site mappers rewritten; old strings gone) → Tasks 5–6 + Task 13 Step 2 ✓
- Criterion 5 (dialog component with all required behavior) → Task 10 ✓
- Criterion 6 (store + helper + OfflineCancelled; call-site migration) → Tasks 9, 11, 12 ✓
- Criterion 7 (manual QA passes) → Task 13 ✓
- Criterion 8 (no PHI in logs) → preflight.rs logs only host:port + elapsed; reviewed in Task 2 ✓
- Criterion 9 (workspace tests green) → Task 13 Step 4 ✓
- Criterion 10 (npm run check green) → Task 13 Step 4 ✓

**Type consistency:**
- `ServiceKind` / `OfflineReason` defined in Task 1, referenced in Tasks 2–8, 10–11 — names match throughout.
- `CommandKind` defined in Task 3, referenced in Tasks 7–8 — match.
- `EndpointOfflinePayload` / `OfflineCancelled` / `isEndpointOffline` / `EndpointOfflineDecision` defined in Task 9, used in Tasks 10–12 — match.
- `endpointOfflineStore.openAndWait` / `_resolve` / `close` defined in Task 9, called in Tasks 10–11 — match.

**Known under-specifications** (acceptable to leave for the implementor, with notes flagged in-line):
- Task 5 Step 1: `RemoteWhisperProvider::new` constructor shape — note in the step tells the implementor to read the existing tests in the same file.
- Task 6 Step 2: `ep.lan_url()` accessor — note tells the implementor to add it if missing.
- Task 6 Step 4: shared HTTP client location — Step 1 of the task is explicitly a "locate" step before changing code.
- Task 12 Step 2: settings-router navigation mechanism — comment in the code block tells the implementor to inspect existing nav and replicate.

These are intentional: the spec is the source of truth for *what*; the plan tells the implementor *where*; the implementor reads the file to find the *exact line*. The notes are flagged with words like "Implementor:" and "If … doesn't exist" so they're greppable.

---

Plan complete and saved to `docs/superpowers/plans/2026-05-12-server-down-error-messages-phase1.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Aligns with the project's existing convention from CLAUDE.md ("prefer subagent-driven development with TDD per task").

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
