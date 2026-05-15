# Sharing Crate Tier 3 Test Backfill — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add 21 unit/integration tests to `crates/sharing` covering pairing HTTP routes, SharingService lifecycle, and additional xml_escape edges. Brings medical-sharing from 47 → 68 tests. One small `pub(crate)` extraction.

**Architecture:** Tower-based axum testing via `ServiceExt::oneshot` with synthetic `ConnectInfo` for deterministic loopback-vs-non-loopback paths. Real `TokenStore`/`PairingState` over `tempfile::tempdir()` for state. No subprocesses, no real mDNS, no real Ollama.

**Tech Stack:** Rust 1.x · axum 0.7 · tokio · tower 0.5 (new dev-dep) · tempfile · `tower::ServiceExt::oneshot`

**Spec:** `docs/superpowers/specs/2026-05-15-sharing-tier3-test-design.md`

**Worktree:** `.worktrees/sharing-tier3-tests` on branch `sharing-tier3-tests` (created from `master` at `1443c5b`)

**Baseline:** `cargo test -p medical-sharing --lib` → 47 passed

---

## Task 1: Add tower dev-dep

**Files:**
- Modify: `crates/sharing/Cargo.toml` (`[dev-dependencies]` section, after the `wiremock = { workspace = true }` line)

- [ ] **Step 1: Add tower to dev-dependencies**

Edit `crates/sharing/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = { workspace = true }
tokio = { workspace = true }
wiremock = { workspace = true }
tower = { version = "0.5", features = ["util"] }
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build -p medical-sharing --tests`
Expected: success, no warnings; Cargo.lock updated with `tower = "0.5.x"` and its transitive `tower-layer`, `tower-service`, etc.

- [ ] **Step 3: Run existing tests**

Run: `cargo test -p medical-sharing --lib`
Expected: `47 passed; 0 failed`

- [ ] **Step 4: Commit**

```bash
git add crates/sharing/Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
chore(sharing): add tower (util) dev-dep for axum router tests

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Refactor — extract build_pairing_router

**Files:**
- Modify: `crates/sharing/src/orchestrator.rs` (function `spawn_pairing_service`, lines 322–413)

- [ ] **Step 1: Extract Router construction into pub(crate) fn**

Replace `spawn_pairing_service` with a new `build_pairing_router` plus a thinned-out `spawn_pairing_service`. Move the entire `St` struct, the four inner async fns (`enroll`, `list_clients`, `revoke`, `info_handler`), and the `Router::new()...with_state(st)` construction into `build_pairing_router`. The serializable structs (`EnrollReq`, `EnrollResp`, `ClientView`) stay nested inside `build_pairing_router` since they're only referenced by its handlers.

Target shape (intent — keep the existing handler bodies unchanged):

```rust
pub(crate) fn build_pairing_router(
    pairing: Arc<PairingState>,
    store: Arc<TokenStore>,
    info: InfoSnapshot,
) -> axum::Router {
    use axum::{Json, Router, extract::{ConnectInfo, State}, routing::{get, post}};
    use serde::{Deserialize, Serialize};
    use std::net::SocketAddr;

    #[derive(Clone)]
    struct St { pairing: Arc<PairingState>, store: Arc<TokenStore>, info: InfoSnapshot }

    #[derive(Deserialize)]
    struct EnrollReq { code: String, label: String }
    #[derive(Serialize)]
    struct EnrollResp { token: String }

    async fn enroll(
        State(st): State<St>,
        Json(req): Json<EnrollReq>,
    ) -> Result<Json<EnrollResp>, axum::http::StatusCode> {
        let token = st
            .pairing
            .enroll(&req.code, &req.label)
            .await
            .map_err(|_| axum::http::StatusCode::UNAUTHORIZED)?;
        Ok(Json(EnrollResp { token }))
    }

    #[derive(Serialize)]
    struct ClientView { id: i64, label: String }

    async fn list_clients(
        State(st): State<St>,
        ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ) -> Result<Json<Vec<ClientView>>, axum::http::StatusCode> {
        if !addr.ip().is_loopback() {
            return Err(axum::http::StatusCode::FORBIDDEN);
        }
        let v = st
            .store
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|r| ClientView { id: r.id, label: r.label })
            .collect();
        Ok(Json(v))
    }

    async fn revoke(
        State(st): State<St>,
        ConnectInfo(addr): ConnectInfo<SocketAddr>,
        axum::extract::Path(id): axum::extract::Path<i64>,
    ) -> axum::http::StatusCode {
        if !addr.ip().is_loopback() {
            return axum::http::StatusCode::FORBIDDEN;
        }
        match st.store.revoke(id) {
            Ok(_) => axum::http::StatusCode::NO_CONTENT,
            Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    async fn info_handler(State(st): State<St>) -> Json<InfoSnapshot> {
        Json(st.info.clone())
    }

    let st = St { pairing, store, info };
    Router::new()
        .route("/pair/enroll", post(enroll))
        .route("/pair/clients", get(list_clients))
        .route("/pair/revoke/:id", post(revoke))
        .route("/info", get(info_handler))
        .with_state(st)
}

async fn spawn_pairing_service(
    port: u16,
    pairing: Arc<PairingState>,
    store: Arc<TokenStore>,
    info: InfoSnapshot,
) -> crate::Result<tokio::task::JoinHandle<()>> {
    use std::net::SocketAddr;

    let app = build_pairing_router(pairing, store, info);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| crate::SharingError::Pairing(format!("bind 0.0.0.0:{port}: {e}")))?;

    Ok(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        ).await;
    }))
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p medical-sharing`
Expected: success, no warnings about unused imports.

- [ ] **Step 3: Run existing tests**

Run: `cargo test -p medical-sharing --lib`
Expected: `47 passed; 0 failed` (no behavior change)

- [ ] **Step 4: Commit**

```bash
git add crates/sharing/src/orchestrator.rs
git commit -m "$(cat <<'EOF'
refactor(orchestrator): extract build_pairing_router helper

