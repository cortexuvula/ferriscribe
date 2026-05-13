# Paired-Bearer Auto-Fill — Design

**Status:** draft 2026-05-13
**Author:** brainstorm session, 2026-05-13
**Scope:** Phase 3 of the server-down / sharing UX effort. Fixes a real defect uncovered after Phase 1 + Phase 2 shipped (v0.10.57 / v0.10.58 / v0.10.59 to master): paired clients receive a bearer token at pair time but the rest of the app — Settings → Audio, Settings → Models, the pre-flight probe, and the endpointHealth poller — does not see it. Paired clients hit the auth proxy without an `Authorization` header and get 401 from every STT/AI call. Phase 3 unifies the storage so a successful pair populates every field the app reads from.

## Problem

A user paired the Windows client to the Mac office server with v0.10.59. The pair flow succeeded — `Room-6` appears in the office server's "Connected clients" list. But the Windows client's Settings → Audio → API Key field is empty, the Whisper STT probe returns 401, and the Phase 2 banner ("Office server offline — transcription and SOAP will fail") shows.

Investigation found two parallel storage paths that never meet:

- **Pair-flow path.** `commands::sharing::pairing::pair_with_server` (`src-tauri/src/commands/sharing/pairing.rs:87-209`) writes the bearer to keychain at `"sharing-bearer"`, serializes a `PairedConnection { lan, tailscale, ports, label }` to `~/.local/share/rust-medical-assistant/sharing-paired.json`, and rebuilds the in-memory `state.ollama_provider`, `state.lmstudio_provider`, and `state.stt_providers` with the resolved `RemoteEndpoint` carrying that bearer.
- **Settings path.** The rest of the app — `endpointHealth.ts`, `test_*_connection` Tauri commands, the Phase 1 pre-flight, the Settings → Audio / Models UI — reads `stt_remote_host` / `stt_remote_port` / `stt_remote_api_key` (keychain) / `ollama_host` / `ollama_port` / `lmstudio_host` / `lmstudio_port` from `AppConfig` (the settings JSON in DB).

Pairing populates the first path. Nothing copies that state to the second path. Result:

- The in-memory provider used by actual transcription has the right bearer (because `pair_with_server` calls `provider.set_endpoint(remote_endpoint_with_bearer)`).
- But every other code path — including pre-flight, the Phase 2 polling, and the user-visible Settings UI — sees empty fields and behaves as if the user never paired.
- Worse: `test_ollama_connection` and `test_lmstudio_connection` don't even accept an `api_key` argument today. Even if the bearer existed in the right slot, those commands couldn't send it.

Phase 3 unifies the two paths.

## Non-goals

