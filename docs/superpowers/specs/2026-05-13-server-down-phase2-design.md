# Server-Down Error Messages (Phase 2) — Design

**Status:** draft 2026-05-13
**Author:** brainstorm session, 2026-05-13
**Scope:** Phase 2 of the server-down error-messaging effort. Phase 1 (v0.10.57, shipped 2026-05-13) added structured `EndpointOffline` errors + plain-language dialog + pre-flight at processing time. Phase 2 adds *ambient awareness* so the clinician notices the office server is down *before* clicking Process: a status-bar pill that polls the configured AI/STT endpoints, plus an inline banner above the record button when the server is offline.

## Problem

Phase 1 closed the worst gap: cryptic "Connection refused — is Ollama running at X:Y?" errors became a friendly dialog with retry. But the clinician still has to *try to do something* before learning the server is offline — they record a consultation, click Generate SOAP, wait ~3 seconds for the pre-flight probe, see the dialog, restart the server, retry. That works but it interrupts the flow at the worst moment (after the recording is captured).

A second class of users in the original incident: **the clinician forgot the server was off and started recording anyway**. A passive, always-visible status signal would have prevented the recording-in-the-dark scenario entirely.

Phase 2 provides that ambient signal in two surfaces:

1. **Status-bar pill ("AI")** — small badge in the existing app footer status bar, present across all views, polling the configured AI/STT endpoints every 10 s.
2. **Inline banner above the record button** — when the pill is `partial` or `offline`, a non-blocking banner appears in `RecordingHeader.svelte` explaining the recording will save locally but processing will fail.

Phase 2 reuses Phase 1's existing Tauri commands (`test_ollama_connection`, `test_lmstudio_connection`, `test_stt_remote_connection`) for probing — no new backend code is required. All work is frontend.

## Non-goals

- **Backend changes.** Phase 2 is frontend-only; reuses Phase 1's pre-flight infrastructure (`probe_endpoint`, `classify_reqwest_error`, `preflight_for_command`) and the Settings test-connection Tauri commands.
- **Telemetry.** Probes are local-network only; no data leaves the device.
- **A new error variant or dialog.** The pill is purely informational; the dialog from Phase 1 still fires when the user actually clicks Process.
- **Renaming the existing `StatusBadge` ("Office server" — sharing pairing).** Both pills coexist with distinct labels (`Office server` for the sharing/auth-proxy reachability, `AI` for the model/STT health).
- **Per-service pills.** The pill is a unified label covering both AI and Remote STT. Per-service detail surfaces in the hover tooltip and the click-target.
- **Manual "Refresh now" UI control.** The 10 s tick plus the on-focus immediate probe is sufficient. Click on the pill goes to Settings (see below); clicking the existing "Test Connection" buttons in Settings already provides a manual refresh.
- **Adaptive cadence.** The 10 s cadence is hardcoded for Phase 2. If the user community asks for faster/slower polling later, a single constant changes.
- **Mobile / responsive layout work.** This app is desktop-only; the existing status-bar layout is fine.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│  src/lib/stores/endpointHealth.ts                                    │
│    (single source of truth; one polling timer per app instance)      │
│                                                                       │
│  state shape:                                                         │
│    {                                                                  │
│      ai: 'online' | 'offline' | 'skipped',                            │
│      stt: 'online' | 'offline' | 'skipped',                           │
│      lastCheckedAt: number | null,    // epoch ms                     │
│      overall: 'online' | 'partial' | 'offline' | 'hidden'             │
│    }                                                                  │
│                                                                       │
│  probes used:                                                         │
│    test_ollama_connection(host, port)                                 │
│    test_lmstudio_connection(host, port)                               │
│    test_stt_remote_connection(host, port, api_key)                    │
│                                                                       │
│  triggers:                                                            │
│    - first subscriber  → start setInterval(10s) + immediate probe     │
│    - last unsubscriber → clear interval                               │
│    - document.visibilitychange to 'hidden' → clear interval           │
│    - document.visibilitychange to 'visible' → immediate probe         │
│    - $settings change (ai_provider/host/port/stt_remote_*) →          │
│        immediate re-probe (next 10s tick still fires normally)        │
└─────────────────────────────────────────────────────────────────────┘
                    ↑                        ↑
                    │                        │
                    │ subscribe              │ subscribe
                    │                        │