Pulls the axum Router construction out of spawn_pairing_service so the
pairing HTTP routes can be tested via tower::ServiceExt::oneshot with
synthetic ConnectInfo. spawn_pairing_service still binds the listener
and wraps bind failures in SharingError::Pairing; observable behavior
preserved.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Pairing handler tests (10)

**Files:**
- Modify: `crates/sharing/src/orchestrator.rs` (append `#[cfg(test)] mod pairing_router_tests` at end of file)

- [ ] **Step 1: Write the failing test module**

Append to `crates/sharing/src/orchestrator.rs`:

```rust
#[cfg(test)]
mod pairing_router_tests {
    use super::*;
    use crate::mdns::ServerPorts;
    use crate::pairing::PairingState;
    use crate::token_store::TokenStore;
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::{Method, Request, StatusCode};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tower::ServiceExt;

    fn fresh_store_and_pairing() -> (tempfile::TempDir, Arc<TokenStore>, Arc<PairingState>) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.db");
        let key = [0u8; 32];
        let store = Arc::new(TokenStore::open(&path, &key).expect("open store"));
        let pairing = Arc::new(PairingState::new(store.clone()));
        (dir, store, pairing)
    }

    fn sample_info() -> InfoSnapshot {
        InfoSnapshot {
            host: "test-host".into(),
            version: "9.9.9".into(),
            ports: ServerPorts {
                ollama: Some(11435),
                whisper: Some(8081),
                lmstudio: None,
                pairing: Some(11436),
                vocab: Some(11437),
            },
        }
    }

    fn loopback_connect_info() -> ConnectInfo<SocketAddr> {
        ConnectInfo("127.0.0.1:50000".parse().unwrap())
    }

    fn lan_connect_info() -> ConnectInfo<SocketAddr> {
        ConnectInfo("192.168.1.50:50000".parse().unwrap())
    }

    fn json_body<T: serde::Serialize>(v: &T) -> Body {
        Body::from(serde_json::to_vec(v).unwrap())
    }

    #[tokio::test]
    async fn enroll_succeeds_with_valid_code() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let app = build_pairing_router(pairing.clone(), store.clone(), sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri("/pair/enroll")
            .header("content-type", "application/json")
            .body(json_body(&serde_json::json!({ "code": code, "label": "iPad" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed["token"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[tokio::test]
    async fn enroll_returns_401_on_invalid_code() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let app = build_pairing_router(pairing, store.clone(), sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri("/pair/enroll")
            .header("content-type", "application/json")
            .body(json_body(&serde_json::json!({ "code": "000000", "label": "iPad" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(store.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn enroll_persists_token_in_store() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let app = build_pairing_router(pairing.clone(), store.clone(), sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri("/pair/enroll")
            .header("content-type", "application/json")
            .body(json_body(&serde_json::json!({ "code": code, "label": "phone-1" })))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "phone-1");
    }

    #[tokio::test]
    async fn list_clients_from_loopback_returns_paired_clients() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let _ = pairing.enroll(&code, "loopback-client").await.unwrap();
        let app = build_pairing_router(pairing, store, sample_info());

        let req = Request::builder()
            .method(Method::GET)
            .uri("/pair/clients")
            .extension(loopback_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["label"], "loopback-client");
    }

    #[tokio::test]
    async fn list_clients_from_non_loopback_returns_403() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let _ = pairing.enroll(&code, "client").await.unwrap();
        let app = build_pairing_router(pairing, store, sample_info());

        let req = Request::builder()
            .method(Method::GET)
            .uri("/pair/clients")
            .extension(lan_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn revoke_from_loopback_removes_token() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let _ = pairing.enroll(&code, "to-revoke").await.unwrap();
        let id = store.list().unwrap()[0].id;
        let app = build_pairing_router(pairing, store.clone(), sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("/pair/revoke/{id}"))
            .extension(loopback_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(store.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn revoke_from_non_loopback_returns_403() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let code = pairing.issue_code().await;
        let _ = pairing.enroll(&code, "to-keep").await.unwrap();
        let id = store.list().unwrap()[0].id;
        let app = build_pairing_router(pairing, store.clone(), sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("/pair/revoke/{id}"))
            .extension(lan_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn revoke_returns_204_even_for_unknown_id() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let app = build_pairing_router(pairing, store, sample_info());

        let req = Request::builder()
            .method(Method::POST)
            .uri("/pair/revoke/99999")
            .extension(loopback_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn info_returns_snapshot_with_configured_ports() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let app = build_pairing_router(pairing, store, sample_info());

        let req = Request::builder()
            .method(Method::GET)
            .uri("/info")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["host"], "test-host");
        assert_eq!(parsed["version"], "9.9.9");
        assert_eq!(parsed["ports"]["ollama"], 11435);
        assert_eq!(parsed["ports"]["pairing"], 11436);
        assert_eq!(parsed["ports"]["vocab"], 11437);
        assert!(parsed["ports"]["lmstudio"].is_null());
    }

    #[tokio::test]
    async fn info_requires_no_auth_or_loopback() {
        let (_dir, store, pairing) = fresh_store_and_pairing();
        let app = build_pairing_router(pairing, store, sample_info());

        let req = Request::builder()
            .method(Method::GET)
            .uri("/info")
            .extension(lan_connect_info())
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
```

