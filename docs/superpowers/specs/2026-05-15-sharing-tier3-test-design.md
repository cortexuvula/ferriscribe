# Sharing Crate Tier 3 Test Backfill — Design

**Date:** 2026-05-15
**Branch:** `sharing-tier3-tests`
**Predecessor:** Tier 2 (`sharing-tier2-tests`, merged at `1443c5b`)

## Goal

Close the remaining untested boundaries in `crates/sharing` that can be exercised without invasive trait-based refactoring of platform subprocess and filesystem effects. Bring `medical-sharing` from 47 to 68 unit/integration tests.

## Scope

Three test surfaces:

1. **Pairing HTTP handlers** in `orchestrator::spawn_pairing_service` — the axum service exposed on the pairing port. Routes: `POST /pair/enroll`, `GET /pair/clients`, `POST /pair/revoke/:id`, `GET /info`.
2. **`SharingService` + `SharingConfig` lifecycle** in `orchestrator.rs` — config defaults, security-critical `Debug` redaction, `new`/`stop`/`status` behavior with the heavyweight `start()` deliberately excluded.
3. **`service_installer::xml_escape` edge coverage** — additional cases on top of the 2 baseline tests.

## Non-goals

- `SharingService::start()` integration — needs real whisper.cpp binary, real Ollama, real mDNS multicast. Not testable without major mocking infrastructure.
- Platform `install()` writers (Launchd plist, Systemd unit, schtasks XML) — needs trait-based subprocess + filesystem abstractions ("Maximal" scope, deferred).
- `find_ollama_binary` — relies on real `which`/`where.exe` plus `PathBuf::exists` against real filesystem. Env-var mutation under parallel `cargo test` is unsafe.
- mDNS browse/advertise integration — needs real multicast.

## Architecture

### Refactor: extract `build_pairing_router`

`spawn_pairing_service` (lines 322–413 of `orchestrator.rs`) currently builds the axum `Router` inline, binds a TCP listener, and spawns the serve task. To test the routes deterministically (especially the loopback-only enforcement on `/pair/clients` and `/pair/revoke/:id`), extract the `Router` construction:

```rust
pub(crate) fn build_pairing_router(
    pairing: Arc<PairingState>,
    store: Arc<TokenStore>,
    info: InfoSnapshot,
) -> axum::Router {
    // existing inner-state struct, handlers, and Router::new()...with_state(st) chain
}
```

`spawn_pairing_service` then becomes:

```rust
async fn spawn_pairing_service(port, pairing, store, info) -> Result<JoinHandle<()>> {
    let app = build_pairing_router(pairing, store, info);
    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| SharingError::Pairing(format!("bind 0.0.0.0:{port}: {e}")))?;
    Ok(tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        ).await;
    }))
}
```

Observable behavior preserved: same routes, same handlers, same state, same bind, same error wrapping. The lock-on-bind invariant (bind errors surface as `SharingError::Pairing` before the spawn) is unchanged.

### Test mechanics: tower::ServiceExt::oneshot with synthetic ConnectInfo

The pairing handlers use `ConnectInfo<SocketAddr>` for loopback enforcement on the admin routes. To deterministically test both branches without binding to a non-loopback IP (impossible inside a sandboxed test), tests use `tower::ServiceExt::oneshot` and inject the `ConnectInfo` extension directly:

```rust
use tower::ServiceExt;

let req = Request::builder()
    .method(Method::POST)
    .uri("/pair/revoke/42")
    .extension(ConnectInfo::<SocketAddr>("192.168.1.50:33333".parse().unwrap()))
    .body(Body::empty())
    .unwrap();

let response = build_pairing_router(pairing, store, info)
    .oneshot(req)
    .await
    .unwrap();
assert_eq!(response.status(), StatusCode::FORBIDDEN);
```

This is the standard axum testing idiom and is faster than spawning a real listener.

### Dev-dependencies to add

In `crates/sharing/Cargo.toml`:

```toml
[dev-dependencies]
# existing entries kept
tower = { version = "0.5", features = ["util"] }
```

Already present: `wiremock`, `tempfile`, `tokio`. `axum::body::to_bytes` is re-exported by axum 0.7 — no `http-body-util` needed.

## Test inventory

### A. Pairing HTTP handlers (10 tests, in `orchestrator.rs` `#[cfg(test)] mod tests`)