┌───────────────────┴─────────────┐  ┌──────┴──────────────────────────┐
│  src/lib/components/             │  │  src/lib/components/            │
│  EndpointHealthPill.svelte       │  │  OfflineRecordBanner.svelte     │
│    (new — mounted in StatusBar)  │  │    (new — mounted in            │
│                                  │  │     RecordingHeader above       │
│  - shows dot + "AI" label        │  │     .controls-row)              │
│  - hidden when overall='hidden'  │  │                                 │
│  - tooltip shows per-service     │  │  - shown when overall =         │
│    detail + lastChecked          │  │    'partial' or 'offline'       │
│  - click → settingsNav.navigateTo│  │  - reuses .error-banner CSS     │
│      ('models' or 'audio')       │  │  - copy varies by overall       │
└──────────────────────────────────┘  │  - "Open Settings" button       │
                                       │    fires the same nav as       │
                                       │    the pill click              │
                                       └─────────────────────────────────┘
```

Both surfaces are pure consumers of `endpointHealth`. The store does all polling, dedupe, and lifecycle management. Multiple subscribers share one interval (reference-counted).

## Components

### New: `src/lib/stores/endpointHealth.ts`

```ts
import { writable, get, derived, type Readable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { settings } from './settings';

const POLL_INTERVAL_MS = 10_000;

type ServiceStatus = 'online' | 'offline' | 'skipped';
type Overall = 'online' | 'partial' | 'offline' | 'hidden';

interface EndpointHealthState {
  ai: ServiceStatus;
  stt: ServiceStatus;
  lastCheckedAt: number | null;
  overall: Overall;
}

interface EndpointHealthStore extends Readable<EndpointHealthState> {
  /** Force an immediate probe (used by settings-change and visibilitychange triggers). */
  probeNow(): Promise<void>;
}

function isLoopbackHost(host: string): boolean {
  if (!host) return true;
  const h = host.trim().toLowerCase();
  const stripped = h.replace(/^\[|\]$/g, '');
  if (stripped === 'localhost' || stripped === '::1') return true;
  // Match 127/8
  return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(stripped);
}

function computeOverall(ai: ServiceStatus, stt: ServiceStatus): Overall {
  if (ai === 'skipped' && stt === 'skipped') return 'hidden';
  const states = [ai, stt].filter((s) => s !== 'skipped');
  if (states.every((s) => s === 'online')) return 'online';
  if (states.every((s) => s === 'offline')) return 'offline';
  return 'partial';
}

function createEndpointHealthStore(): EndpointHealthStore { … }
export const endpointHealth = createEndpointHealthStore();
```

**Implementation notes** (binding details that the writer of the plan will fill in):

- The store uses Svelte's `writable` start/stop callback (`writable(initial, (set) => { ...; return () => {} })`) to register the polling lifecycle on first subscribe and tear down on last unsubscribe.
- The polling job calls `probeAll(cfg)` which reads `get(settings)`, builds the probe set per `ai_provider` + `stt_remote_host`, runs them in parallel via `Promise.all`, and updates the writable.
- `probeNow()` is exposed for callers (the visibility-change handler, the settings-change handler) to bypass the timer and probe immediately.
- A second internal subscription on `settings` triggers `probeNow()` whenever `ai_provider`, `lmstudio_host`, `lmstudio_port`, `ollama_host`, `ollama_port`, `stt_remote_host`, `stt_remote_port`, or `stt_mode` changes. To avoid an infinite loop, the previous tracked subset is held in a closure and only `probeNow()` if the relevant fields differ.
- `document.visibilitychange` is attached on first subscribe and removed on last unsubscribe.

### New: `src/lib/components/EndpointHealthPill.svelte`

A small pill that subscribes to `endpointHealth`. Renders nothing when overall is `'hidden'`. Otherwise renders a colored dot + the text "AI" with a `title` attribute carrying the per-service detail. Click handler navigates to Settings via `settingsNav.navigateTo(...)`. Styling reuses the existing `.sharing-badge` pattern from `StatusBar.svelte:138-169` for visual consistency.

States (CSS class on the badge element):

| `overall` | class | dot color | aria-label |
|---|---|---|---|
| `online` | `endpoint-pill ok` | green (`rgba(22, 163, 74, 0.85)`) | "AI services online" |
| `partial` | `endpoint-pill warn` | amber (`rgba(217, 119, 6, 0.85)`) | "AI services partially offline" |
| `offline` | `endpoint-pill error` | red (`rgba(220, 38, 38, 0.85)`) | "AI services offline" |
| `hidden` | (component returns null) | — | — |

Tooltip text (`title` attribute) computed from store state:

| state | tooltip |
|---|---|
| `online`, both probed | "Ollama online, Whisper STT online — last checked 4s ago" |
| `online`, only AI probed | "Ollama online — last checked 4s ago" (STT skipped → not mentioned) |
| `partial`, AI offline | "Ollama offline, Whisper STT online — last checked 4s ago" |
| `partial`, STT offline | "Ollama online, Whisper STT offline — last checked 4s ago" |
| `offline` | "Ollama offline, Whisper STT offline — last checked 4s ago" |

(Provider name comes from `$settings.ai_provider`: `'ollama'` → `Ollama`; `'lmstudio'` → `LM Studio`. STT name is `Whisper STT`.)

Click routing:

| state | click target |
|---|---|
| `online` | no-op |
| `partial`, AI offline only | `settingsNav.navigateTo('models')` + `settingsOpen = true` |
| `partial`, STT offline only | `settingsNav.navigateTo('audio')` + `settingsOpen = true` |
| `offline` (both) | `settingsNav.navigateTo('models')` + `settingsOpen = true` (AI is the more common case) |

The `settingsOpen` flag lives on `App.svelte` and is set by emitting an event the parent listens for, or by writing to a small new boolean store. To keep this self-contained, the pill emits an `openSettings` callback prop that `StatusBar.svelte` forwards to `App.svelte` (the same pattern the Phase 1 `EndpointOfflineDialog` uses).

### Mount: `src/lib/components/StatusBar.svelte`

Add `<EndpointHealthPill onopenSettings={...} />` inside the badge area, adjacent to the existing `.sharing-badge` pills. The new pill renders nothing when `overall === 'hidden'`, so users without remote endpoints see no visual change.

### New: `src/lib/components/OfflineRecordBanner.svelte`

A small banner using the existing `.error-banner` CSS in `RecordingHeader.svelte`. Subscribes to `endpointHealth`. Renders only when overall is `'partial'` or `'offline'`. Copy:

| state | text |
|---|---|
| `offline` | "Office server offline — your recording will save locally, but transcription and SOAP will fail until it's back online." |
| `partial`, AI offline only | "AI offline — your recording will save locally, but SOAP generation will fail." |
| `partial`, STT offline only | "Whisper STT offline — your recording will save locally, but transcription will fail." |

(For `partial`, both messages name the specific service that's down. The `offline` state uses the "Office server" framing because both services down typically means the whole host is down — matches the user's mental model.)

A right-aligned "Open Settings" button on the banner uses the same `onopenSettings` callback the pill uses.

### Mount: `src/lib/components/RecordingHeader.svelte`

Place `<OfflineRecordBanner />` immediately above `.controls-row` (around the existing `.error-banner` position at lines 39-44). The banner styling already exists in this file (`.error-banner` block at lines 97-128); the new banner either reuses those styles by sharing a class name or copies them.

## Data flow

### Happy path

1. App boots. Sharing/status surfaces (StatusBar, RecordingHeader) mount. The new `EndpointHealthPill` is rendered in `StatusBar`.
2. The pill subscribes to `endpointHealth`. First subscriber triggers:
   - Initial `probeNow()` call.
   - `setInterval(probeAll, 10_000)` starts.
   - `document.addEventListener('visibilitychange', …)` attached.
3. First probe reads `get(settings)`, sees `ai_provider = 'ollama'`, `ollama_host = '192.168.1.10'`, `ollama_port = 11434`. Builds an AI probe via `invoke('test_ollama_connection', { host: '192.168.1.10', port: 11434 })`. Reads `stt_remote_host = ''` → skip STT probe.
4. Probe succeeds → store updates to `{ ai: 'online', stt: 'skipped', overall: 'online', lastCheckedAt: now }`.
5. Pill re-renders: green dot, "AI" label, tooltip "Ollama online — last checked 0s ago".
6. Banner is hidden because overall is `'online'`.
7. Every 10 s, the probe re-runs and `lastCheckedAt` advances. The tooltip's "Xs ago" text recomputes on each render (using a derived `lastCheckedDescription` value or just a periodic re-render via the next probe).

### Server goes offline mid-session

1. The clinician's Mac sleeps. Ollama becomes unreachable.
2. Next 10 s probe rejects (the Tauri command returns `AppError::AiProvider("Connection refused — is Ollama running at 192.168.1.10:11434?")`).
3. Store updates to `{ ai: 'offline', stt: 'skipped', overall: 'offline', lastCheckedAt: now }`.
4. Pill turns red. Tooltip says "Ollama offline — last checked 0s ago".
5. `OfflineRecordBanner` renders (because the clinician is on the Record tab). Copy: "Office server offline — your recording will save locally, but transcription and SOAP will fail until it's back online."
6. Clinician notices the red pill / banner and either restarts the Mac or knows to record offline and process later. **The pre-recording mistake is prevented.**

### Settings change

1. The clinician changes `ai_provider` from `'ollama'` to `'lmstudio'` in Settings.
2. `endpointHealth`'s internal subscription on `settings` fires; the new value differs from the cached subset; `probeNow()` is called immediately.
3. The next probe is against LM Studio at `lmstudio_host:lmstudio_port`. Ollama is no longer probed.
4. Store updates within < 200ms (assuming LAN response).

### Visibility / background

1. Clinician switches to another app. `document.visibilityState` becomes `'hidden'`. The store's listener clears the `setInterval` — no probes fire while backgrounded.
2. Clinician returns. `visibilityState` becomes `'visible'`. The store's listener calls `probeNow()` for an immediate fresh status, then `setInterval(10_000)` resumes.

### No remote endpoints configured (fully local)

1. `ai_provider = 'lmstudio'` with `lmstudio_host = 'localhost'`. `stt_remote_host = ''`.
2. First probe builds two probe specs and both are skipped by `isLoopbackHost`.
3. Store updates to `{ ai: 'skipped', stt: 'skipped', overall: 'hidden' }`.
4. Pill renders nothing. Banner renders nothing. Polling still runs at 10 s (cheap: both probes return `'skipped'` synchronously, no HTTP traffic).
5. If the user later switches to a remote host, the settings-change trigger immediately re-probes and the pill appears.

## Settings access

The store imports the existing `settings` writable from `src/lib/stores/settings.ts` (which already has `ai_provider`, `lmstudio_host`, `lmstudio_port`, `ollama_host`, `ollama_port`, `stt_remote_host`, `stt_remote_port`, `stt_remote_api_key`). No new settings fields are added.

The store does **not** modify settings. It only reads, never writes.

## Click routing

The pill and banner both surface an `openSettings` action. Both reuse the Phase 1 `settingsNav` store from `src/lib/stores/settingsNav.ts`:

```ts
import { settingsNav } from '$lib/stores/settingsNav'; // (relative path in practice)

function openSettings(target: 'models' | 'audio') {
  // The same pattern Phase 1's App.svelte uses for the EndpointOfflineDialog:
  // set the requested section and open the Settings dialog.
  settingsNav.navigateTo(target);
  // The parent component (App.svelte) flips settingsOpen = true.
}
```

The pill emits `onopenSettings('models' | 'audio')`. StatusBar.svelte threads the callback through to App.svelte where `settingsOpen = true` and the `settingsNav` write happen in parallel — same wiring as the Phase 1 dialog.

## Logging

Per CLAUDE.md (no PHI in logs), the store emits no logs at all. The Tauri commands it calls (`test_*_connection`) already log appropriately on the backend (host:port only). The frontend store never `console.log`s user data.

If a probe fails, the error is captured into the store's state as `'offline'` — not logged.

## Testing strategy

### Store unit tests

`src/lib/stores/endpointHealth.test.ts`:

- `initial state is hidden when no subscribers` — `get(endpointHealth)` immediately after import returns `{ ai: 'skipped', stt: 'skipped', overall: 'hidden', lastCheckedAt: null }`.
- `probes remote ollama when subscribed` — mock `invoke`, mock `settings` with `ai_provider='ollama'`, `ollama_host='192.168.1.10'`, `ollama_port=11434`. Subscribe; assert `invoke` was called with `test_ollama_connection`, `{ host: '192.168.1.10', port: 11434 }`.
- `marks ai online on probe success` — mock `invoke` resolves; advance fake timers; assert state shows `ai='online'`, `overall='online'`.
- `marks ai offline on probe rejection` — mock `invoke` rejects; assert state shows `ai='offline'`.
- `marks stt offline on probe rejection while ai online` — mixed mocks; assert `overall='partial'`.
- `skips loopback ai_provider` — settings have `ollama_host='127.0.0.1'`; assert `invoke` was NOT called for that probe; state shows `ai='skipped'`.
- `skips empty stt_remote_host` — settings have `stt_remote_host=''`; assert no STT probe and `stt='skipped'`.
- `settings change triggers immediate re-probe` — change `ai_provider` in settings; advance one microtask; assert `invoke` was called with the new probe args.
- `visibility change to hidden clears interval` — set `document.visibilityState='hidden'`, dispatch `visibilitychange`; advance 30s; assert `invoke` was NOT called again.
- `visibility change to visible triggers immediate probe and resumes interval` — set hidden, then visible; assert `invoke` was called immediately on `visible` and again 10s later.
- `last unsubscriber clears the interval` — subscribe; unsubscribe; advance 30s; assert no further `invoke` calls.

The standard `vi.useFakeTimers()` pattern handles the 10 s tick. Mock `document.visibilityState` and `addEventListener('visibilitychange')` via `vi.spyOn`.

### Component tests

Following Phase 1's precedent, component DOM tests are deferred to manual QA (no `@testing-library/svelte` installed). The pill and banner are thin store-consumers; the store tests give us the meaningful regression net.

If we *do* want lightweight verification, a snapshot-style test on the rendered HTML for each `overall` state is possible by running the component in isolation, but the cost/benefit is marginal — manual QA covers the visual cases.

### Manual QA

1. Set `ai_provider = ollama`, `ollama_host = 192.168.1.10` (or a real reachable Ollama). Start the app. Within 10s the pill appears green in the status bar with tooltip "Ollama online — last checked Xs ago".
2. Stop Ollama. Within 10s the pill turns red. Tooltip names Ollama offline. Open the Record tab — the banner appears above the controls row with the "Office server offline" copy.
3. Restart Ollama. Within 10s the pill turns green again. Banner disappears.
4. Configure remote STT at a reachable host (or a wiremock). Pill stays green showing both services in tooltip.
5. Stop just the STT server. Pill turns amber; tooltip says "Ollama online, Whisper STT offline". Banner shows the AI-only-online copy.
6. Click the pill while red → app navigates to Settings → Models.
7. Click the banner's "Open Settings" → same destination.
8. Switch `ai_provider` to LM Studio (still remote, reachable). Within ~200ms the pill re-probes against the new endpoint (verify via devtools network panel).
9. Switch to fully-local (`ai_provider = lmstudio` with `localhost`, `stt_remote_host = ''`). The pill disappears within 10s. Banner does not appear.
10. Minimize the window. Devtools network panel should show no probe traffic. Restore the window — one immediate probe fires, then the 10s cadence resumes.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| 10 s polling adds visible network noise on a metered LAN/Tailscale | Probes pause when the window is hidden; each probe is a tiny HTTP GET (~80 B request, ~100 B–1 kB response). Negligible. The cadence is a single constant for future tuning. |
| The "AI" label is ambiguous to a clinician who doesn't think of Ollama as "AI" | The tooltip and the click destination (Settings → Models) reinforce the meaning. The dialog from Phase 1 uses the provider name verbatim ("Ollama", "LM Studio"); the pill uses the broader category for footer brevity. |
| The pill and the existing `StatusBadge` ("Office server" for sharing) coexist and could confuse | They communicate different things — the existing badge is about the auth-proxy LAN/Tailscale reachability; the new pill is about the model/STT services themselves. Each has a distinct label. A future iteration may merge them, but Phase 2 keeps them separate. |
| Probe failures that aren't connectivity (server returns 5xx, auth fails) are treated as "offline" in the pill | Acceptable. The pill is a binary signal; a 503 from Ollama still means "you can't use it right now". Phase 1's dialog handles the granular distinction at processing time. |
| Settings-change reactivity loops if `probeNow()` writes back to settings | The store never writes to `settings`; it only reads. No loop possible. |
| Multiple browser windows (Tauri webviews) would each run independent timers | This app is single-window. Not applicable. |
| The pill / banner add visual clutter when everything is fine | Pill is small (~6 ch wide) and uses the existing badge styling. Banner only shows when `partial`/`offline`. Fully-local users see nothing. |

## Acceptance criteria

1. `src/lib/stores/endpointHealth.ts` exports a Svelte store with the documented `EndpointHealthState` shape and a `probeNow()` method.
2. The store starts polling on first subscribe at 10 s intervals; clears on last unsubscribe.
3. The store pauses polling on `document.visibilitychange` to `hidden`; resumes with an immediate probe on `visible`.
4. The store calls `test_ollama_connection`, `test_lmstudio_connection`, or `test_stt_remote_connection` per the active `ai_provider` and `stt_remote_host`. Loopback / empty hosts are skipped.
5. Settings changes affecting probed fields trigger an immediate re-probe.
6. `EndpointHealthPill.svelte` renders only when `overall !== 'hidden'`. Color and tooltip match the state table above. Click triggers the matching `settingsNav.navigateTo(...)` and opens Settings.
7. `OfflineRecordBanner.svelte` renders only when `overall ∈ {'partial', 'offline'}`. Copy matches the state table.
8. Both components are mounted: pill in `StatusBar.svelte`, banner in `RecordingHeader.svelte` above the controls row.
9. No frontend logs (`console.log`) emit user data. (PHI rule from CLAUDE.md applies even though pills/banners don't normally handle PHI.)
10. `npx vitest run` and `npm run check` are green. ≥ 8 store tests cover the documented behaviors.
11. Manual QA checklist (above) passes end-to-end.