- [ ] **Step 2: Run the new tests**

Run: `cargo test -p medical-sharing --lib pairing_router_tests`
Expected: `10 passed; 0 failed`

- [ ] **Step 3: Run the full sharing suite**

Run: `cargo test -p medical-sharing --lib`
Expected: `57 passed; 0 failed` (47 baseline + 10 new)

- [ ] **Step 4: Commit**

```bash
git add crates/sharing/src/orchestrator.rs
git commit -m "$(cat <<'EOF'
test(orchestrator): add pairing HTTP router tests via tower::oneshot

10 axum router tests via tower::ServiceExt::oneshot covering all 4
pairing routes plus loopback enforcement on admin routes. Synthetic
ConnectInfo lets us exercise both 127.0.0.1 (accept) and 192.168.x.x
(403) branches deterministically.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: SharingService lifecycle tests (9)

**Files:**
- Modify: `crates/sharing/src/orchestrator.rs` (append `#[cfg(test)] mod lifecycle_tests` after `pairing_router_tests`)

- [ ] **Step 1: Write the failing test module**

Append to `crates/sharing/src/orchestrator.rs`:

```rust
#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn cfg_with_tokens_at(path: PathBuf, key: [u8; 32], api_key: &str) -> SharingConfig {
        SharingConfig {
            enabled: true,
            friendly_name: "test-server".into(),
            ollama_proxy_port: 11435,
            whisper_proxy_port: 8081,
            pairing_port: 11436,
            whisper_internal_port: 8080,
            lmstudio_internal_port: None,
            lmstudio_proxy_port: None,
            vocab_port: 11437,
            token_store_path: path,
            token_store_key: key,
            binary_dir: PathBuf::from("/tmp"),
            whisper_model_path: PathBuf::from("/tmp/model.bin"),
            whisper_internal_api_key: api_key.to_string(),
            version: "9.9.9".into(),
        }
    }

    #[test]
    fn sharing_config_default_has_expected_ports() {
        let c = SharingConfig::default();
        assert_eq!(c.ollama_proxy_port, 11435);
        assert_eq!(c.whisper_proxy_port, 8081);
        assert_eq!(c.pairing_port, 11436);
        assert_eq!(c.whisper_internal_port, 8080);
        assert_eq!(c.vocab_port, 11437);
    }

    #[test]
    fn sharing_config_default_is_disabled() {
        let c = SharingConfig::default();
        assert!(!c.enabled);
        assert!(c.lmstudio_internal_port.is_none());
        assert!(c.lmstudio_proxy_port.is_none());
    }

    #[test]
    fn sharing_config_debug_redacts_token_store_key() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7).wrapping_add(3);
        }
        let c = cfg_with_tokens_at(PathBuf::from("/tmp/x"), key, "irrelevant");
        let dbg = format!("{:?}", c);
        assert!(dbg.contains("<redacted: 32 bytes>"), "Debug must redact key marker; got: {dbg}");
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert!(
            !dbg.to_lowercase().contains(&hex),
            "Debug must not contain key bytes as hex"
        );
    }

    #[test]
    fn sharing_config_debug_redacts_whisper_internal_api_key() {
        let api_key = "secret-key-DO-NOT-LEAK-12345";
        let c = cfg_with_tokens_at(PathBuf::from("/tmp/x"), [0u8; 32], api_key);
        let dbg = format!("{:?}", c);
        assert!(dbg.contains("<redacted>"), "Debug must contain redacted marker for api key; got: {dbg}");
        assert!(
            !dbg.contains(api_key),
            "Debug must not contain literal api key"
        );
    }

    #[test]
    fn sharing_service_new_creates_token_store_on_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.db");
        let c = cfg_with_tokens_at(path.clone(), [0u8; 32], "k");
        let _svc = SharingService::new(c).expect("new() should succeed");
        assert!(path.exists(), "token store db should be created on disk");
    }

    #[test]
    fn sharing_service_new_returns_token_store_error_on_unwritable_path() {
        // A path under /dev/null/... can't be created because /dev/null isn't a directory.
        let c = cfg_with_tokens_at(
            PathBuf::from("/dev/null/cannot-create/tokens.db"),
            [0u8; 32],
            "k",
        );
        let err = SharingService::new(c).expect_err("expected TokenStore error");
        assert!(
            matches!(err, SharingError::TokenStore(_)),
            "expected TokenStore variant, got {err:?}"
        );
    }

    #[tokio::test]
    async fn sharing_service_status_when_not_running_reports_disabled() {
        let dir = tempdir().unwrap();
        let c = cfg_with_tokens_at(dir.path().join("tokens.db"), [0u8; 32], "k");
        let svc = SharingService::new(c).unwrap();
        let s = svc.status().await;
        assert!(!s.enabled);
        assert!(!s.ollama_ok);
        assert!(!s.whisper_ok);
        assert!(!s.lmstudio_ok);
        assert!(!s.mdns_ok);
        assert!(!s.pairing_ok);
        assert_eq!(s.paired_clients, 0);
    }

    #[tokio::test]
    async fn sharing_service_status_counts_paired_clients_when_stopped() {
        let dir = tempdir().unwrap();
        let c = cfg_with_tokens_at(dir.path().join("tokens.db"), [0u8; 32], "k");
        let svc = SharingService::new(c).unwrap();
        let pairing = svc.pairing_state();
        let code = pairing.issue_code().await;
        let _token = pairing.enroll(&code, "client-a").await.unwrap();
        let s = svc.status().await;
        // paired_clients reflects store state even though service was never started
        assert_eq!(s.paired_clients, 1);
        assert!(!s.enabled);
    }

    #[tokio::test]
    async fn sharing_service_stop_is_idempotent_when_never_started() {
        let dir = tempdir().unwrap();
        let c = cfg_with_tokens_at(dir.path().join("tokens.db"), [0u8; 32], "k");
        let svc = SharingService::new(c).unwrap();
        svc.stop().await.expect("first stop should be Ok");
        svc.stop().await.expect("second stop should also be Ok");
    }
}
```

