# Server-Down Error Messages — Design

**Status:** draft 2026-05-12
**Author:** brainstorm session, 2026-05-12
**Scope:** Phase 1 of a two-phase effort to improve clinician awareness of remote-server outages. Phase 1 covers the *safety net*: pre-flight checks at processing time and a plain-language dialog for connection failures. Phase 2 (separate spec, future) adds the ambient layer: a persistent "Office server" status pill and a record-time inline banner.

## Problem

The clinician runs the FerriScribe desktop app on a Windows machine; their AI provider (Ollama or LM Studio) and Whisper STT server both live on a Mac on the LAN/Tailscale. When the Mac server is unreachable — closed, asleep, network glitch — the current behavior is:

1. Recording works (it's local).
2. Pressing **Transcribe** or **Generate SOAP** kicks off the full pipeline, which only fails once the underlying HTTP call times out or is refused.
3. The error surfaces in a toast as something like:
   - `Cannot reach Whisper server at http://192.168.1.10:8080: error sending request: Connection refused` (`crates/stt-providers/src/remote_provider.rs:236-251`)
   - `Connection refused — is Ollama running at 192.168.1.10:11434?` (`src-tauri/src/commands/providers.rs:234`)
   - `Office server unreachable on LAN or Tailscale (Ollama).` (`crates/ai-providers/src/ollama.rs:119-126`)

These strings name the right thing but read like developer jargon and offer no path forward. A non-technical clinician sees "Connection refused" and doesn't know what to do, whether their recording is safe, or whether the problem is their app, their network, or their server.

Test-Connection commands already exist (`test_lmstudio_connection`, `test_ollama_connection`, `test_stt_remote_connection` in `src-tauri/src/commands/providers.rs`), but they only run when the user manually clicks a button in Settings. Nothing exercises them during the actual pipeline.

This design closes that gap with two complementary changes that share one new error variant.

## Non-goals

- **Ambient status pill** in the app chrome. Deferred to Phase 2; its own spec.
- **Inline banner above the record button.** Deferred to Phase 2.
- **Background polling / periodic health checks.** Deferred to Phase 2.
- **New Settings UI.** Phase 1 reuses existing Settings → Models and Settings → Audio panes; the dialog deep-links to them but adds no new fields.
- **Retry/circuit-breaker logic on the real call.** The existing retry policy (`crates/ai-providers/src/http_client.rs`) is untouched. Pre-flight is a single-shot probe, not a retry loop.
- **Rewriting non-connection errors.** Real provider errors (5xx, malformed JSON, "model not loaded", auth failures) keep their current `AppError::AiProvider(String)` / `AppError::SttProvider(String)` paths. We only intercept the "the call never reached the server" case.
- **Handling of fully-local setups.** When the configured endpoint resolves to `localhost`/`127.0.0.1`, pre-flight is skipped and behavior is unchanged — the connect-failure error still produces the new `EndpointOffline` variant, but the dialog copy is the same (the user fix is "start your local Ollama," surfaced via the same wording).
- **Telemetry.** Per CLAUDE.md, no phone-home. Failed pre-flights log locally (counts only, never endpoint contents containing PHI — and endpoints don't contain PHI here, but stay disciplined).

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│  Tauri command (generate_soap, transcribe_recording, generate_*,    │
│  chat, …)                                                            │
│                                                                      │
│  1. Load settings                                                   │
│  2. preflight_for_command(kind, &settings) ──► probe each remote    │
│       │                                          endpoint (parallel)│
│       ├─ all reachable  ──► proceed to real call                    │
│       └─ any offline    ──► return AppError::EndpointOffline { … }  │
│  3. Real call runs                                                  │
│  4. If real call returns reqwest::Error with is_connect() /         │
│     is_timeout() / DNS / TLS → map to EndpointOffline at the        │
│     existing call sites (replaces today's string error)             │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              ▼  (Tauri serializes AppError as
                                  { kind, message, …fields })
┌─────────────────────────────────────────────────────────────────────┐
│  Frontend                                                            │
│                                                                      │
│  invoke() catch handler                                              │
│    │                                                                 │
│    ├─ kind === "EndpointOffline"  ──► endpointOfflineStore.open(err) │
│    │                                    └─► <EndpointOfflineDialog/> │
│    │                                        renders title/body/      │
│    │                                        actions by reason +      │
│    │                                        service                  │
│    │                                                                 │
│    └─ any other kind                ──► existing toast / error UI    │
└─────────────────────────────────────────────────────────────────────┘
```

`preflight_for_command` and the new `EndpointOffline` variant are the only two pieces of *shared* infrastructure. Each command opts in by calling pre-flight at the top and (for completeness) the call-site error mapper is updated where the real reqwest call happens.

## Components

### New: `AppError::EndpointOffline` variant

In `crates/core/src/error.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ServiceKind {
    AiProvider,
    RemoteStt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum OfflineReason {
    ConnectionRefused,
    Timeout,
    DnsFailure,
    TlsFailure,
}

// added to AppError:
#[error("Endpoint offline: {provider_name} at {endpoint} ({reason:?})")]
EndpointOffline {
    service: ServiceKind,
    endpoint: String,        // e.g. "http://192.168.1.10:11434"
    reason: OfflineReason,
    provider_name: String,   // user-facing: "Ollama" | "LM Studio" | "Whisper STT"
},
```

The Display impl is developer-facing only (logs, dev console). The frontend renders from the structured fields, never from the Display string.

`AppError`'s existing Serialize impl is extended to emit `EndpointOffline` as:

```json
{
  "kind": "EndpointOffline",
  "service": "AiProvider",
  "endpoint": "http://192.168.1.10:11434",
  "reason": "ConnectionRefused",
  "provider_name": "Ollama",
  "message": "Endpoint offline: Ollama at http://192.168.1.10:11434 (ConnectionRefused)"
}
```

The `message` field is included for parity with other variants (so any generic error logger keeps working), but the dialog reads from `service` / `endpoint` / `reason` / `provider_name`.

### New: `crates/core/src/preflight.rs`

```rust
/// What capability does a given Tauri command depend on?
#[derive(Debug, Clone, Copy)]
pub enum CommandKind {
    Transcribe,        // needs STT (remote or local)
    GenerateSoap,      // needs AI provider
    GenerateReferral,  // needs AI provider
    GenerateLetter,    // needs AI provider
    GenerateSynopsis,  // needs AI provider
    Chat,              // needs AI provider
}

/// Inspect settings, decide which remote endpoints this command needs,
/// probe each in parallel with a 3s timeout, return Ok(()) if all are
/// reachable (or the command's endpoints are all local) and the
/// appropriate EndpointOffline error otherwise.
///
/// First failure wins; remaining probes are dropped (their results
/// aren't surfaced in Phase 1 — the dialog is per-service, and the
/// user fix is the same either way).
pub async fn preflight_for_command(
    kind: CommandKind,
    settings: &AppConfig,
) -> Result<(), AppError>;

/// Lower-level probe used by preflight_for_command and reusable for
/// future health-check polling (Phase 2).
async fn probe_endpoint(
    service: ServiceKind,
    provider_name: &str,
    base_url: &str,
    probe_path: &str,    // "/api/tags" | "/v1/models"
    bearer: Option<&str>,
) -> Result<(), AppError>;
```

`probe_endpoint` makes a single `GET base_url + probe_path` with `Duration::from_secs(3)`, no retries, no body. Any of:
- `reqwest::Error::is_connect()` → `OfflineReason::ConnectionRefused`
- `reqwest::Error::is_timeout()` → `OfflineReason::Timeout`
- DNS resolution failure (inspect the inner error chain) → `OfflineReason::DnsFailure`
- TLS handshake failure → `OfflineReason::TlsFailure`

…returns `Err(AppError::EndpointOffline { … })`. A 4xx/5xx HTTP response returns `Ok(())` — the *server* is reachable; whatever the user's auth or model situation is, that's a different error category and the real call will surface it properly.

### Local-endpoint skip rule

`preflight_for_command` skips a probe entirely when the endpoint's host parses to a loopback address (`127.0.0.1`, `::1`, `localhost`). Rationale: a local server's "down" state isn't a network failure mode in the meaningful sense — the user closed it on the same machine they're sitting at, and the dialog's "your Mac is asleep / network is down" copy doesn't apply. If a local endpoint is actually down, the real call's connect error still maps to `EndpointOffline` (see "Race condition handling" below) and the user sees the same dialog — but pre-flight doesn't add a 3s ceiling to every local-only command.

### Modified: call-site error mappers

The three existing connect/timeout branches all become `EndpointOffline` constructors:

| File | Lines | Change |
|---|---|---|
| `crates/stt-providers/src/remote_provider.rs` | 236-251 | `is_connect()` / `is_timeout()` arms construct `AppError::EndpointOffline { service: RemoteStt, … }` instead of `AppError::SttProvider(String)` |
| `crates/ai-providers/src/ollama.rs` | 119-126 | "Office server unreachable" branch becomes `EndpointOffline { service: AiProvider, provider_name: "Ollama", … }` |
| `crates/ai-providers/src/lmstudio.rs` | 121 | `EndpointOffline { service: AiProvider, provider_name: "LM Studio", … }` |
| `src-tauri/src/commands/providers.rs` | 98, 234, 239 | The `test_*_connection` commands keep their existing string-return shape (they're called from the Settings "Test Connection" UI; behavior unchanged) |

`endpoint` is reconstructed from the host + port in the AppConfig at the call site. `provider_name` is a const string per call site.

### New: `src/lib/components/EndpointOfflineDialog.svelte`

A modal dialog that takes the `EndpointOffline` payload as props and renders:

```
┌─────────────────────────────────────────────────────────┐
│  Office server isn't responding                          │
│                                                          │
│  {reason_sentence}                                       │
│                                                          │
│  Common causes:                                          │
│    • The server app isn't running on your Mac            │
│    • Your Mac is asleep or has lost network              │
│    • The address in Settings has changed                 │
│                                                          │
│  Your recording is saved. You can process it once the   │
│  server is back online.                                  │
│                                                          │
│              [ Open Settings ]  [ Cancel ]  [ Retry ]    │
└─────────────────────────────────────────────────────────┘
```

`reason_sentence` is one of:

| reason | copy |
|---|---|
| `ConnectionRefused` | `The {provider_name} server at {endpoint} didn't respond.` |
| `Timeout` | `The {provider_name} server at {endpoint} took too long to respond.` |
| `DnsFailure` | `The address "{endpoint}" couldn't be found on the network.` |
| `TlsFailure` | `Couldn't establish a secure connection to {provider_name} at {endpoint}.` |

The "Common causes" and reassurance lines are constant across reasons.

**Buttons:**
- **Retry** (primary, default focus) — re-runs the original Tauri command via a `retry: () => Promise<void>` callback passed in by the caller. If it succeeds, dialog closes. If it fails with `EndpointOffline` again, the dialog re-renders in place (no flash, no double-modal).
- **Open Settings** — emits a navigation event; the host app routes to Settings → Models when `service === "AiProvider"`, Settings → Audio when `service === "RemoteStt"`.
- **Cancel** — closes the dialog, no further action.

**Accessibility:** matches the recent `ExportDialog.svelte` a11y pattern (commit `5596608`): proper aria roles, focus trap, Escape closes, primary-action default focus, backdrop click closes.

### New: `src/lib/stores/endpointOffline.ts`

A small Svelte store that holds at most one open dialog at a time. `EndpointOfflineDialog` subscribes and renders when populated. The store exposes an async API the helper uses to await the user's choice:

```ts
type EndpointOfflineDecision = 'retry' | 'cancel' | 'opened_settings';

interface EndpointOfflineStore {
  /** Opens the dialog with the given payload, resolves when the user picks an action. */
  openAndWait(payload: EndpointOfflinePayload): Promise<EndpointOfflineDecision>;
  /** Imperative close (used if a navigation cancels the in-flight prompt). */
  close(): void;
}
```

The helper that wraps `invoke`:

```ts
export async function invokeWithOfflineHandling<T>(
  cmd: string,
  args: Record<string, unknown>,
): Promise<T>;
```

**Contract:**

```
loop:
  result = invoke(cmd, args)
  if result resolves           → return result
  if result rejects with EndpointOffline:
      decision = endpointOfflineStore.openAndWait(payload)
      decision === 'retry'           → continue loop
      decision === 'cancel'          → throw OfflineCancelled
      decision === 'opened_settings' → throw OfflineCancelled
  if result rejects with any other error → throw verbatim
```

`OfflineCancelled` is a small sentinel class (e.g. `class OfflineCancelled extends Error`) the helper exports. It signals "the user dismissed the offline dialog; flow should halt silently — no toast, no extra error." Callers that want to react to it can `instanceof`-check; callers that don't care can let it flow into their existing `.catch()` where it should be filtered out before showing a generic error UI.

Caller migration pattern:

```ts
import { invokeWithOfflineHandling, OfflineCancelled } from '$lib/api/invokeWithOfflineHandling';

try {
  const soap = await invokeWithOfflineHandling<string>('generate_soap', { recordingId });
  // …success path. May be the first attempt, or a retry from inside the dialog.
} catch (err) {
  if (err instanceof OfflineCancelled) return; // user dismissed the dialog
  // existing toast/error UI path for genuine errors
}
```

The migration is purely additive over existing `.catch()` blocks: replace `invoke` with `invokeWithOfflineHandling`, add the `OfflineCancelled` early-return, leave the rest. No call site loses error handling it had before. **Retry-from-dialog correctly resumes the original `await`**, so on successful retry the success-path UI runs as if the failure never happened (matching the Section 4 dialog UX commitment).

## Control flow

### Happy path

1. User clicks **Generate SOAP**.
2. Frontend calls `invokeWithOfflineHandling('generate_soap', { recordingId })`.
3. Backend `generate_soap` command runs `preflight_for_command(GenerateSoap, &settings)`.
4. Pre-flight probes `http://192.168.1.10:11434/api/tags` (or `:1234/v1/models` for LM Studio). Returns 200 in ~80ms.
5. Real SOAP call proceeds as today.
6. Frontend resolves the promise; existing success UI runs.

### Pre-flight fails

1–2. As above.
3. Pre-flight probe times out at 3s, or returns `ConnectionRefused`.
4. Backend returns `AppError::EndpointOffline { service: AiProvider, endpoint: "http://192.168.1.10:11434", reason: ConnectionRefused, provider_name: "Ollama" }` *without* invoking the real `provider.complete()`. No audio uploaded, no model load attempted.
5. Frontend's `invokeWithOfflineHandling` catches the rejection, sees `kind === 'EndpointOffline'`, calls `endpointOfflineStore.openAndWait(payload)` and *awaits* the user's decision. The original caller's `await` is still pending.
6. Dialog renders. User clicks **Retry**.
7. `openAndWait` resolves with `'retry'`; the helper's loop re-invokes `generate_soap`. Pre-flight runs again.
   - If the user fixed the server: pre-flight succeeds, real call runs, helper returns the SOAP text, original `await` resumes with success, success-path UI renders.
   - If not: `EndpointOffline` returns again, helper re-opens (or keeps open) the dialog with the fresh payload, awaits another decision. No flash, no double-modal.
8. Alternative: user clicks **Cancel** or **Open Settings**. `openAndWait` resolves with `'cancel'` / `'opened_settings'`; helper throws `OfflineCancelled`; caller's `.catch()` early-returns.

### Race: pre-flight succeeds, real call fails

1–4. Pre-flight passes.
5. Server crashes / network drops in the ~80ms window before the real `complete()` call lands.
6. `complete()` returns a reqwest error with `is_connect()`.
7. The updated call-site mapper (in `ollama.rs` / `lmstudio.rs` / `remote_provider.rs`) constructs `AppError::EndpointOffline` with the same field shape.
8. Frontend dialog appears, identical to the pre-flight-failure flow. User experience is consistent regardless of which side caught it.

### Local-endpoint skip

1–2. As above; settings have `ollama_host = "localhost"`.
3. `preflight_for_command` inspects the resolved host, sees loopback, returns `Ok(())` immediately (no probe).
4. Real call runs. If local Ollama is up, success. If not, the call-site mapper produces `EndpointOffline { provider_name: "Ollama", endpoint: "http://localhost:11434", … }` and the same dialog appears.

## Settings linkage

The dialog's **Open Settings** button routes the user to the existing Settings pane responsible for the failing service:

| `service` | Destination | Existing file |
|---|---|---|
| `AiProvider` | Settings → Models | `src/lib/components/settings/Models.svelte` |
| `RemoteStt` | Settings → Audio | `src/lib/components/settings/Audio.svelte` |

No new fields, no new test buttons — both panes already have host/port inputs and a "Test Connection" button (`Models.svelte:202-218`, `Audio.svelte:329-356`). The user verifies/fixes the address there and can re-attempt from the recording view.

## Logging

Per CLAUDE.md ("No PHI in logs"):

- `tracing::warn!` on every pre-flight failure with structured fields: `service`, `provider_name`, `endpoint` (host:port only — no auth tokens), `reason`. No transcript content, no patient data, no SOAP body.
- `tracing::debug!` on pre-flight success (host:port + elapsed ms). Useful for diagnosing slow LAN.
- The call-site mappers already log at error level today; they keep their existing logging shape, just with the new error variant.

The `endpoint` value never contains PHI today (it's host + port from settings), but the spec calls this out so a future change that, say, started embedding query params from user input would have to revisit logging.

## Testing strategy

### Backend (Rust)

Inside `crates/core/src/preflight.rs`:

- `probe_endpoint_classifies_connection_refused` — `wiremock` server refuses connections; assert `EndpointOffline { reason: ConnectionRefused }`.
- `probe_endpoint_classifies_timeout` — `wiremock` hangs past 3s; assert `Timeout`.
- `probe_endpoint_returns_ok_on_2xx` — `wiremock` returns 200; assert `Ok(())`.
- `probe_endpoint_returns_ok_on_5xx` — `wiremock` returns 500; assert `Ok(())` (server reachable; not our concern).
- `probe_endpoint_classifies_dns_failure` — point at `http://invalid.local.test:11434`; assert `DnsFailure`.
- `preflight_skips_local_endpoint` — settings point at `127.0.0.1`; assert `Ok(())` without any HTTP attempt (use a mock probe fn or test that no request was made — implementation-specific).

Per-command integration tests under `src-tauri/tests/` (or as `#[cfg(test)]` blocks):

- `generate_soap_returns_endpoint_offline_when_provider_unreachable` — inject settings pointing at an unreachable host, run `generate_soap`, assert `Err(AppError::EndpointOffline { service: AiProvider, provider_name: "Ollama", … })` and that no `provider.complete()` call was made (mock or spy).
- `transcribe_recording_returns_endpoint_offline_when_stt_unreachable` — same shape for STT.
- One test per remaining command (`generate_referral`, `generate_letter`, `generate_synopsis`, `chat`) — they share infrastructure, so the test bodies are nearly identical.
- `race_condition_real_call_failure_produces_same_variant` — pre-flight succeeds (mocked), real call errors with `is_connect()`, assert the call-site mapper produces `AppError::EndpointOffline` with the same fields.

In `crates/core/src/error.rs`:

- `endpoint_offline_serializes_with_expected_fields` — `serde_json::to_value` on `AppError::EndpointOffline { … }`; assert the JSON has `kind`, `service`, `endpoint`, `reason`, `provider_name`, `message`.

### Frontend (Vitest + Svelte Testing Library)

In `src/lib/components/EndpointOfflineDialog.test.ts`:

- One snapshot per `reason` (4 reasons × 2 services = 8 snapshots) of the rendered dialog body — locks the copy so a change requires deliberate review.
- `retry_button_calls_retry_callback` — fires Retry, asserts the prop callback was invoked.
- `cancel_button_closes_dialog` — fires Cancel, asserts the store close action ran.
- `open_settings_navigates_to_models_for_ai_provider` / `…to_audio_for_remote_stt` — fires Open Settings, asserts the correct navigation event.
- `escape_key_closes_dialog`, `backdrop_click_closes_dialog`, `focus_traps_inside_dialog`, `retry_has_default_focus` — match the a11y patterns from `ExportDialog.svelte`.

In `src/lib/stores/endpointOffline.test.ts`:

- `openAndWait_resolves_with_retry_when_retry_clicked` — call `openAndWait(payload)`, simulate Retry click, assert the promise resolves with `'retry'`.
- `openAndWait_resolves_with_cancel_when_cancel_clicked` — same shape for Cancel.
- `openAndWait_resolves_with_opened_settings_when_settings_clicked` — same for Open Settings.
- `concurrent_open_replaces_payload` — call `openAndWait` while one is already pending (e.g. a retry failed and the helper re-opens); assert the older pending promise is settled appropriately (decision: it resolves with the new decision, so the helper's outer loop continues correctly — implementation detail confirmed during writing-plans).

In `src/lib/api/invokeWithOfflineHandling.test.ts`:

- `resolves_normally_on_first_attempt_success` — mock `invoke` to resolve with a value; helper returns it; store is never opened.
- `passes_through_non_offline_errors_unchanged` — mock to reject with `{kind: 'AiProvider', message: '...'}`; helper rejects with the same error; store is never opened.
- `loops_on_retry_and_resolves_on_eventual_success` — mock `invoke` to reject with `EndpointOffline` the first call and resolve the second; simulate Retry; assert helper resolves with the second result and `invoke` was called twice.
- `loops_on_retry_and_re_opens_dialog_on_repeated_failure` — mock `invoke` to reject with `EndpointOffline` twice; simulate Retry then Cancel; assert helper rejects with `OfflineCancelled` and `invoke` was called twice and the store was opened twice.
- `throws_OfflineCancelled_on_cancel` — mock to reject with `EndpointOffline`; simulate Cancel; assert helper rejects with an `OfflineCancelled` instance.
- `throws_OfflineCancelled_on_opened_settings` — same for Open Settings.

### Manual QA checklist

(Will be embedded in the implementation plan, listed here for completeness.)

1. Stop Ollama on the Mac. From Windows, click **Generate SOAP** on an existing recording.
   - **Expected:** dialog appears within ~3s with title "Office server isn't responding," body naming "Ollama" and the configured endpoint, "Common causes" list, "Your recording is saved." reassurance, and three buttons. Pre-flight should fire before any visible "generating…" state.
2. Click **Retry** without restarting Ollama → dialog re-opens identically.
3. Restart Ollama → click **Retry** → dialog closes, SOAP generation proceeds normally.
4. Click **Open Settings** → app navigates to Settings → Models.
5. Stop the Whisper STT server. Click **Transcribe** → dialog appears naming "Whisper STT" and the STT endpoint.
6. Click **Open Settings** from the STT dialog → app navigates to Settings → Audio.
7. Switch to local Ollama (host = `localhost`). Stop local Ollama. Click **Generate SOAP** → dialog appears (skip rule means no pre-flight delay; failure surfaces via the real call's mapper; user sees the same dialog).
8. With a remote endpoint pointing at a non-routable host (e.g. `http://192.0.2.1:11434` — TEST-NET-1, guaranteed to timeout): click **Generate SOAP** → dialog appears with `reason: Timeout` copy after the 3s probe ceiling.
9. With a malformed hostname (e.g. `http://nonexistent.invalid:11434`): click **Generate SOAP** → dialog appears with `reason: DnsFailure` copy.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| The 3s pre-flight adds 3s to every failed command — perceived as slow when the user knows the server is down. | Phase 2's status pill removes the surprise (user sees offline state before clicking). For Phase 1, 3s is the price of a clean dialog vs. a 30–60s timeout. Tunable constant if we need to revisit. |
| Pre-flight passes but real call fails (race). | Call-site mapper produces the same `EndpointOffline` variant — user sees the same dialog. Tested explicitly. |
| Reqwest version upgrade changes how `is_connect()` / `is_timeout()` classify edge cases. | Mapping tests pin the behavior. A future reqwest upgrade that reclassifies will fail the test loud. |
| `provider_name` is a const string at the call site; if we add a new AI provider, it's easy to forget to update the mapper. | Phase 1 only has two AI providers (Ollama, LM Studio) and one STT, all already covered. Adding a third would surface the requirement during code review. |
| The dialog over-fires for non-network errors that happen to look like connection errors. | Mapping is strict: only `is_connect()` / `is_timeout()` / explicit DNS / explicit TLS produce `EndpointOffline`. Everything else stays in the existing `AiProvider(String)` / `SttProvider(String)` paths. |
| Frontend missing the `kind === 'EndpointOffline'` branch at a new invoke call site silently regresses to the old toast behavior. | `invokeWithOfflineHandling` helper centralises the branch; new call sites should use it. A lint rule or grep-check in CI is a possible Phase-2 addition. |
| A user with auth-protected endpoints sees a probe that doesn't include their bearer token and gets a 401, but we treat that as `Ok(())`. | Correct behavior — 401 means the server is reachable. The real call sends auth and surfaces auth failures via the existing `AiProvider` / `SttProvider` paths. |

## Acceptance criteria

1. New `AppError::EndpointOffline` variant exists in `crates/core/src/error.rs`, serializes to the JSON shape documented above, and has unit tests covering serialization.
2. `crates/core/src/preflight.rs` exports `preflight_for_command` and `probe_endpoint` with the signatures above, and the test suite covers all four `OfflineReason` classifications plus the local-endpoint skip rule.
3. Each of these Tauri commands invokes pre-flight at the top and returns `EndpointOffline` on failure without performing the underlying work: `generate_soap`, `generate_referral`, `generate_letter`, `generate_synopsis`, `chat`, `transcribe_recording` (when STT is remote).
4. The three call-site connect/timeout mappers (`crates/stt-providers/src/remote_provider.rs`, `crates/ai-providers/src/ollama.rs`, `crates/ai-providers/src/lmstudio.rs`) produce `EndpointOffline` instead of string errors. The old strings (`"Cannot reach Whisper server at …"`, `"Office server unreachable on LAN or Tailscale (…)"`, `"Connection refused — is … running at …"`) no longer appear in user-facing surfaces (greppable as a regression guard).
5. `EndpointOfflineDialog.svelte` renders the documented copy per `reason`, supports Retry / Open Settings / Cancel, traps focus, closes on Escape / backdrop, and gives Retry default focus.
6. `endpointOfflineStore` (with `openAndWait`), `invokeWithOfflineHandling`, and `OfflineCancelled` exist; all generation/transcription invoke call sites that today land in a generic catch handler are migrated to use the helper and filter out `OfflineCancelled`. Successful Retry-from-dialog resumes the original action's success-path UI (verified by manual QA step 3).
7. Manual QA checklist (above) passes on a real Windows-client / Mac-server pair.
8. No PHI appears in `tracing::*` output produced by the new code paths (verified by reading the new log call sites; covered implicitly by code review per CLAUDE.md).
9. `cargo test --workspace --lib` and `npx vitest run` are both green.
10. `npm run check` passes (svelte-check on the new component and store).
