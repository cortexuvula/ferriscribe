# STT Polling False-Negative — Design

**Status:** draft 2026-05-13
**Author:** brainstorm session, 2026-05-13
**Scope:** Hotfix following Phase 3 (paired-bearer auto-fill, v0.10.60). A real user reported a false-positive offline banner after re-pairing on v0.10.60: the Phase 2 polling probe reports STT as offline, even though actual transcription succeeds.

## Problem

After v0.10.60 deployed and the user re-paired the Windows client to the Mac office server, the AI status pill shows **partial** (yellow) and the record-time banner reads **"Whisper STT offline — your recording will save locally, but transcription will fail."** Both messages are wrong: an actual transcription completes successfully.

Root cause: Phase 2's `endpointHealth` poller calls `test_stt_remote_connection`, which sends `GET /v1/models`. The auth proxy at port 8081 forwards the request to local Whisper.cpp at port 8080. Whisper.cpp's standalone server (per `crates/sharing/src/whisper_supervisor.rs:257`) only exposes `/v1/audio/transcriptions` — it does not implement `/v1/models`. The response is HTTP 404. Phase 1 Task 4's refactor of `test_stt_remote_connection` treats any non-2xx response as an error, which Phase 2's `probeStt` catches and marks as `'offline'`.

The same probe works fine for Ollama (`/api/tags` exists) and LM Studio (`/v1/models` exists). Only Whisper.cpp is missing the discovery endpoint.

This is a real architectural mismatch between two distinct semantics that share one command today:

- **Strict, user-triggered:** *"Settings → Audio → Test Connection"* — the clinician explicitly wants to know "can this server list models?". A 404 should fail.
- **Lenient, background polling:** *"endpointHealth poller"* — only needs "is the server reachable?". Any HTTP response (including 404) means the server is alive.

Phase 1 actually already has the right primitive for the lenient case: `probe_endpoint` in `crates/core/src/preflight.rs` returns `Ok(())` for any HTTP status and only fails on connect/timeout/DNS/TLS network errors. Phase 4 exposes that primitive as its own Tauri command and switches the poller to use it.

## Non-goals

- **Changing `test_stt_remote_connection` semantics.** Settings → Audio → Test Connection users genuinely want to know "does this look like an OpenAI-compatible server?". Keep strict mode there.
- **Changing the AI providers' polling path.** Ollama's `/api/tags` and LM Studio's `/v1/models` both work and return 200. Their `test_*_connection` commands stay in the poller's call path *for now* — but they'll switch to the new reachability command in this same release for consistency.
- **Discovery / synthetic `/v1/models` in the auth proxy.** Possible future work to add a synthetic `/v1/models` route to the Whisper auth proxy, but unnecessary for this hotfix.
- **New banner / pill copy.** The existing copy is right; the bug is just that the state machine reaches "offline" incorrectly.

## Architecture

```
  endpointHealth poller
        ↓
        ├──── (new) invoke('probe_endpoint_reachable', { service, host, port, apiKey, probePath })
        │           │
        │           ▼
        │     src-tauri/src/commands/providers.rs::probe_endpoint_reachable
        │           │
        │           ▼
        │     medical_core::preflight::probe_endpoint
        │           │
        │           ▼
        │     reqwest GET → any HTTP status → Ok(())
        │                   network failure → AppError::EndpointOffline { reason, … }
        │
        └─ (no change to Settings → Test Connection)
              Settings → Audio → Test Connection button → test_stt_remote_connection (strict)
              Settings → Models → Test Connection button → test_ollama/lmstudio_connection (strict)
```

The two paths now have separate Tauri commands matching their semantics. `endpointHealth.probeStt` / `probeAi` switch to the new lenient command. Test commands stay strict for explicit user-triggered checks.

## Components

### New: `src-tauri/src/commands/providers.rs::probe_endpoint_reachable`