- [ ] **Step 2: Run the new tests**

Run: `cargo test -p medical-sharing --lib lifecycle_tests`
Expected: `9 passed; 0 failed`

- [ ] **Step 3: Run the full sharing suite**

Run: `cargo test -p medical-sharing --lib`
Expected: `66 passed; 0 failed` (47 + 10 + 9)

- [ ] **Step 4: Commit**

```bash
git add crates/sharing/src/orchestrator.rs
git commit -m "$(cat <<'EOF'
test(orchestrator): add SharingService lifecycle + Debug-redaction tests

9 tests covering SharingConfig::default port invariants, the
security-critical Debug redaction of token_store_key and
whisper_internal_api_key, SharingService::new happy + unwritable-path
paths, status() reporting when not running, paired_clients count
reflecting store state, and stop() idempotence.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: xml_escape edge tests (2)

**Files:**
- Modify: `crates/sharing/src/service_installer.rs` (append two tests to the existing `#[cfg(test)] mod tests` at lines 293–310)

- [ ] **Step 1: Append the two tests**

Inside the existing `mod tests` block in `crates/sharing/src/service_installer.rs`, after `xml_escape_empty_input_is_empty`:

```rust
    #[test]
    fn xml_escape_handles_ampersand_before_other_chars() {
        // Input literally contains "&lt;" — we must NOT see "&amp;amp;lt;"
        // (which would happen if we replaced < before & on this input).
        // The chain replaces & first, so "&lt;" becomes "&amp;lt;".
        assert_eq!(xml_escape("&lt;"), "&amp;lt;");
        assert_eq!(xml_escape("&&"), "&amp;&amp;");
    }

    #[test]
    fn xml_escape_handles_realistic_windows_path() {
        // Defends the Windows ScheduledTask install() path against
        // unescaped '&' injection in folder names.
        let input = r"C:\Program Files & Co\ollama.exe";
        let expected = r"C:\Program Files &amp; Co\ollama.exe";
        assert_eq!(xml_escape(input), expected);
    }
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p medical-sharing --lib service_installer::tests`
Expected: `4 passed; 0 failed` (2 existing + 2 new)