- **Redesigning the bearer storage.** `"sharing-bearer"` stays where it is for backward compatibility with already-paired clients. Phase 3 *mirrors* the value into the settings-path slots; it doesn't replace the pair-flow's keychain entry.
- **Refactoring `RemoteEndpoint` away.** The in-memory `RemoteEndpoint` abstraction is correct for the actual call path (LAN-then-Tailscale probing, bearer caching). Phase 3 doesn't change that.
- **A new "paired" STT mode.** Settings stays `stt_mode: 'local' | 'remote'`. After pairing, `stt_mode` is `'remote'` and the standard remote-STT code path handles it — just with values populated by the pair flow.
- **Provider switching on pair.** If the user is on `ai_provider: 'lmstudio'` when they pair, they stay on LM Studio. Pairing populates both Ollama and LM Studio host/port/api_key so the user can switch later without re-pairing, but does NOT change `ai_provider`.
- **Settings → Sharing UX changes.** No new buttons, no new fields surfaced. The user pairs once and the app handles the rest. (A future iteration could add a "view bearer" affordance, but that's not Phase 3.)
- **Migration UI for previously-paired clients.** On v0.10.60 first launch, already-paired clients won't have their settings auto-populated unless they unpair-and-re-pair. The release notes will mention this — it's a one-shot inconvenience for ~one paired client (the user).

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│  pair_with_server (commands/sharing/pairing.rs)                      │
│                                                                       │
│  After the existing successful enroll:                                │
│    1. (existing) keyring set "sharing-bearer" = token                 │
│    2. (existing) write sharing-paired.json                            │
│    3. (existing) rebuild in-memory providers                          │
│    4. (NEW) keyring set "stt_remote_api_key" = token                  │
│    5. (NEW) keyring set "ollama_api_key" = token                      │
│    6. (NEW) keyring set "lmstudio_api_key" = token                    │
│    7. (NEW) update AppConfig in DB:                                   │
│         stt_mode = 'remote'                                           │
│         stt_remote_host = resolved_host (LAN preferred, TS fallback)  │
│         stt_remote_port = ports.whisper (= 8081)                      │
│         ollama_host = resolved_host                                   │
│         ollama_port = ports.ollama (= 11435)                          │
│         lmstudio_host = resolved_host                                 │
│         lmstudio_port = ports.lmstudio (= 1235, if Some)              │
│         (ai_provider unchanged)                                       │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│  test_ollama_connection / test_lmstudio_connection                   │
│  (src-tauri/src/commands/providers.rs)                               │
│                                                                       │
│  Signature gains an optional api_key:                                 │
│    fn test_ollama_connection(host: String, port: u16,                 │
│                              api_key: Option<String>) -> AppResult    │
│  When Some, sends Authorization: Bearer <api_key>.                    │
│  401 response → existing "Authentication failed" message              │
│  (already present for test_stt_remote_connection).                    │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│  endpointHealth store (src/lib/stores/endpointHealth.ts)             │
│                                                                       │
│  probeAi() — extends Task 1 fix pattern to AI:                        │
│    Fetch ollama_api_key / lmstudio_api_key via                        │
│    invoke('get_api_key', {provider: <key>}) before the probe.         │
│    Pass to test_ollama_connection / test_lmstudio_connection.         │
│    Keychain failure → continue without auth (existing pattern).       │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│  unpair (commands/sharing/pairing.rs)                                │
│                                                                       │
│  After existing keyring/file cleanup:                                 │
│    keyring delete "stt_remote_api_key"                                │
│    keyring delete "ollama_api_key"                                    │
│    keyring delete "lmstudio_api_key"                                  │
│    AppConfig update:                                                  │
│      stt_mode = 'local'                                               │
│      stt_remote_host = ''                                             │
│      stt_remote_port = 8080 (default)                                 │
│      ollama_host = 'localhost', ollama_port = 11434 (defaults)        │
│      lmstudio_host = 'localhost', lmstudio_port = 1234 (defaults)     │
│      ai_provider unchanged (preserves user choice)                    │
└─────────────────────────────────────────────────────────────────────┘
```

Five touch points: pair handler, unpair handler, two test commands (signature change + auth header), and the frontend endpointHealth store. The pair handler's logic — choosing a resolved host, deciding which keychain slots to write — is the bulk of the work. Everything else is mechanical.

## Components

### Modified: `src-tauri/src/commands/sharing/pairing.rs::pair_with_server`

After the existing block at line 206 (after `init_stt_providers_with_config` is called), add:

```rust
// Phase 3: mirror the bearer into the per-service keychain slots and
// populate AppConfig host/port fields so the rest of the app (test_*
// connection commands, endpointHealth polling, Settings UI) sees the
// paired endpoint.

// 1. Resolve the host the rest of the app should use. Prefer LAN; fall
//    back to Tailscale. (Matches the existing RemoteEndpoint.resolve_base_url
//    precedence in crates/core/src/types/endpoint.rs.)
let resolved_host = lan.clone().or_else(|| tailscale.clone());
let host = match resolved_host {
    Some(h) => h,
    None => {
        // Defensive: pair_with_server already requires at least one address
        // in its argument validation, but be explicit.
        return Err(AppError::Other(
            "pair link missing both LAN and Tailscale addresses".into(),
        ));
    }
};

// 2. Write the bearer to per-service keychain slots.
//    Each key matches what get_api_key(provider) returns elsewhere.
keyring_set_secret("stt_remote_api_key", &token)?;
keyring_set_secret("ollama_api_key", &token)?;
keyring_set_secret("lmstudio_api_key", &token)?;

// 3. Populate AppConfig.
let mut cfg = SettingsRepo::load_config(&conn)
    .map_err(|e| AppError::Database(e.to_string()))?;
cfg.migrate();
cfg.stt_mode = SttMode::Remote;
cfg.stt_remote_host = host.clone();
cfg.stt_remote_port = ports.whisper;
cfg.ollama_host = host.clone();
cfg.ollama_port = ports.ollama;
if let Some(lp) = ports.lmstudio {
    cfg.lmstudio_host = host.clone();
    cfg.lmstudio_port = lp;
}
// Do NOT touch cfg.ai_provider — preserve user choice.
SettingsRepo::save_config(&conn, &cfg)
    .map_err(|e| AppError::Database(e.to_string()))?;
```

`keyring_set_secret(name, value)` is a thin helper added in this task that wraps `keyring::Entry::new("rustMedicalAssistant", name).set_password(value)` with the project's existing error mapping. If a similar helper already exists, reuse it.

### Modified: `src-tauri/src/commands/sharing/pairing.rs::unpair`

After the existing keychain + file cleanup (lines 224-238), add:

```rust
// Phase 3: clear the per-service keychain slots and reset AppConfig
// fields the pair flow populated. Don't change ai_provider — preserve
// the user's choice.

for slot in &["stt_remote_api_key", "ollama_api_key", "lmstudio_api_key"] {
    // Idempotent — ignore "not found" errors.
    let _ = keyring::Entry::new("rustMedicalAssistant", slot)
        .and_then(|e| e.delete_credential());
}

let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
let mut cfg = SettingsRepo::load_config(&conn)
    .map_err(|e| AppError::Database(e.to_string()))?;
cfg.migrate();
cfg.stt_mode = SttMode::Local;
cfg.stt_remote_host = String::new();
cfg.stt_remote_port = 8080;       // matches default_stt_remote_port
cfg.ollama_host = "localhost".into();
cfg.ollama_port = 11434;
cfg.lmstudio_host = "localhost".into();
cfg.lmstudio_port = 1234;
SettingsRepo::save_config(&conn, &cfg)
    .map_err(|e| AppError::Database(e.to_string()))?;
```

The default-value resets match `default_*` functions in `crates/core/src/types/settings.rs`. If those defaults change, this code should match — extract a helper if drift becomes a risk.

### Modified: `src-tauri/src/commands/providers.rs::test_ollama_connection` + `::test_lmstudio_connection`

Both gain an `api_key: Option<String>` parameter:

```rust
#[tauri::command]
pub async fn test_ollama_connection(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
    api_key: Option<String>,   // NEW
) -> AppResult<String> {
    // …existing code…
    let mut req = state.http_client.get(&url).timeout(Duration::from_secs(5));
    if let Some(key) = api_key.as_deref().filter(|s| !s.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let response = req.send().await.map_err(/* …existing classify_reqwest_error path… */)?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(AppError::AiProvider(
            "Authentication failed — verify the API key, or if this is a paired client, \
             re-pair the office server (Settings → Sharing → Unpair, then scan a fresh code)."
                .into(),
        ));
    }
    // …rest of existing code unchanged…
}
```

Same shape for `test_lmstudio_connection`. (The wording is borrowed verbatim from the existing `test_stt_remote_connection` 401 branch — keep clients seeing consistent language across services.)

`test_stt_remote_connection` already has the `api_key` parameter (Phase 1 left it). No change needed there.

### Modified: `src/lib/stores/endpointHealth.ts::probeAi`

Mirror Phase 2 Task 1's STT pattern. Inside `probeAi`:

```ts
async function probeAi(cfg: AppConfig): Promise<ServiceStatus> {
  if (cfg.ai_provider === 'ollama') {
    if (isLoopbackHost(cfg.ollama_host)) return 'skipped';
    let apiKey: string | undefined = undefined;
    try {
      const key = await invoke<string | null>('get_api_key', {
        provider: 'ollama_api_key',
      });
      if (key) apiKey = key;
    } catch {
      // Keychain unavailable — continue without auth.
    }
    try {
      await invoke('test_ollama_connection', {
        host: cfg.ollama_host,
        port: cfg.ollama_port,
        apiKey,
      });
      return 'online';
    } catch {
      return 'offline';
    }
  }
  if (cfg.ai_provider === 'lmstudio') {
    // …identical shape, swap to 'lmstudio_api_key' and 'test_lmstudio_connection'…
  }
  return 'skipped';
}
```

### Frontend Settings → Audio reactivity

After the pair handler updates `AppConfig` and saves, the frontend `settings` writable store needs to reflect the new values so the Settings UI shows them. The existing pair flow calls `init_stt_providers_with_config` which doesn't notify the frontend. Phase 3 must ensure the frontend's `settings.load()` runs after pair (and after unpair). Two options:

- **A:** The pair Tauri command emits a `settings-updated` Tauri event after saving; the frontend `settings.ts` listens for it and calls `settings.load()`.
- **B:** The pair flow's frontend caller (the Sharing pane) calls `settings.load()` after a successful pair invoke.

Option B is simpler — the Sharing component already runs in the same dispatcher and can chain a `.then(() => settings.load())`. Use B unless the project already has a `settings-updated` event pattern, in which case use A for consistency.

## Data flow

### Happy path: client pairs for the first time

1. User scans QR or enters 6-digit code on the Windows client.
2. Frontend invokes `pair_with_server({ lan, tailscale, ports, code, label })`.
3. Backend: POST to `{base}/pair/enroll`, receives `{token: "abc123..."}`.
4. **(existing)** Bearer → `keyring "sharing-bearer"`.
5. **(existing)** `PairedConnection` → `sharing-paired.json`.
6. **(existing)** In-memory providers rebuilt with `RemoteEndpoint` containing the bearer.
7. **(new)** Bearer → `keyring "stt_remote_api_key"`, `"ollama_api_key"`, `"lmstudio_api_key"`.
8. **(new)** `AppConfig` updated: `stt_mode='remote'`, `stt_remote_host=<lan>`, `stt_remote_port=8081`, `ollama_host=<lan>`, `ollama_port=11435`, `lmstudio_host=<lan>`, `lmstudio_port=1235`. Saved to DB.
9. Frontend caller chains `.then(() => settings.load())`. The reactive `$settings` store updates. Settings → Audio / Models UI shows the new values. The endpointHealth poller's `settings.subscribe` fires; the change triggers `probeNow()`.
10. Next probe: `probeStt` fetches `stt_remote_api_key`, calls `test_stt_remote_connection({host, port, apiKey})`, proxy returns 200. STT = online. `probeAi` fetches `ollama_api_key`, calls `test_ollama_connection({host, port, apiKey})`, proxy returns 200. AI = online.
11. `endpointHealth.overall = 'online'`. The pill is green. The OfflineRecordBanner is hidden. Transcription works.

### Happy path: client unpairs

1. User clicks Unpair in Settings → Sharing.
2. Frontend invokes `unpair()`.
3. Backend: clears `sharing-paired.json`, `keyring "sharing-bearer"`, and the three new per-service keychain slots. Resets AppConfig fields to defaults.
4. In-memory providers are rebuilt with `RemoteEndpoint = None`.
5. Frontend caller chains `settings.load()`. UI shows blank/default values. endpointHealth's settings subscriber fires `probeNow()`; with `stt_mode='local'` and loopback hosts, both probes return `'skipped'`; `overall='hidden'`; pill disappears.

### Migration path: client already paired on v0.10.59 upgrading to v0.10.60

On first launch after upgrade, the user is paired (`sharing-paired.json` exists, `keyring "sharing-bearer"` is set) but the new per-service slots and AppConfig fields are NOT populated. STT/AI calls still work through the in-memory providers (rebuilt at startup from `sharing-paired.json`), but probes still fail with 401 (banner still shows).

To trigger Phase 3's auto-fill, the user must **unpair-and-re-pair once**. Release notes for 0.10.60 must call this out:

> **Action required if you paired before 0.10.60:** Open Settings → Sharing on the client, click Unpair, then re-pair (scan the QR or enter the code). This is a one-time step — 0.10.60 populates new settings fields that older pair runs didn't write.

An automated migration is possible (on startup, if `sharing-paired.json` exists and the per-service slots are empty, derive them from the existing bearer + paired-connection). Defer to a follow-up if not needed.

### Pair flow returns an error (network down, code expired, etc.)

1. The `/pair/enroll` POST fails.
2. None of the Phase 3 writes happen. Existing rollback / no-op behavior is preserved.

## Settings handling

- The pair flow's writes to `AppConfig` use `SettingsRepo::save_config`, the same path the Settings UI uses. No new schema, no new fields.
- The keychain slot names match what `get_api_key(provider)` already expects. No new Tauri commands.
- The pair flow takes the `lan` field for `stt_remote_host` etc. — the LAN address is preferred over Tailscale for the static AppConfig fields because the in-memory `RemoteEndpoint` already handles dynamic LAN-vs-Tailscale fallback at request time. The Settings UI showing `192.168.4.37` is more meaningful to a user than `100.95.40.47` (Tailscale CGNAT addresses are opaque).

## Logging

Per CLAUDE.md (no PHI in logs). The new code paths:

- Pair handler logs at `info!` the labels and host being set, NOT the bearer.
- Unpair handler logs at `info!` what was cleared, NOT secrets.
- `tracing::warn!` on keychain write failure with the slot name (e.g. `slot=ollama_api_key`) but never the value.

The bearer string itself never appears in any log call.

## Testing strategy

### Backend (Rust)

In `src-tauri/src/commands/sharing/pairing.rs` (or sibling test file):

- `pair_with_server_populates_stt_keychain_and_settings` — mock the enroll endpoint, run the command, assert `keyring "stt_remote_api_key"` is set and `AppConfig.stt_mode == Remote`, `stt_remote_host == "192.168.4.37"`, `stt_remote_port == 8081`.
- `pair_with_server_populates_ollama_and_lmstudio_too` — same shape, assert `ollama_api_key` set and `ollama_host`/`port` populated. If `lmstudio` port is `None` in the pair payload, the lmstudio fields are not touched.
- `pair_with_server_preserves_ai_provider` — initial config has `ai_provider='lmstudio'`. After pair, still `'lmstudio'`.
- `unpair_clears_keychain_and_resets_settings` — pair, then unpair; assert all three slots deleted; assert AppConfig back to defaults; assert `ai_provider` unchanged.
- `pair_with_server_handles_missing_lan_and_tailscale` — both `lan` and `tailscale` are `None`; returns an error without writing anything.

In `src-tauri/src/commands/providers.rs`:

- `test_ollama_connection_sends_bearer_when_api_key_provided` — wiremock asserts the request includes `authorization: Bearer secret-token`; helper command returns Ok.
- `test_ollama_connection_returns_auth_failed_on_401` — wiremock returns 401; assert the error message contains "Authentication failed".
- Same two tests for `test_lmstudio_connection`.

### Frontend (TypeScript)

In `src/lib/stores/endpointHealth.test.ts`:

- `fetches_ollama_api_key_from_keychain_and_forwards_to_AI_probe` — mock `get_api_key` for `'ollama_api_key'`; assert `test_ollama_connection` receives `apiKey: 'secret-token-xyz'`.
- `AI_probe_continues_without_auth_if_keychain_fetch_fails` — same shape as the existing STT test added in Phase 2 Task 1 fix.

### Manual QA

1. Fresh install of v0.10.60 on Windows; office server on Mac. Pair from Windows. Verify:
   - Settings → Audio shows `stt_mode: Remote`, `host = <Mac LAN IP>`, `port = 8081`, and the API Key field is non-empty.
   - Settings → Models shows `ollama_host`/`lmstudio_host` set to `<Mac LAN IP>`, ports 11435 / 1235 (if present).
   - Status pill is green within ~10s.
   - Record a 5-second test consultation. Transcription completes. SOAP generates.
2. Click Unpair. Verify the Settings fields revert to local defaults. Pill disappears within 10s.
3. From a v0.10.59 install with an existing pair, upgrade to v0.10.60. Verify the release-note instruction is shown / banner still appears (no auto-migration). Unpair and re-pair. Verify Phase 3 takes effect.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Pair handler partially succeeds (bearer in old slot but new slots fail) | Wrap the three keychain writes + the AppConfig update in a best-effort group with rollback. If any fails, log at `error!`, leave the bearer in `"sharing-bearer"` (so the in-memory path still works), and surface a soft warning in the Sharing UI. |
| Migration confusion for already-paired users | Release-note callout. Inline note in the Sharing pane: "If you paired before 0.10.60, unpair and re-pair to enable auto-fill." |
| User manually edits Settings → Audio after pair, then re-pairs | Pair handler overwrites without asking. Acceptable for Phase 3 — the alternative (modal confirm-overwrite) is more UI work than is justified. Note in the spec for future iteration. |
| `keyring::Entry::delete_credential` panics or errors on missing entry on some platforms | Already idempotent in the existing unpair path (lines 227-229 wrap in `let _ =`). Apply the same pattern to the new slots. |
| `test_ollama_connection`'s new `api_key` parameter breaks existing callers | All callers are inside this repo. Update Settings → Models's "Test Connection" button to also pass the keychain api_key. Frontend type changes flow through TypeScript types — svelte-check will catch any miss. |

## Acceptance criteria

1. `pair_with_server` writes the bearer to keychain slots `"stt_remote_api_key"`, `"ollama_api_key"`, `"lmstudio_api_key"`.
2. `pair_with_server` updates `AppConfig` with `stt_mode=Remote`, `stt_remote_host`, `stt_remote_port=ports.whisper`, `ollama_host`, `ollama_port=ports.ollama`, `lmstudio_host`, `lmstudio_port=ports.lmstudio` (when Some). `ai_provider` is unchanged.
3. `unpair` clears all three keychain slots and resets the listed `AppConfig` fields to defaults (mode=Local, hosts/ports to default values). `ai_provider` is unchanged.
4. `test_ollama_connection` and `test_lmstudio_connection` accept an optional `api_key` and, when provided, send `Authorization: Bearer <api_key>`. 401 responses produce the same "Authentication failed" message used by `test_stt_remote_connection`.
5. `endpointHealth.ts::probeAi` fetches the appropriate per-provider api_key from keychain and forwards it to the test command.
6. `Settings → Audio` and `Settings → Models` on the client reflect the populated fields after pair (frontend reloads `settings` after the pair invoke resolves).
7. Manual QA passes the three steps in the testing strategy above.
8. Existing tests in `crates/sharing/tests/pairing.rs` and `crates/core/src/error.rs` continue to pass.
9. `cargo test --workspace --lib` and `npx vitest run` are green.
10. `npm run check` produces no new errors.