```rust
/// Lenient reachability probe — calls medical_core::preflight::probe_endpoint
/// which treats ANY HTTP status (200/404/5xx) as "server is alive" and only
/// returns Err on connect/timeout/DNS/TLS network failures.
///
/// Used by the background endpointHealth poller. NOT used by Settings →
/// Test Connection buttons (those use the strict test_*_connection commands).
#[tauri::command]
pub async fn probe_endpoint_reachable(
    service: ServiceKind,      // "AiProvider" | "RemoteStt"
    provider_name: String,     // e.g. "Whisper STT" — appears in EndpointOffline.provider_name
    host: String,
    port: u16,
    probe_path: String,        // "/v1/models" for OpenAI-compat; "/api/tags" for Ollama
    api_key: Option<String>,
) -> AppResult<()>;
```

Returns `Ok(())` on any HTTP response (incl. 404, 5xx, 401 — *all* of these mean "reachable"; 401 is *especially* "reachable + auth required"). Returns `Err(AppError::EndpointOffline { … })` on connect/timeout/DNS/TLS — same shape Phase 1's pre-flight uses.

The implementation is a 5-line wrapper around `probe_endpoint`:

```rust
let base_url = format!("http://{}:{}", host, port);
probe_endpoint(service, &provider_name, &base_url, &probe_path, api_key.as_deref()).await
```

### Modified: `src/lib/stores/endpointHealth.ts`

Both `probeStt` and `probeAi` switch from `test_*_connection` to `probe_endpoint_reachable`. The shape is parallel:

```ts
async function probeStt(cfg: AppConfig): Promise<ServiceStatus> {
  if (cfg.stt_mode !== 'remote') return 'skipped';
  if (isLoopbackHost(cfg.stt_remote_host)) return 'skipped';

  let apiKey: string | undefined = undefined;
  try {
    const key = await invoke<string | null>('get_api_key', {
      provider: 'stt_remote_api_key',
    });
    if (key) apiKey = key;
  } catch { /* continue without auth */ }

  try {
    await invoke('probe_endpoint_reachable', {
      service: 'RemoteStt',
      providerName: 'Whisper STT',
      host: cfg.stt_remote_host,
      port: cfg.stt_remote_port,
      probePath: '/v1/models',     // value irrelevant — any response = reachable
      apiKey,
    });
    return 'online';
  } catch {
    return 'offline';
  }
}
```

`probeAi` follows the same shape with `service: 'AiProvider'`, `providerName: 'Ollama'` or `'LM Studio'`, and the corresponding `probePath`.

### Unchanged: `test_stt_remote_connection` / `test_ollama_connection` / `test_lmstudio_connection`

The three strict commands keep their existing signature and behavior. Settings → Audio / Models "Test Connection" buttons keep using them.

## Data flow

### Happy path (paired client, Whisper.cpp on Mac)

1. Poller tick (every 10 s).
2. `probeStt` invokes `probe_endpoint_reachable({ service: 'RemoteStt', providerName: 'Whisper STT', host: '192.168.4.173', port: 8081, probePath: '/v1/models', apiKey: '<bearer>' })`.
3. Tauri command builds `http://192.168.4.173:8081/v1/models` with `Authorization: Bearer <bearer>`.
4. Auth proxy at Mac:8081 validates bearer (success), forwards to `localhost:8080/v1/models`.
5. Whisper.cpp returns **404 — File Not Found (/v1/models)**.
6. `probe_endpoint` receives the 404 response, treats it as reachable, returns `Ok(())`.
7. `probeStt` returns `'online'`.
8. Pill: green. Banner: hidden. Transcription was already working; now the UI agrees.

### Network failure (server truly down)

1. Poller tick.
2. `probe_endpoint_reachable` invoked.
3. TCP connect refused / DNS failure / timeout.
4. `probe_endpoint` classifies via `classify_reqwest_error` → returns `Err(AppError::EndpointOffline { service, endpoint, reason, provider_name })`.
5. `probeStt` catches, returns `'offline'`.
6. Pill: red. Banner: shown. *Correct* — the server actually is unreachable.

### Auth failure (paired with revoked / wrong bearer)

