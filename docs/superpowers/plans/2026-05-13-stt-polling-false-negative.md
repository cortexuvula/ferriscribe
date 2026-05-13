# STT Polling False-Negative Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the false-positive offline banner that appears for paired clients on v0.10.60. The Phase 2 polling probe sends `GET /v1/models` to the Whisper auth proxy, which forwards to Whisper.cpp, which returns 404 (it doesn't implement that path). The current `test_stt_remote_connection` Tauri command treats 404 as failure, so the poller marks STT offline even though actual transcription works. Phase 4 introduces a thin Tauri command that wraps Phase 1's existing `probe_endpoint` (which already treats any HTTP status as reachable) and switches the poller to use it.

**Architecture:** Add `probe_endpoint_reachable` Tauri command in `providers.rs` — a 5-line wrapper around the existing `medical_core::preflight::probe_endpoint`. Special-case 401/403 to still report offline (auth issues should surface in the pill). Switch `endpointHealth.ts::probeStt` and `probeAi` to call this command instead of `test_*_connection`. Leave the existing `test_*_connection` commands untouched — Settings → Test Connection buttons still need strict "can list models?" semantics.

**Tech Stack:** Rust (Tauri 2, reqwest), TypeScript (Svelte 5, Vitest + wiremock). Reuses Phase 1's `probe_endpoint`. No new dependencies.

**Spec:** [`docs/superpowers/specs/2026-05-13-stt-polling-false-negative-design.md`](../specs/2026-05-13-stt-polling-false-negative-design.md)

---

## File Structure

**Modified:**
- `src-tauri/src/commands/providers.rs` — add `probe_endpoint_reachable` Tauri command.
- `src-tauri/src/lib.rs` — register the new command in the Tauri builder.
- `src/lib/stores/endpointHealth.ts` — switch `probeStt` and `probeAi` to call `probe_endpoint_reachable`.
- `src/lib/stores/endpointHealth.test.ts` — update tests' mocked command names.

**No new files.** No backend test files for the new command — implementer adds tests in `providers.rs` alongside the function.

---

## Task 1: Add `probe_endpoint_reachable` Tauri command

**Files:**
- Modify: `src-tauri/src/commands/providers.rs`
- Modify: `src-tauri/src/lib.rs`

**Why:** Foundation. The new command wraps Phase 1's `probe_endpoint` and special-cases 401/403. Once this exists, Task 2 can switch the frontend to use it.

- [ ] **Step 1: Write the failing test**

Find the existing `#[cfg(test)] mod` block in `src-tauri/src/commands/providers.rs` (it has tests for the strict commands). Append:

```rust
    #[tokio::test]
    async fn probe_endpoint_reachable_returns_ok_on_any_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let parsed: url::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        // Call the inner reachability helper directly. Since the Tauri command
        // is a thin wrapper, test the underlying probe behavior through it.
        // If Tauri test harness is heavy, factor inner logic to a pure fn
        // taking (service, provider_name, host, port, probe_path, api_key) and
        // test that. The wrapper just adapts Tauri State.
        let result = probe_endpoint_reachable_inner(
            ServiceKind::RemoteStt,
            "Whisper STT".to_string(),
            host,
            port,
            "/v1/models".to_string(),
            None,
        ).await;

        assert!(result.is_ok(), "200 should be Ok; got {result:?}");
    }

    #[tokio::test]
    async fn probe_endpoint_reachable_returns_ok_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(404).set_body_string("File Not Found"))
            .mount(&server)
            .await;

        let parsed: url::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = probe_endpoint_reachable_inner(
            ServiceKind::RemoteStt,
            "Whisper STT".to_string(),
            host,
            port,
            "/v1/models".to_string(),
            None,
        ).await;

        assert!(
            result.is_ok(),
            "404 means 'server alive, route absent' — must be Ok for reachability; got {result:?}"
        );
    }

    #[tokio::test]
    async fn probe_endpoint_reachable_returns_endpoint_offline_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let parsed: url::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = probe_endpoint_reachable_inner(
            ServiceKind::RemoteStt,
            "Whisper STT".to_string(),
            host,
            port,
            "/v1/models".to_string(),
            None,
        ).await;

        let err = result.expect_err("401 must surface as Err so the pill reflects auth issues");
        assert!(
            matches!(err, AppError::EndpointOffline { .. }),
            "auth failure must produce EndpointOffline; got {err:?}"
        );
    }

    #[tokio::test]
    async fn probe_endpoint_reachable_forwards_bearer_when_provided() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer secret-token"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let parsed: url::Url = server.uri().parse().unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let port = parsed.port().unwrap();

        let result = probe_endpoint_reachable_inner(
            ServiceKind::RemoteStt,
            "Whisper STT".to_string(),
            host,
            port,
            "/v1/models".to_string(),
            Some("secret-token".to_string()),
        ).await;

        assert!(result.is_ok(), "authenticated 200 should be Ok; got {result:?}");
    }

    #[tokio::test]
    async fn probe_endpoint_reachable_returns_endpoint_offline_on_connect_refused() {
        // Bind+drop to get a guaranteed-refused port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let result = probe_endpoint_reachable_inner(
            ServiceKind::RemoteStt,
            "Whisper STT".to_string(),
            "127.0.0.1".to_string(),
            port,
            "/v1/models".to_string(),
            None,
        ).await;

        let err = result.expect_err("connect refused must error");
        assert!(matches!(err, AppError::EndpointOffline { .. }));
    }
```