- [ ] **Step 3: Run the full sharing suite**

Run: `cargo test -p medical-sharing --lib`
Expected: `68 passed; 0 failed`

- [ ] **Step 4: Commit**

```bash
git add crates/sharing/src/service_installer.rs
git commit -m "$(cat <<'EOF'
test(service_installer): add xml_escape edge tests for ordering and Windows paths

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Final verification

- [ ] **Step 1: Run targeted suite**

Run: `cargo test -p medical-sharing --lib`
Expected: `68 passed; 0 failed`

- [ ] **Step 2: Run full workspace lib tests**

Run: `cargo test --workspace --lib 2>&1 | grep -E "^test result:"`
Expected: 14 lines, all "ok", none "FAILED"

- [ ] **Step 3: Verify no PHI in new code**

Run: `grep -rE "(patient|transcript|soap|medication|allergy|condition)" crates/sharing/src/ | grep -v "/// " | grep -v "// "`
Expected: no matches in test or production code beyond unrelated comments

- [ ] **Step 4: Verify clean git state**

Run: `git status`
Expected: clean (no untracked files left over from test runs)

- [ ] **Step 5: Confirm commit chain**

Run: `git log --oneline master..HEAD`
Expected: 5 commits — spec doc (already committed), tower dep, refactor, pairing tests, lifecycle tests, xml_escape tests. Plus this plan commit.

After all tasks: Dispatch final code reviewer subagent for entire implementation. Then use superpowers:finishing-a-development-branch.