1. Poller tick.
2. `probe_endpoint_reachable` invoked.
3. Auth proxy returns 401.
4. `probe_endpoint` treats 401 as reachable → `Ok(())`.
5. `probeStt` returns `'online'`. Pill: green.

Wait — that's wrong for the user. If the bearer is invalid, transcription will fail when the user records, but the pill says "online". This is the trade-off of treating "any response" as reachable.

**Mitigation:** the Phase 1 dialog (which fires at processing time via `AppError::EndpointOffline`) still catches the auth failure at the moment of an actual transcribe call. The pill is "ambient reassurance," not the source of truth. A user with a revoked bearer would see a green pill and then hit the dialog when they actually try to transcribe — at which point Phase 1's "Authentication failed — re-pair" message guides them.

The alternative — making the reachability probe also flag 401 — would mean adding a special-case for that status code. Acceptable, and arguably better UX. Adopt it: if the response is 401/403, return `Err(AppError::EndpointOffline { reason: ConnectionRefused, ... })` so the pill reflects auth issues too. (Reason variant choice debatable — there isn't an `AuthFailed` variant; reusing `ConnectionRefused` is the closest fit and the dialog copy still makes sense.)

Update the spec accordingly:

```rust
// probe_endpoint_reachable's flow:
// - 2xx-3xx-404-5xx → Ok(()) (reachable)
// - 401-403 → Err(EndpointOffline { reason: ConnectionRefused, ... })
//   (special case — auth issue should surface in the pill)
// - connect/timeout/DNS/TLS → Err(EndpointOffline { reason: ... }) (existing)
```

The 401/403 special-case is added to the wrapper, not to `probe_endpoint` itself (which stays a pure reachability probe — useful for other callers in the future).

## Testing

### Backend
- `probe_endpoint_reachable_returns_ok_on_404` — wiremock returns 404, command returns Ok.
- `probe_endpoint_reachable_returns_endpoint_offline_on_401` — wiremock returns 401, command returns Err(EndpointOffline).
- `probe_endpoint_reachable_returns_endpoint_offline_on_connect_refused` — bind+drop pattern.
- (Existing `probe_endpoint` tests already cover the reachability semantics, so the new command's tests are thin wrappers verifying the bearer header is forwarded and the 401 special-case fires.)

### Frontend
- `probeStt sends probe_endpoint_reachable with the bearer and treats Ok as online` — mock invoke to resolve, assert state.stt === 'online'.
- `probeStt treats a probe_endpoint_reachable rejection as offline` — mock invoke to reject with EndpointOffline shape, assert state.stt === 'offline'.
- Same shape for `probeAi` with Ollama / LM Studio.

### Manual QA
1. On v0.10.61, paired client, Whisper.cpp on Mac (the current setup): pill turns green within 10 s. Banner clears.
2. Stop Whisper.cpp on Mac (or kill the auth proxy at port 8081): pill turns red within 10 s, banner shows correctly.
3. Restart Whisper: pill turns green again, banner clears.
4. Revoke the paired client's bearer on the Mac (Settings → Sharing → Revoke for Room-N): pill turns red within 10 s (the 401 special-case fires).

## Acceptance criteria

1. New Tauri command `probe_endpoint_reachable` exists in `src-tauri/src/commands/providers.rs` with the documented signature.
2. The command treats any HTTP response (2xx/3xx/4xx/5xx — except 401/403) as `Ok(())`.
3. 401/403 responses surface as `Err(AppError::EndpointOffline)` so the pill reflects auth issues.
4. Network failures (connect refused, timeout, DNS, TLS) surface as `Err(AppError::EndpointOffline)` via the existing `probe_endpoint` classification.
5. `endpointHealth.ts::probeStt` and `probeAi` call `probe_endpoint_reachable` (not `test_*_connection`).
6. `test_*_connection` commands' signature and strict semantics are UNCHANGED.
7. Existing `endpointHealth` tests pass after migration (mock the new command instead).
8. Manual QA on the user's paired setup: pill goes green within 10 s, banner clears.