(The tests call `probe_endpoint_reachable_inner` — a pure async fn that the Tauri command wraps. Define it next so tests are buildable without faking `tauri::State`.)

- [ ] **Step 2: Run the tests to verify they fail (function doesn't exist)**

```bash
cargo build -p rust-medical-assistant --tests 2>&1 | tail -10
```

Expected: build error — `probe_endpoint_reachable_inner` not defined.

- [ ] **Step 3: Implement the inner reachability fn + the Tauri command**

In `src-tauri/src/commands/providers.rs`, add (anywhere in the module above the existing strict commands):

```rust
use medical_core::error::ServiceKind;
use medical_core::preflight::probe_endpoint;

/// Inner reachability check — exposed as a pure async fn so unit tests can
/// call it without constructing `tauri::State`. The Tauri command is a thin
/// wrapper around this.
///
/// Returns Ok(()) for any HTTP response *except* 401/403 — auth failures
/// surface as EndpointOffline so the polling pill reflects them. Network
/// errors (connect/timeout/DNS/TLS) flow through probe_endpoint's existing
/// classification.
async fn probe_endpoint_reachable_inner(
    service: ServiceKind,
    provider_name: String,
    host: String,
    port: u16,
    probe_path: String,
    api_key: Option<String>,
) -> AppResult<()> {
    // probe_endpoint accepts any HTTP status. Run it first; if it errors
    // (network/timeout/etc), that error is what we want. If it succeeds,
    // run an extra check to detect 401/403 and surface those as EndpointOffline
    // — we need to send the request ourselves to inspect the status code,
    // because probe_endpoint discards the response body and status.

    let effective_host = if host.is_empty() { "localhost".to_string() } else { host };
    let base_url = format!("http://{}:{}", effective_host, port);

    // Quick network reachability via probe_endpoint (handles connect/timeout/DNS).
    probe_endpoint(
        service,
        &provider_name,
        &base_url,
        &probe_path,
        api_key.as_deref(),
    ).await?;

    // probe_endpoint succeeded (any HTTP response = reachable). Now do a
    // second request specifically to look for 401/403. The HTTP request
    // is cheap and reusing probe_endpoint's result would require changing
    // its API to return the status, which expands scope. Keep this command
    // self-contained.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| AppError::Config(format!("reachability client build: {e}")))?;
    let url = format!("{}/{}", base_url.trim_end_matches('/'), probe_path.trim_start_matches('/'));
    let mut req = client.get(&url);
    if let Some(key) = api_key.as_deref().filter(|s| !s.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let resp = req.send().await.map_err(|e| {
        AppError::EndpointOffline {
            service,
            endpoint: base_url.clone(),
            reason: medical_core::error::OfflineReason::ConnectionRefused,
            provider_name: provider_name.clone(),
        }
        // (network error here would've already been caught by probe_endpoint above,
        //  so this is just defensive — keep it minimal)
    })?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED
        || resp.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(AppError::EndpointOffline {
            service,
            endpoint: base_url,
            reason: medical_core::error::OfflineReason::ConnectionRefused,
            provider_name,
        });
    }

    // Any other status (200/404/5xx) is reachable.
    Ok(())
}

/// Lenient reachability probe for the background endpointHealth poller.
/// Wraps `probe_endpoint_reachable_inner` for Tauri exposure.
///
/// Returns Ok for any HTTP response except 401/403. Returns
/// `AppError::EndpointOffline` for network errors and auth failures.
///
/// Used by `src/lib/stores/endpointHealth.ts`. NOT used by Settings →
/// Test Connection buttons (those use the strict `test_*_connection` commands).
#[tauri::command]
pub async fn probe_endpoint_reachable(
    service: ServiceKind,
    provider_name: String,
    host: String,
    port: u16,
    probe_path: String,
    api_key: Option<String>,
) -> AppResult<()> {
    probe_endpoint_reachable_inner(service, provider_name, host, port, probe_path, api_key).await
}
```

**Note:** the double-request approach (probe_endpoint then a second GET to inspect status) is intentional to keep `probe_endpoint`'s API unchanged. If this becomes a hot path, refactor `probe_endpoint` to also return the status code, then collapse to one request. For now, two short HTTP requests per 10 s poll is negligible.

Actually — re-read this. Doing the request twice is wasteful. Simpler: skip `probe_endpoint` entirely and do one request with our own classification. The classify_reqwest_error helper handles network errors; we add the 401/403 special case for HTTP responses. Replace `probe_endpoint_reachable_inner`'s body with:

```rust
async fn probe_endpoint_reachable_inner(
    service: ServiceKind,
    provider_name: String,
    host: String,
    port: u16,
    probe_path: String,
    api_key: Option<String>,
) -> AppResult<()> {
    use medical_core::preflight::classify_reqwest_error;

    let effective_host = if host.is_empty() { "localhost".to_string() } else { host };
    let base_url = format!("http://{}:{}", effective_host, port);
    let url = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        probe_path.trim_start_matches('/'),
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| AppError::Config(format!("reachability client build: {e}")))?;

    let mut req = client.get(&url);
    if let Some(key) = api_key.as_deref().filter(|s| !s.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let response = req.send().await.map_err(|e| {
        match classify_reqwest_error(&e) {
            Some(reason) => AppError::EndpointOffline {
                service,
                endpoint: base_url.clone(),
                reason,
                provider_name: provider_name.clone(),
            },
            None => AppError::EndpointOffline {
                service,
                endpoint: base_url.clone(),
                reason: medical_core::error::OfflineReason::ConnectionRefused,
                provider_name: provider_name.clone(),
            },
        }
    })?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(AppError::EndpointOffline {
            service,
            endpoint: base_url,
            reason: medical_core::error::OfflineReason::ConnectionRefused,
            provider_name,
        });
    }

    // Any other HTTP status (200/3xx/404/5xx) = reachable.
    Ok(())
}
```

This is cleaner — one request, one classification path. The `classify_reqwest_error` import is reused from Phase 1 Task 2.

- [ ] **Step 4: Register the command in `src-tauri/src/lib.rs`**

Find where the existing `test_*_connection` commands are registered (search for `test_ollama_connection` in `lib.rs`). Add `probe_endpoint_reachable` to the same Tauri builder list, alongside them. The exact form depends on the project's pattern — likely:

```rust
.invoke_handler(tauri::generate_handler![
    // …existing commands…
    crate::commands::providers::test_ollama_connection,
    crate::commands::providers::test_lmstudio_connection,
    crate::commands::providers::test_stt_remote_connection,
    crate::commands::providers::probe_endpoint_reachable,  // NEW
    // …more…
])
```

- [ ] **Step 5: Build and run the tests**

```bash
cargo build -p rust-medical-assistant 2>&1 | tail -3
cargo test -p rust-medical-assistant --lib providers 2>&1 | tail -15
```

Expected: clean build; 5 new tests pass.

- [ ] **Step 6: Full suite regression**

```bash
cargo test -p rust-medical-assistant --lib 2>&1 | tail -3
```

Expected: existing tests still pass.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/commands/providers.rs src-tauri/src/lib.rs
git commit -m "feat(providers): add probe_endpoint_reachable for lenient polling

Reachability probe used by endpointHealth's background poller.
Treats any HTTP response (200/3xx/404/5xx) as reachable except 401/403
(auth failure → EndpointOffline). Network errors flow through
classify_reqwest_error.

Whisper.cpp's standalone server returns 404 for /v1/models because it
only implements /v1/audio/transcriptions. The strict test_*_connection
commands treat 404 as offline, causing a false-positive banner for
paired clients. probe_endpoint_reachable fixes that without affecting
the strict commands that Settings → Test Connection buttons rely on.

Phase 4 Task 1."
```

---

## Task 2: Switch endpointHealth to `probe_endpoint_reachable`

**Files:**
- Modify: `src/lib/stores/endpointHealth.ts`
- Modify: `src/lib/stores/endpointHealth.test.ts`

**Why:** With the new lenient command in place, switch the poller from `test_*_connection` to `probe_endpoint_reachable`. The pill stops over-reporting offline; banner stops false-firing.

- [ ] **Step 1: Update tests first (they'll fail because the implementation still uses test_*_connection)**

Open `src/lib/stores/endpointHealth.test.ts`. Find the existing tests for `probeStt` happy path and `probeAi` happy path. Update them to expect `probe_endpoint_reachable` instead of `test_stt_remote_connection`/`test_ollama_connection`/`test_lmstudio_connection`.

For each, the assertion pattern was:

```ts
expect(invokeMock).toHaveBeenCalledWith('test_stt_remote_connection', {
  host: '192.168.1.20',
  port: 8080,
  apiKey: 'secret',
});
```

Change to:

```ts
expect(invokeMock).toHaveBeenCalledWith('probe_endpoint_reachable', {
  service: 'RemoteStt',
  providerName: 'Whisper STT',
  host: '192.168.1.20',
  port: 8080,
  probePath: '/v1/models',
  apiKey: 'secret',
});
```

Same shape for Ollama (`service: 'AiProvider'`, `providerName: 'Ollama'`, `probePath: '/api/tags'`) and LM Studio (`providerName: 'LM Studio'`, `probePath: '/v1/models'`).

Also update the mocks' `mockResolvedValueOnce` / `mockImplementation` blocks: they previously responded to `'test_stt_remote_connection'` etc. by checking the cmd name. Update those branches to match `'probe_endpoint_reachable'`.

- [ ] **Step 2: Run the tests; expect failures**

```bash
npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -15
```

Expected: tests fail — implementation still calls the old commands.

- [ ] **Step 3: Update `probeStt` and `probeAi`**

In `src/lib/stores/endpointHealth.ts`, the current `probeStt` looks like:

```ts
async function probeStt(cfg: AppConfig): Promise<ServiceStatus> {
  if (cfg.stt_mode !== 'remote') return 'skipped';
  if (isLoopbackHost(cfg.stt_remote_host)) return 'skipped';

  let apiKey: string | undefined = undefined;
  try {
    const key = await invoke<string | null>('get_api_key', { provider: 'stt_remote_api_key' });
    if (key) apiKey = key;
  } catch { /* keychain unavailable */ }

  try {
    await invoke('test_stt_remote_connection', {
      host: cfg.stt_remote_host,
      port: cfg.stt_remote_port,
      apiKey,
    });
    return 'online';
  } catch {
    return 'offline';
  }
}
```

Replace the inner `invoke` block:

```ts
  try {
    await invoke('probe_endpoint_reachable', {
      service: 'RemoteStt',
      providerName: 'Whisper STT',
      host: cfg.stt_remote_host,
      port: cfg.stt_remote_port,
      probePath: '/v1/models',
      apiKey,
    });
    return 'online';
  } catch {
    return 'offline';
  }
```

Apply identical changes to the Ollama and LM Studio branches in `probeAi`. The values:

- Ollama: `service: 'AiProvider'`, `providerName: 'Ollama'`, `probePath: '/api/tags'`.
- LM Studio: `service: 'AiProvider'`, `providerName: 'LM Studio'`, `probePath: '/v1/models'`.

- [ ] **Step 4: Run the tests**

```bash
npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -15
```

Expected: all tests pass.

- [ ] **Step 5: Full frontend sweep + svelte-check**

```bash
npx vitest run 2>&1 | tail -5
npm run check 2>&1 | tail -10
```

Expected: 172 tests pass; 0 svelte-check errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/stores/endpointHealth.ts src/lib/stores/endpointHealth.test.ts
git commit -m "feat(endpointHealth): use probe_endpoint_reachable for polling

probeStt and probeAi now call the new lenient reachability command
instead of the strict test_*_connection commands. The pill stops
over-reporting offline when the server returns 404 (Whisper.cpp on
/v1/models). Settings → Test Connection buttons keep their strict
behavior — those uses test_*_connection unchanged.

Phase 4 Task 2."
```

---

## Task 3: Manual QA + version bump 0.10.61

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `package.json`
- Modify: `src-tauri/tauri.conf.json`

- [ ] **Step 1: Final automated sweep**

```bash
cargo test --workspace --lib 2>&1 | tail -3
npx vitest run 2>&1 | tail -3
npm run check 2>&1 | tail -5
```

Expected: 603 backend / 172 frontend, 0 svelte-check errors (1 pre-existing ExportDialog warning).

- [ ] **Step 2: Bump 0.10.60 → 0.10.61**

Edit three files: `version = "0.10.60"` → `version = "0.10.61"` (or `"version": "0.10.61"` in JSON).

- `src-tauri/Cargo.toml`
- `package.json`
- `src-tauri/tauri.conf.json`

Verify:

```bash
grep -E '0\.10\.6[01]' src-tauri/Cargo.toml package.json src-tauri/tauri.conf.json
```

- [ ] **Step 3: Final build after bump**

```bash
cargo build -p rust-medical-assistant 2>&1 | tail -3
```

- [ ] **Step 4: Commit + tag**

```bash
git add src-tauri/Cargo.toml package.json src-tauri/tauri.conf.json Cargo.lock
git commit -m "chore: bump 0.10.61 — fix STT polling false negative (Phase 4)

The Phase 2 polling probe (added in v0.10.58) sent GET /v1/models to
test STT reachability. Whisper.cpp's standalone server returns 404
for that path because it only implements /v1/audio/transcriptions.
test_stt_remote_connection treated the 404 as offline, causing a
false-positive banner for paired clients on v0.10.60.

This release adds probe_endpoint_reachable — a lenient probe that
treats any HTTP response except 401/403 as reachable. endpointHealth's
poller now uses it. Settings → Test Connection buttons keep using
the strict test_*_connection commands."

git tag v0.10.61
```

(Don't push the tag — user pushes when QA passes.)

## Self-Review

**Spec coverage:**
- AC#1 (new command exists) → Task 1 ✓
- AC#2 (any HTTP except 401/403 = Ok) → Task 1 tests cover 200, 404, 401 ✓
- AC#3 (401/403 = EndpointOffline) → Task 1 test ✓
- AC#4 (network errors flow through) → Task 1 connect-refused test ✓
- AC#5 (endpointHealth uses new command) → Task 2 ✓
- AC#6 (test_*_connection unchanged) → no edit to those functions ✓
- AC#7 (existing tests pass after migration) → Task 2 Step 4 ✓
- AC#8 (manual QA: green pill on paired setup) → Task 3 ✓

**Placeholder scan:** none.

**Type consistency:** `service` enum values `'RemoteStt'` / `'AiProvider'` match the Rust `ServiceKind` PascalCase serialization. `probePath` is a free-form string in both. All shapes match.

---

Plan complete and saved to `docs/superpowers/plans/2026-05-13-stt-polling-false-negative.md`. Two execution options:

**1. Subagent-Driven (recommended)** — fresh subagent per task with two-stage review.
**2. Inline Execution** — checkpoints between tasks.

Which approach?