| # | Test | Asserts |
|---|------|---------|
| 1 | `enroll_succeeds_with_valid_code` | PairingState issues code; POST /pair/enroll with that code returns 200 + non-empty token JSON |
| 2 | `enroll_returns_401_on_invalid_code` | POST with a random code returns 401, no token persisted |
| 3 | `enroll_persists_token_in_store` | After successful enroll, `store.list()` contains exactly one entry with the requested label |
| 4 | `list_clients_from_loopback_returns_paired_clients` | Pre-populate store via TokenStore directly; GET /pair/clients from `127.0.0.1:N` returns the labels |
| 5 | `list_clients_from_non_loopback_returns_403` | Same setup; GET /pair/clients with synthetic ConnectInfo `10.0.0.5:N` returns 403 |
| 6 | `revoke_from_loopback_removes_token` | Pre-populate, POST /pair/revoke/:id from loopback, store.list() returns empty |
| 7 | `revoke_from_non_loopback_returns_403` | Synthetic ConnectInfo on a public IP returns 403; store unchanged |
| 8 | `revoke_returns_204_even_for_unknown_id` | `TokenStore::revoke(99999)` returns Ok; handler returns 204 NO_CONTENT (documents current behavior) |
| 9 | `info_returns_snapshot_with_configured_ports` | GET /info returns InfoSnapshot JSON with the configured host/version/ports |
| 10 | `info_requires_no_auth_or_loopback` | GET /info from a non-loopback synthetic ConnectInfo still returns 200 (public-by-design) |

### B. SharingService + SharingConfig lifecycle (9 tests, in `orchestrator.rs` `#[cfg(test)] mod tests`)

| # | Test | Asserts |
|---|------|---------|
| 11 | `sharing_config_default_has_expected_ports` | `SharingConfig::default()`: ollama_proxy_port=11435, whisper_proxy_port=8081, pairing_port=11436, whisper_internal_port=8080, vocab_port=11437 |
| 12 | `sharing_config_default_is_disabled` | `enabled` is false; lmstudio_internal_port and lmstudio_proxy_port are None |
| 13 | `sharing_config_debug_redacts_token_store_key` | `format!("{:?}", config)` contains `"<redacted: 32 bytes>"` and does NOT contain any hex of the actual key bytes |
| 14 | `sharing_config_debug_redacts_whisper_internal_api_key` | `format!("{:?}", config)` contains `"<redacted>"` and does NOT contain the literal API key string |
| 15 | `sharing_service_new_creates_token_store_on_disk` | After `SharingService::new(cfg)`, the file at `cfg.token_store_path` exists |
| 16 | `sharing_service_new_returns_token_store_error_on_unwritable_path` | Pass a `token_store_path` whose parent directory does not exist and cannot be created (e.g., under `/dev/null/...`); expect `SharingError::TokenStore(_)` |
| 17 | `sharing_service_status_when_not_running_reports_disabled` | new(), then status() — all booleans false, paired_clients = 0 |
| 18 | `sharing_service_status_counts_paired_clients_when_stopped` | new(), pre-issue a code via `pairing_state().issue_code()` and enroll via `pairing_state().enroll(...)`; status().paired_clients reflects the count even though service not started |
| 19 | `sharing_service_stop_is_idempotent_when_never_started` | new(), call stop() — returns Ok; call stop() again — still Ok |

### C. service_installer::xml_escape edges (2 tests appended to existing `mod tests`)

| # | Test | Asserts |
|---|------|---------|
| 20 | `xml_escape_handles_ampersand_before_other_chars` | Input `"&lt;"` (literal text) is escaped to `"&amp;lt;"` (the `&` is replaced first, not double-encoded) — proves ordering of the `replace()` chain |
| 21 | `xml_escape_handles_realistic_windows_path` | Input `r"C:\Program Files & Co\ollama.exe"` produces `"C:\Program Files &amp; Co\ollama.exe"` — defends the Windows ScheduledTask install() path against unescaped `&` injection |

## Out-of-scope details

**Why no `find_ollama_binary` test:** the function runs `which ollama` (Unix) or `where.exe ollama` (Windows) and probes hard-coded absolute paths. Mocking would require either replacing `Command::new` (no native test seam in std), or fully wrapping in a `trait OllamaLocator`. Both are larger lifts than this Tier earns.

**Why no platform `install()` test:** each `install()` writes a real file under `$HOME/Library/LaunchAgents/`, `$XDG_CONFIG_HOME/systemd/user/`, or `$TEMP\ferriscribe-ollama.xml` and then invokes `launchctl`/`systemctl`/`schtasks`. Both effects need behind-trait abstractions to test in isolation. Belongs in a future "Maximal" Tier 4 spec if/when the user wants installer determinism guaranteed by tests.

## Local-only invariant

No new endpoints, no new outbound URLs. All test fixtures use `127.0.0.1` or synthetic in-process sockets. No PHI flows through the pairing service in production code (only labels, codes, and tokens — never transcripts/SOAP/medications), and tests preserve that boundary.

## Acceptance criteria

- `cargo test -p medical-sharing --lib` — 68 passed, 0 failed
- `cargo test --workspace --lib` — all green, no regressions
- Single `pub(crate)` refactor in `orchestrator.rs`, behavior of `spawn_pairing_service` unchanged
- 5–6 logical commits: dev-deps, refactor, pairing handler tests, lifecycle tests, xml_escape edges, Cargo.lock sync if needed
