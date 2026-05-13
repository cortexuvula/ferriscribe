# Server-Down Error Messages (Phase 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an ambient "AI" status pill to the app's footer status bar that polls the configured AI / Remote STT endpoints every 10 s, plus an inline banner above the record button that warns when the office server is unreachable. Both consume a single `endpointHealth` Svelte store that pauses while the window is hidden, re-probes immediately on settings change, and reuses Phase 1's `test_*_connection` Tauri commands so no backend work is required.

**Architecture:** One Svelte store (`endpointHealth`) is the source of truth. It owns the `setInterval`, the `visibilitychange` listener, and the `settings` subscription that triggers re-probes. Two presentational components subscribe to it: `EndpointHealthPill` (mounted in `StatusBar`) and `OfflineRecordBanner` (mounted in `RecordingHeader`). Click-to-Settings reuses the Phase 1 `settingsNav` store. The pill emits an `onopenSettings(target)` callback prop that `StatusBar` forwards up to `App.svelte` — the same wiring the Phase 1 `EndpointOfflineDialog` uses.

**Tech Stack:** Svelte 5 with runes (`$state`, `$props`, `$effect`, `onclick=`), TypeScript, Vitest (node environment, no `@testing-library/svelte`), `@tauri-apps/api/core` for `invoke`. No new dependencies. No backend changes.

**Spec:** [`docs/superpowers/specs/2026-05-13-server-down-phase2-design.md`](../specs/2026-05-13-server-down-phase2-design.md)

---

## File Structure

**New files:**
- `src/lib/stores/endpointHealth.ts` — the polling store
- `src/lib/stores/endpointHealth.test.ts` — store tests (~11 tests)
- `src/lib/components/EndpointHealthPill.svelte` — the pill component
- `src/lib/components/OfflineRecordBanner.svelte` — the banner

**Modified files:**
- `src/lib/components/StatusBar.svelte` — mount the pill alongside existing sharing badges, forward `openSettings` callback
- `src/App.svelte` — pass `onopenSettings` down to `StatusBar` (re-wires same as the existing `EndpointOfflineDialog` callback)
- `src/lib/components/RecordingHeader.svelte` — mount the banner above `.controls-row`

**Worktree:** Use `superpowers:using-git-worktrees` to create `.worktrees/server-down-phase2` from master before starting Task 1. The Phase 1 work merged into master at `f82febc`; this branch builds on that.

---

## Task 1: Create the `endpointHealth` Svelte store

**Files:**
- Create: `src/lib/stores/endpointHealth.ts`
- Create: `src/lib/stores/endpointHealth.test.ts`

**Why:** Every other component depends on this store. Build it first with comprehensive tests so the visible UI in Tasks 2–5 can layer on top of a known-correct state machine. TDD discipline: write tests, watch them fail, implement, watch them pass.

- [ ] **Step 1: Write the failing initial-state test**

Create `src/lib/stores/endpointHealth.test.ts` with the first test:

```ts
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
const invokeMock = vi.mocked(invoke);

vi.mock('../stores/settings', () => {
  const { writable } = require('svelte/store');
  return {
    settings: writable({
      ai_provider: 'lmstudio',
      lmstudio_host: '',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'local',
    }),
  };
});

// Import after mocks are set up.
import { endpointHealth } from './endpointHealth';
import { settings } from '../stores/settings';

describe('endpointHealth store', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    vi.useFakeTimers();
  });

  it('starts in hidden state before any subscriber', () => {
    const state = get(endpointHealth);
    expect(state.ai).toBe('skipped');
    expect(state.stt).toBe('skipped');
    expect(state.overall).toBe('hidden');
    expect(state.lastCheckedAt).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test; expect it to fail because `endpointHealth.ts` does not exist**

Run: `npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -20`

Expected: build error — `Cannot find module './endpointHealth'`.

- [ ] **Step 3: Create the minimal store**

Create `src/lib/stores/endpointHealth.ts`:

```ts
import { writable, get, type Readable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { settings } from './settings';

const POLL_INTERVAL_MS = 10_000;

export type ServiceStatus = 'online' | 'offline' | 'skipped';
export type Overall = 'online' | 'partial' | 'offline' | 'hidden';

export interface EndpointHealthState {
  ai: ServiceStatus;
  stt: ServiceStatus;
  lastCheckedAt: number | null;
  overall: Overall;
}

export interface EndpointHealthStore extends Readable<EndpointHealthState> {
  /** Force an immediate probe (used by settings-change and visibilitychange triggers). */
  probeNow(): Promise<void>;
}

const INITIAL: EndpointHealthState = {
  ai: 'skipped',
  stt: 'skipped',
  lastCheckedAt: null,
  overall: 'hidden',
};

function isLoopbackHost(host: string): boolean {
  if (!host) return true;
  const h = host.trim().toLowerCase();
  const stripped = h.replace(/^\[/, '').replace(/\]$/, '');
  if (stripped === 'localhost' || stripped === '::1') return true;
  return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(stripped);
}

function computeOverall(ai: ServiceStatus, stt: ServiceStatus): Overall {
  if (ai === 'skipped' && stt === 'skipped') return 'hidden';
  const states = [ai, stt].filter((s): s is 'online' | 'offline' => s !== 'skipped');
  if (states.every((s) => s === 'online')) return 'online';
  if (states.every((s) => s === 'offline')) return 'offline';
  return 'partial';
}

function createEndpointHealthStore(): EndpointHealthStore {
  const state = writable<EndpointHealthState>(INITIAL);

  async function probeAi(cfg: any): Promise<ServiceStatus> {
    const provider = cfg.ai_provider;
    if (provider === 'ollama') {
      if (isLoopbackHost(cfg.ollama_host)) return 'skipped';
      try {
        await invoke('test_ollama_connection', {
          host: cfg.ollama_host,
          port: cfg.ollama_port,
        });
        return 'online';
      } catch {
        return 'offline';
      }
    }
    if (provider === 'lmstudio') {
      if (isLoopbackHost(cfg.lmstudio_host)) return 'skipped';
      try {
        await invoke('test_lmstudio_connection', {
          host: cfg.lmstudio_host,
          port: cfg.lmstudio_port,
        });
        return 'online';
      } catch {
        return 'offline';
      }
    }
    return 'skipped';
  }

  async function probeStt(cfg: any): Promise<ServiceStatus> {
    if (cfg.stt_mode !== 'remote') return 'skipped';
    if (isLoopbackHost(cfg.stt_remote_host)) return 'skipped';
    try {
      await invoke('test_stt_remote_connection', {
        host: cfg.stt_remote_host,
        port: cfg.stt_remote_port,
        apiKey: cfg.stt_remote_api_key,
      });
      return 'online';
    } catch {
      return 'offline';
    }
  }

  async function probeAll(): Promise<void> {
    const cfg = get(settings);
    const [ai, stt] = await Promise.all([probeAi(cfg), probeStt(cfg)]);
    const overall = computeOverall(ai, stt);
    state.set({ ai, stt, lastCheckedAt: Date.now(), overall });
  }

  return {
    subscribe: state.subscribe,
    probeNow: probeAll,
  };
}

export const endpointHealth = createEndpointHealthStore();
```

- [ ] **Step 4: Run the test; expect it to pass**

Run: `npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -15`

Expected: 1 test passes.

- [ ] **Step 5: Add a probe-success test**

Append to `endpointHealth.test.ts`:

```ts
  it('probeNow marks ai as online when test_lmstudio_connection resolves', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '192.168.1.10',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'local',
    } as any);

    invokeMock.mockResolvedValueOnce('Connected — 3 models available');

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('test_lmstudio_connection', {
      host: '192.168.1.10',
      port: 1234,
    });
    const state = get(endpointHealth);
    expect(state.ai).toBe('online');
    expect(state.stt).toBe('skipped');
    expect(state.overall).toBe('online');
    expect(state.lastCheckedAt).not.toBeNull();
  });
```

Run: `npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -15`

Expected: 2 tests pass.

- [ ] **Step 6: Add a probe-failure test**

Append:

```ts
  it('probeNow marks ai as offline when probe rejects', async () => {
    settings.set({
      ai_provider: 'ollama',
      lmstudio_host: '',
      lmstudio_port: 1234,
      ollama_host: '192.168.1.10',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'local',
    } as any);

    invokeMock.mockRejectedValueOnce({
      kind: 'AiProvider',
      message: 'Connection refused — is Ollama running at 192.168.1.10:11434?',
    });

    await endpointHealth.probeNow();

    const state = get(endpointHealth);
    expect(state.ai).toBe('offline');
    expect(state.overall).toBe('offline');
  });
```

Run: `npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -15`

Expected: 3 tests pass.

- [ ] **Step 7: Add the partial-state test**

Append:

```ts
  it('overall is partial when one service is online and the other offline', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '192.168.1.10',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '192.168.1.10',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'remote',
    } as any);

    invokeMock
      .mockResolvedValueOnce('Connected') // AI probe
      .mockRejectedValueOnce({ kind: 'SttProvider', message: 'timeout' }); // STT probe

    await endpointHealth.probeNow();

    const state = get(endpointHealth);
    expect(state.ai).toBe('online');
    expect(state.stt).toBe('offline');
    expect(state.overall).toBe('partial');
  });
```

Run: `npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -15`

Expected: 4 tests pass.

- [ ] **Step 8: Add loopback-skip tests**

Append:

```ts
  it('skips loopback ai_provider host without calling invoke', async () => {
    settings.set({
      ai_provider: 'ollama',
      lmstudio_host: '',
      lmstudio_port: 1234,
      ollama_host: '127.0.0.1',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'local',
    } as any);

    await endpointHealth.probeNow();

    expect(invokeMock).not.toHaveBeenCalled();
    const state = get(endpointHealth);
    expect(state.ai).toBe('skipped');
    expect(state.stt).toBe('skipped');
    expect(state.overall).toBe('hidden');
  });

  it('skips empty stt_remote_host', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '192.168.1.10',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'remote',
    } as any);

    invokeMock.mockResolvedValueOnce('Connected');

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith('test_lmstudio_connection', expect.anything());
    const state = get(endpointHealth);
    expect(state.stt).toBe('skipped');
  });

  it('skips stt when stt_mode is local even if stt_remote_host is set', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '127.0.0.1',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '192.168.1.20',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'local',
    } as any);

    await endpointHealth.probeNow();

    expect(invokeMock).not.toHaveBeenCalled();
    const state = get(endpointHealth);
    expect(state.stt).toBe('skipped');
    expect(state.overall).toBe('hidden');
  });

  it('recognizes bracketed IPv6 loopback as loopback', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '[::1]',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'local',
    } as any);

    await endpointHealth.probeNow();

    expect(invokeMock).not.toHaveBeenCalled();
    const state = get(endpointHealth);
    expect(state.ai).toBe('skipped');
  });
```

Run: `npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -15`

Expected: 8 tests pass.

- [ ] **Step 9: Add the polling-lifecycle behaviors**

The store currently lacks: (a) start `setInterval` on first subscribe, (b) clear on last unsubscribe, (c) visibility-change pause/resume, (d) settings-change auto-reprobe. Add these now.

First, write the failing tests:

```ts
  it('starts a 10s polling interval on first subscribe', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '192.168.1.10',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'local',
    } as any);
    invokeMock.mockResolvedValue('Connected');

    const unsub = endpointHealth.subscribe(() => {});
    // First probe fires immediately on subscribe.
    await Promise.resolve();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(1);

    // Second probe after 10 s tick.
    await vi.advanceTimersByTimeAsync(10_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);

    unsub();
  });

  it('clears the interval when the last subscriber unsubscribes', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '192.168.1.10',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'local',
    } as any);
    invokeMock.mockResolvedValue('Connected');

    const unsub = endpointHealth.subscribe(() => {});
    await Promise.resolve();
    await Promise.resolve();
    const callsAfterSubscribe = invokeMock.mock.calls.length;
    unsub();

    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock).toHaveBeenCalledTimes(callsAfterSubscribe);
  });

  it('triggers an immediate re-probe when settings change', async () => {
    settings.set({
      ai_provider: 'ollama',
      lmstudio_host: '',
      lmstudio_port: 1234,
      ollama_host: '192.168.1.10',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'local',
    } as any);
    invokeMock.mockResolvedValue('Connected');

    const unsub = endpointHealth.subscribe(() => {});
    await Promise.resolve();
    await Promise.resolve();
    const callsBefore = invokeMock.mock.calls.length;

    // Change a probed field.
    settings.update((s: any) => ({ ...s, ollama_host: '192.168.1.99' }));
    await Promise.resolve();
    await Promise.resolve();

    expect(invokeMock.mock.calls.length).toBeGreaterThan(callsBefore);
    expect(invokeMock).toHaveBeenLastCalledWith('test_ollama_connection', {
      host: '192.168.1.99',
      port: 11434,
    });
    unsub();
  });
```

Run: `npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -20`

Expected: 3 new tests fail (the polling lifecycle isn't implemented yet).

- [ ] **Step 10: Implement the polling lifecycle in `endpointHealth.ts`**

Replace the `createEndpointHealthStore` body with the start/stop-callback shape that owns the interval, the `settings` subscription, and (optionally) the visibility listener:

```ts
function createEndpointHealthStore(): EndpointHealthStore {
  let timer: ReturnType<typeof setInterval> | null = null;
  let settingsUnsub: (() => void) | null = null;
  let visibilityHandler: (() => void) | null = null;
  let lastProbedKey = '';

  const state = writable<EndpointHealthState>(INITIAL, (set) => {
    // Runs on first subscribe.
    startPolling(set);
    return () => stopPolling();
  });

  function probedKey(cfg: any): string {
    return [
      cfg.ai_provider,
      cfg.lmstudio_host, cfg.lmstudio_port,
      cfg.ollama_host, cfg.ollama_port,
      cfg.stt_remote_host, cfg.stt_remote_port, cfg.stt_remote_api_key ?? '',
      cfg.stt_mode,
    ].join('|');
  }

  async function probeAll(): Promise<void> {
    const cfg = get(settings);
    lastProbedKey = probedKey(cfg);
    const [ai, stt] = await Promise.all([probeAi(cfg), probeStt(cfg)]);
    const overall = computeOverall(ai, stt);
    state.set({ ai, stt, lastCheckedAt: Date.now(), overall });
  }

  function startPolling(_set: (v: EndpointHealthState) => void): void {
    // Immediate probe (fire-and-forget; tests await via subscribe()).
    void probeAll();
    timer = setInterval(() => { void probeAll(); }, POLL_INTERVAL_MS);

    settingsUnsub = settings.subscribe((cfg) => {
      const key = probedKey(cfg);
      if (key !== lastProbedKey && lastProbedKey !== '') {
        void probeAll();
      }
    });

    if (typeof document !== 'undefined') {
      visibilityHandler = () => {
        if (document.visibilityState === 'hidden') {
          if (timer) {
            clearInterval(timer);
            timer = null;
          }
        } else {
          if (!timer) {
            void probeAll();
            timer = setInterval(() => { void probeAll(); }, POLL_INTERVAL_MS);
          }
        }
      };
      document.addEventListener('visibilitychange', visibilityHandler);
    }
  }

  function stopPolling(): void {
    if (timer) { clearInterval(timer); timer = null; }
    if (settingsUnsub) { settingsUnsub(); settingsUnsub = null; }
    if (visibilityHandler && typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', visibilityHandler);
      visibilityHandler = null;
    }
    lastProbedKey = '';
  }

  return {
    subscribe: state.subscribe,
    probeNow: probeAll,
  };
}
```

(The other functions — `probeAi`, `probeStt`, `isLoopbackHost`, `computeOverall` — remain unchanged.)

- [ ] **Step 11: Run all 11 tests**

Run: `npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -20`

Expected: 11 tests pass. If the polling-lifecycle tests still fail because `probeAll()` runs as a fire-and-forget Promise, the test's `await Promise.resolve(); await Promise.resolve();` lines drain the microtask queue. If the first probe isn't observed, add a third `await Promise.resolve()` — the chain length depends on the number of awaited Promises inside `probeAll`. The intent is "drain enough microtasks that the immediate-on-subscribe probe has called `invoke` at least once."

- [ ] **Step 12: Add the visibility-change test**

Append:

```ts
  it('clears interval on visibilitychange to hidden; resumes on visible', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '192.168.1.10',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_remote_api_key: undefined,
      stt_mode: 'local',
    } as any);
    invokeMock.mockResolvedValue('Connected');

    const unsub = endpointHealth.subscribe(() => {});
    await Promise.resolve();
    await Promise.resolve();
    const initialCalls = invokeMock.mock.calls.length;

    // Simulate visibility change to hidden.
    Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
    document.dispatchEvent(new Event('visibilitychange'));

    // Advance time; no probes should fire.
    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock.mock.calls.length).toBe(initialCalls);

    // Simulate return to visible.
    Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
    document.dispatchEvent(new Event('visibilitychange'));
    await Promise.resolve();
    await Promise.resolve();
    expect(invokeMock.mock.calls.length).toBeGreaterThan(initialCalls);

    unsub();
  });
```

Run: `npx vitest run src/lib/stores/endpointHealth.test.ts 2>&1 | tail -15`

Expected: 12 tests pass.

If the test fails because `document.visibilityState` is not configurable in your jsdom/happy-dom setup, swap `Object.defineProperty` for a setter on `globalThis.document.visibilityState`. Alternatively, expose a `__setVisibilityForTest` helper on the store and call that. Pick whichever your environment supports.

- [ ] **Step 13: Full frontend sweep**

```bash
npx vitest run 2>&1 | tail -5
npm run check 2>&1 | tail -10
```

Expected: all tests pass; 0 svelte-check errors. The pre-existing `ExportDialog.svelte` warning from before this branch is acceptable.

- [ ] **Step 14: Commit**

```bash
git add src/lib/stores/endpointHealth.ts src/lib/stores/endpointHealth.test.ts
git commit -m "feat(frontend): add endpointHealth store with 10s polling

Subscribes to settings, polls test_ollama_connection / test_lmstudio_connection
/ test_stt_remote_connection at 10s intervals when at least one subscriber is
active. Skips loopback (127.x, ::1, localhost, empty, bracketed [::1]) and
empty STT host. Pauses on document.visibilitychange to hidden; resumes with
an immediate probe on visible. Reacts to settings changes with an immediate
re-probe. 12 store tests cover the state machine.

Phase 2 of the server-down error-messages effort (spec
docs/superpowers/specs/2026-05-13-server-down-phase2-design.md)."
```

---

## Task 2: Create `EndpointHealthPill.svelte`

**Files:**
- Create: `src/lib/components/EndpointHealthPill.svelte`

**Why:** The pill is the most visible piece of Phase 2 — it's what the clinician sees in the footer. It subscribes to `endpointHealth`, renders nothing when `overall === 'hidden'`, otherwise renders a colored dot + the "AI" label with a tooltip and a click handler.

- [ ] **Step 1: Create the component skeleton**

Create `src/lib/components/EndpointHealthPill.svelte`:

```svelte
<script lang="ts">
  import { endpointHealth, type EndpointHealthState } from '../stores/endpointHealth';
  import { settings } from '../stores/settings';

  type Props = {
    onopenSettings: (target: 'models' | 'audio') => void;
  };
  let { onopenSettings }: Props = $props();

  let health = $state<EndpointHealthState>({
    ai: 'skipped', stt: 'skipped', lastCheckedAt: null, overall: 'hidden',
  });
  const unsub = endpointHealth.subscribe((s) => (health = s));
  import { onDestroy } from 'svelte';
  onDestroy(unsub);

  function aiProviderLabel(): string {
    return $settings.ai_provider === 'ollama' ? 'Ollama' : 'LM Studio';
  }

  function lastCheckedDescription(): string {
    if (!health.lastCheckedAt) return 'not checked yet';
    const seconds = Math.floor((Date.now() - health.lastCheckedAt) / 1000);
    return `last checked ${seconds}s ago`;
  }

  function tooltipText(): string {
    const parts: string[] = [];
    if (health.ai === 'online') parts.push(`${aiProviderLabel()} online`);
    else if (health.ai === 'offline') parts.push(`${aiProviderLabel()} offline`);
    if (health.stt === 'online') parts.push('Whisper STT online');
    else if (health.stt === 'offline') parts.push('Whisper STT offline');
    if (parts.length === 0) return '';
    return `${parts.join(', ')} — ${lastCheckedDescription()}`;
  }

  function ariaLabelText(): string {
    switch (health.overall) {
      case 'online': return 'AI services online';
      case 'partial': return 'AI services partially offline';
      case 'offline': return 'AI services offline';
      default: return '';
    }
  }

  function onClick() {
    if (health.overall === 'online' || health.overall === 'hidden') return;
    // partial with AI offline only → models
    // partial with STT offline only → audio
    // offline (both) → models (AI is the more common case)
    if (health.overall === 'partial' && health.ai === 'online' && health.stt === 'offline') {
      onopenSettings('audio');
    } else {
      onopenSettings('models');
    }
  }

  function variantClass(): string {
    switch (health.overall) {
      case 'online': return 'ok';
      case 'partial': return 'warn';
      case 'offline': return 'error';
      default: return '';
    }
  }
</script>

{#if health.overall !== 'hidden'}
  <button
    type="button"
    class="endpoint-pill {variantClass()}"
    title={tooltipText()}
    aria-label={ariaLabelText()}
    onclick={onClick}
  >
    <span class="dot" aria-hidden="true"></span>
    AI
  </button>
{/if}

<style>
  .endpoint-pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 1px 7px;
    border-radius: 999px;
    font-weight: 600;
    letter-spacing: 0.02em;
    font-size: 11px;
    cursor: pointer;
    background: transparent;
  }
  .endpoint-pill.ok {
    background: rgba(22, 163, 74, 0.15);
    color: #16a34a;
    border: 1px solid rgba(22, 163, 74, 0.35);
  }
  .endpoint-pill.warn {
    background: rgba(217, 119, 6, 0.15);
    color: #d97706;
    border: 1px solid rgba(217, 119, 6, 0.35);
  }
  .endpoint-pill.error {
    background: rgba(220, 38, 38, 0.15);
    color: #dc2626;
    border: 1px solid rgba(220, 38, 38, 0.35);
  }
  .endpoint-pill .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: currentColor;
    box-shadow: 0 0 4px currentColor;
  }
  .endpoint-pill:hover {
    filter: brightness(1.05);
  }
</style>
```

The CSS mirrors the existing `.sharing-badge` block at `StatusBar.svelte:138-169` for visual consistency.

- [ ] **Step 2: Verify it compiles via svelte-check**

```bash
npm run check 2>&1 | tail -10
```

Expected: zero new errors. (Pre-existing ExportDialog warning is acceptable.)

- [ ] **Step 3: Commit**

```bash
git add src/lib/components/EndpointHealthPill.svelte
git commit -m "feat(frontend): EndpointHealthPill component

Subscribes to endpointHealth. Renders nothing when overall='hidden';
otherwise renders a colored pill ('AI' + dot) with per-service tooltip
and click-to-Settings routing (AI offline → models, STT offline → audio,
both offline → models). Reuses the .sharing-badge visual pattern from
StatusBar."
```

---

## Task 3: Mount the pill in StatusBar + wire `onopenSettings` through to App.svelte

**Files:**
- Modify: `src/lib/components/StatusBar.svelte`
- Modify: `src/App.svelte`

**Why:** The pill is now built but invisible. Mount it in the footer status bar's `.status-right` group, alongside the existing sharing badges. The pill emits `onopenSettings(target)`; `StatusBar` forwards the callback up to `App.svelte`, which sets `settingsOpen = true` and writes to `settingsNav` — the exact same pattern Phase 1's `EndpointOfflineDialog` already uses.

- [ ] **Step 1: Add the pill to StatusBar.svelte**

In `src/lib/components/StatusBar.svelte`, change the script section to accept `onopenSettings` as a prop, and mount the pill in `.status-right`.

Replace the `<script lang="ts">` block's imports + `let pollHandle` line area with this (keep the existing `refresh()`, `onMount`, `onDestroy` blocks intact):

```ts
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { audio } from '../stores/audio';
  import { settings } from '../stores/settings';
  import { formatDuration } from '../utils/format';
  import EndpointHealthPill from './EndpointHealthPill.svelte';

  type Props = {
    onopenSettings: (target: 'models' | 'audio') => void;
  };
  let { onopenSettings }: Props = $props();

  type SharingStatus = {
    enabled: boolean;
    paired_clients: number;
  };
  type PairedConn = { label: string } | null;

  let sharing: SharingStatus | null = $state(null);
  let paired: PairedConn = $state(null);
  let pollHandle: ReturnType<typeof setInterval>;
```

(`let sharing` and `let paired` are switched to `$state` because the file is being touched and Svelte 5 prefers it. If this introduces issues — e.g. the parent already expected non-runed reactivity — leave them as plain `let` and rely on Svelte 5's auto-detection.)

Then, in the markup, insert the pill into `.status-right`. Replace the existing `.status-right` block with:

```svelte
  <div class="status-right">
    <EndpointHealthPill {onopenSettings} />
    {#if sharing?.enabled}
      <span class="sharing-badge server" title="This machine is acting as an office server. Other paired clients can reach Ollama / Whisper / LM Studio via this device.">
        <span class="dot" aria-hidden="true"></span>
        Office Server
        {#if sharing.paired_clients > 0}
          <span class="badge-count">· {sharing.paired_clients} client{sharing.paired_clients === 1 ? '' : 's'}</span>
        {/if}
      </span>
      <span class="status-sep">·</span>
    {:else if paired}
      <span class="sharing-badge client" title={`Paired with office server${paired.label ? ` as “${paired.label}”` : ''}.`}>
        <span class="dot" aria-hidden="true"></span>
        Paired
      </span>
      <span class="status-sep">·</span>
    {/if}
    <span class="status-provider">AI: {$settings.ai_provider}/{$settings.ai_model}</span>
    <span class="status-sep">·</span>
    <span class="status-provider">STT: {$settings.whisper_model}</span>
  </div>
```

(The pill goes first so it's the leftmost item — most-prominent placement. The existing sharing badges follow as before.)

- [ ] **Step 2: Pass the callback from App.svelte**

In `src/App.svelte`, find where `<StatusBar />` is rendered. It currently has no props. Change it to:

```svelte
<StatusBar onopenSettings={onEndpointOfflineOpenSettings} />
```

`onEndpointOfflineOpenSettings` already exists in `App.svelte` (added during Phase 1 Task 12 to handle the dialog's callback). The same function handles both pathways — pill click and dialog click — because both go to the same destination (Settings → Models or Audio via `settingsNav.navigateTo`).

If the function's name in App.svelte is different from `onEndpointOfflineOpenSettings`, use whatever the actual name is. Find it with: `grep -n 'navigateTo\|onopenSettings\|settingsNav' src/App.svelte`.

- [ ] **Step 3: Build + svelte-check**

```bash
npm run check 2>&1 | tail -10
```

Expected: 0 errors. If `StatusBar.svelte` had `let sharing = null; ...` (Svelte 5 lets-without-runes), and you converted them to `$state`, watch for "Cannot have reactive ($state) and non-reactive declarations together" warnings. Either run all of `sharing`/`paired`/`pollHandle` through `$state` or none.

- [ ] **Step 4: Smoke-test by running the app dev server**

```bash
npm run tauri dev 2>&1 &
TAURI_PID=$!
sleep 12
# (manually verify the footer shows the new pill if there's a remote endpoint configured)
kill $TAURI_PID
```

(Optional — the QA in Task 6 covers this more thoroughly. If `npm run tauri dev` is slow to bring up, skip and rely on Task 6.)

- [ ] **Step 5: Run all frontend tests**

```bash
npx vitest run 2>&1 | tail -5
```

Expected: all tests still pass. No tests should break since the pill renders nothing in the test environment (no `settings` configured to non-loopback).

- [ ] **Step 6: Commit**

```bash
git add src/lib/components/StatusBar.svelte src/App.svelte
git commit -m "feat(frontend): mount EndpointHealthPill in StatusBar

Pill renders in the .status-right group, leftmost. Forwards
onopenSettings to App.svelte's existing settingsNav handler — reuses
the Phase 1 navigation wiring. Hidden by default for fully-local users
(the pill returns null when overall='hidden')."
```

---

## Task 4: Create `OfflineRecordBanner.svelte`

**Files:**
- Create: `src/lib/components/OfflineRecordBanner.svelte`

**Why:** When `endpointHealth.overall` is `'partial'` or `'offline'` and the user is on the Record tab, an inline banner above the record button warns that recording will save locally but processing will fail. This is the second visible Phase 2 surface.

- [ ] **Step 1: Create the component**

Create `src/lib/components/OfflineRecordBanner.svelte`:

```svelte
<script lang="ts">
  import { endpointHealth, type EndpointHealthState } from '../stores/endpointHealth';
  import { onDestroy } from 'svelte';

  type Props = {
    onopenSettings: (target: 'models' | 'audio') => void;
  };
  let { onopenSettings }: Props = $props();

  let health = $state<EndpointHealthState>({
    ai: 'skipped', stt: 'skipped', lastCheckedAt: null, overall: 'hidden',
  });
  const unsub = endpointHealth.subscribe((s) => (health = s));
  onDestroy(unsub);

  function bannerText(): string {
    if (health.overall === 'offline') {
      return "Office server offline — your recording will save locally, but transcription and SOAP will fail until it's back online.";
    }
    if (health.overall === 'partial') {
      if (health.ai === 'offline' && health.stt === 'online') {
        return 'AI offline — your recording will save locally, but SOAP generation will fail.';
      }
      if (health.stt === 'offline' && health.ai === 'online') {
        return 'Whisper STT offline — your recording will save locally, but transcription will fail.';
      }
    }
    return '';
  }

  function onOpenSettingsClick() {
    // Pick the target the same way the pill does.
    if (health.overall === 'partial' && health.ai === 'online' && health.stt === 'offline') {
      onopenSettings('audio');
    } else {
      onopenSettings('models');
    }
  }
</script>

{#if health.overall === 'partial' || health.overall === 'offline'}
  <div class="offline-banner" role="status" aria-live="polite">
    <span class="banner-text">{bannerText()}</span>
    <button type="button" class="banner-action" onclick={onOpenSettingsClick}>
      Open Settings
    </button>
  </div>
{/if}

<style>
  .offline-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    margin-bottom: 12px;
    background-color: var(--warning-bg, rgba(217, 119, 6, 0.1));
    border: 1px solid var(--warning, #d97706);
    border-radius: var(--radius-md);
    font-size: 13px;
    color: var(--warning, #d97706);
  }
  .banner-text {
    flex: 1;
  }
  .banner-action {
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    font-size: 12px;
    color: var(--warning, #d97706);
    border: 1px solid var(--warning, #d97706);
    background: transparent;
    cursor: pointer;
  }
  .banner-action:hover {
    background-color: var(--warning, #d97706);
    color: white;
  }
</style>
```

The styling closely mirrors the existing `.error-banner` block in `RecordingHeader.svelte:97-128`, using `--warning` (amber) instead of `--danger` (red) so the banner reads as a warning rather than a hard error. (The recording itself isn't failing — it's a heads-up that *processing* will.)

- [ ] **Step 2: svelte-check**

```bash
npm run check 2>&1 | tail -10
```

Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
git add src/lib/components/OfflineRecordBanner.svelte
git commit -m "feat(frontend): OfflineRecordBanner component

Inline warning above the record button when endpointHealth.overall is
partial or offline. Copy varies by which service is down (AI / STT /
both). Reuses the .error-banner styling pattern from RecordingHeader
but in warning-amber rather than danger-red — recording itself works;
only later processing will fail."
```

---

## Task 5: Mount the banner in RecordingHeader

**Files:**
- Modify: `src/lib/components/RecordingHeader.svelte`

**Why:** Banner is now built. Mount it above `.controls-row` (the row containing the timer + record button), so the warning is visible the moment the user looks at the recording controls.

- [ ] **Step 1: Investigate how RecordingHeader gets the `onopenSettings` callback**

`RecordingHeader.svelte` is mounted by `RecordTab.svelte`. The callback chain needs to reach `RecordingHeader`. Two routing options:

(a) **Plumb the callback through:** `App.svelte` → `RecordTab.svelte` → `RecordingHeader.svelte` → `OfflineRecordBanner.svelte`. Adds a `Props.onopenSettings` to two more components.

(b) **Read `settingsNav` directly:** The banner imports and uses `settingsNav.navigateTo(...)` itself, and also writes to `settingsOpen` if there's a way to access it from anywhere.

Option (a) is cleaner and matches what Phase 1's dialog does. The pill is already routed via (a). Use the same approach.

Run: `grep -n 'RecordingHeader\|RecordTab' src/lib/pages/RecordTab.svelte | head -10`

Confirm the file path and how `RecordingHeader` is invoked.

- [ ] **Step 2: Modify RecordTab.svelte to accept and forward the callback**

Find `src/lib/pages/RecordTab.svelte`. Locate the `<script>` block. Add a `Props.onopenSettings: (target: 'models' | 'audio') => void` field and wire it to RecordingHeader.

If `RecordTab.svelte` currently has props like `Props = { … }`, append `onopenSettings` to that type. If it has none, add a Props type from scratch. Concretely, add to the `<script>` block:

```ts
  type Props = {
    onopenSettings: (target: 'models' | 'audio') => void;
    // …any existing props…
  };
  let { onopenSettings /* …existing props… */ }: Props = $props();
```

And update the `<RecordingHeader ... />` call to pass `{onopenSettings}`:

```svelte
<RecordingHeader {onopenSettings} {onStart} {onStop} {onNewRecording} />
```

(Read the existing call site for the exact other props.)

- [ ] **Step 3: Modify App.svelte to pass the callback to RecordTab**

Find where `<RecordTab />` is rendered in `App.svelte`. Pass the same handler:

```svelte
<RecordTab onopenSettings={onEndpointOfflineOpenSettings} />
```

(Use whatever the actual handler name is in `App.svelte` — see Task 3 Step 2.)

If `RecordTab` is rendered inside a tab-routing block (e.g. `{#if currentTab === 'record'}`), the prop goes inside that block.

- [ ] **Step 4: Modify RecordingHeader.svelte to accept the callback and mount the banner**

Update `src/lib/components/RecordingHeader.svelte`. Add the prop and import the banner. Replace the `<script>` block's `Props` interface with:

```ts
<script lang="ts">
  import { audio } from '../stores/audio';
  import { formatDuration } from '../utils/format';
  import Waveform from './Waveform.svelte';
  import OfflineRecordBanner from './OfflineRecordBanner.svelte';

  interface Props {
    onStart?: () => void;
    onStop?: () => void;
    onNewRecording?: () => void;
    onopenSettings: (target: 'models' | 'audio') => void;
  }
  let { onStart, onStop, onNewRecording, onopenSettings }: Props = $props();
```

Then, in the markup, add the banner immediately above `.controls-row` (around line 45 in the original file):

```svelte
<div class="recording-header">
  {#if $audio.error}
    <div class="error-banner">
      <span class="error-text">{$audio.error}</span>
      <button class="error-dismiss" onclick={() => audio.reset()}>Dismiss</button>
    </div>
  {/if}

  <OfflineRecordBanner {onopenSettings} />

  <div class="controls-row">
    …existing controls…
  </div>
  …
</div>
```

(The banner is conditional on store state, so it self-hides when the server is up. No `{#if}` guard needed here.)

- [ ] **Step 5: svelte-check and tests**

```bash
npm run check 2>&1 | tail -10
npx vitest run 2>&1 | tail -5
```

Expected: 0 errors; all tests still pass.

- [ ] **Step 6: Commit**

```bash
git add src/lib/components/RecordingHeader.svelte src/lib/pages/RecordTab.svelte src/App.svelte
git commit -m "feat(frontend): mount OfflineRecordBanner in RecordingHeader

Plumbs onopenSettings from App.svelte through RecordTab to
RecordingHeader (and into the banner). Banner self-hides when
endpointHealth.overall is 'online' or 'hidden'. Clicking Open Settings
takes the same routing decision the pill makes — to Models for AI
issues, to Audio for STT-only issues."
```

---

## Task 6: Manual QA + version bump

**Files:**
- Modify: `src-tauri/Cargo.toml` (version field)
- Modify: `package.json` (version field)
- Modify: `src-tauri/tauri.conf.json` (version field)

**Why:** Phase 2 is shippable. Bump 0.10.57 → 0.10.58 (patch — additive UI feature), run the manual QA checklist on a real Mac/Windows pair, and tag.

- [ ] **Step 1: Run the full sweep one more time**

```bash
cargo test --workspace --lib 2>&1 | tail -5
npx vitest run 2>&1 | tail -5
npm run check 2>&1 | tail -10
```

Expected: 599 backend tests / 155+ (will be 167 with the 12 new) frontend tests / 0 svelte-check errors. Pre-existing ExportDialog warning is acceptable.

- [ ] **Step 2: Manual QA**

Run this checklist on a real Windows-client / Mac-server pair (or a single dev machine with two Ollama instances on different ports — set up a remote-like config pointing at a non-loopback host):

1. Set `ai_provider = ollama`, `ollama_host = <real reachable Ollama>` (or a wiremock at a LAN IP), `ollama_port = 11434`. Start the app. Within 10s the pill appears green in the status bar with tooltip "Ollama online — last checked Xs ago".
2. Stop Ollama. Within 10s the pill turns red. Tooltip names Ollama offline. Open the Record tab — the warning banner appears above the controls row with "Office server offline — your recording will save locally…" copy.
3. Restart Ollama. Within 10s the pill turns green again. Banner disappears.
4. Configure remote STT at a reachable host. Pill stays green showing both services in tooltip.
5. Stop just the STT server. Pill turns amber; tooltip says "Ollama online, Whisper STT offline". Banner shows the STT-only copy.
6. Click the pill while red → app navigates to Settings → Models.
7. Click the banner's "Open Settings" → same destination (or Settings → Audio when STT is the issue).
8. Switch `ai_provider` to LM Studio (still remote, reachable). Within ~200ms the pill re-probes against the new endpoint.
9. Switch to fully-local (`ai_provider = lmstudio` with `localhost`, `stt_remote_host = ''`). The pill disappears within 10s. Banner does not appear on the Record tab.
10. Minimize the window. Devtools network panel should show no probe traffic. Restore the window — one immediate probe fires, then the 10s cadence resumes.

Record any failures; fix and re-run the affected step. Don't proceed to Step 3 unless 1–10 all pass.

- [ ] **Step 3: Bump version**

Locate the current version: `grep '^version' src-tauri/Cargo.toml` → `0.10.57`. Bump to `0.10.58` in all three files:

- `src-tauri/Cargo.toml` — `version = "0.10.58"`
- `package.json` — `"version": "0.10.58"`
- `src-tauri/tauri.conf.json` — `"version": "0.10.58"`

Verify:

```bash
grep -E '0\.10\.5[78]' src-tauri/Cargo.toml package.json src-tauri/tauri.conf.json
```

Expected: all three show `0.10.58`.

- [ ] **Step 4: Final test sweep after bump**

```bash
cargo build -p rust-medical-assistant 2>&1 | tail -5
cargo test -p rust-medical-assistant --lib 2>&1 | tail -5
```

Expected: clean build; tests pass.

- [ ] **Step 5: Commit and tag**

```bash
git add src-tauri/Cargo.toml package.json src-tauri/tauri.conf.json
git commit -m "chore: bump 0.10.58 — server-down error messages (Phase 2)

Ambient AI status pill in the footer status bar + inline banner above
the record button when the server is offline. Frontend-only, reuses
Phase 1's test_*_connection commands. Polling pauses while window is
hidden."

git tag v0.10.58
```

(Don't push; the user pushes when ready.)

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task implementing it |
|---|---|
| AC#1: endpointHealth store with `EndpointHealthState` + `probeNow()` | Task 1 |
| AC#2: 10s polling starts on first subscribe, clears on last unsubscribe | Task 1 (Step 9–11) |
| AC#3: visibilitychange pause/resume | Task 1 (Step 12) |
| AC#4: probes use `test_*_connection`; loopback / empty skip | Task 1 (Steps 5–8) |
| AC#5: settings changes trigger immediate re-probe | Task 1 (Step 9–10) |
| AC#6: pill renders/colors/click per state table | Task 2 + Task 3 |
| AC#7: banner renders only `partial`/`offline`, copy per state | Task 4 + Task 5 |
| AC#8: pill mounted in StatusBar, banner mounted in RecordingHeader | Tasks 3 + 5 |
| AC#9: no PHI in frontend logs | Tasks 1, 2, 4 emit no `console.log` |
| AC#10: vitest + svelte-check green | All tasks include the sweep |
| AC#11: manual QA passes | Task 6 |

**Placeholder scan:** No `TBD`, `TODO`, "fill in details" remain. The two "find this in the actual file" notes in Tasks 3 and 5 (`grep` commands to locate the existing handler name and the existing RecordTab call site) are explicit, executable, with the alternative path documented if the names differ.

**Type consistency:** `EndpointHealthState` field names (`ai`, `stt`, `lastCheckedAt`, `overall`) and types (`ServiceStatus`, `Overall`) match across the store (Task 1), pill (Task 2), and banner (Task 4). `onopenSettings: (target: 'models' | 'audio') => void` signature is identical in `EndpointHealthPill`, `OfflineRecordBanner`, `StatusBar`, `RecordTab`, and `RecordingHeader`. The `settingsNav.navigateTo(...)` destinations (`'models'` / `'audio'`) match the existing Phase 1 store's accepted values.

**Known under-specifications** (flagged inline with "If … in this file …" notes):
- Task 1 Step 11: microtask drain count depends on the test environment's Promise scheduling — add one more `await Promise.resolve()` if a polling-lifecycle test sees zero invokes.
- Task 1 Step 12: `Object.defineProperty(document, 'visibilityState', ...)` may need a setter trick depending on jsdom/happy-dom version — the alternative is a `__setVisibilityForTest` test-only hook.
- Task 3 Step 2: the existing handler name in `App.svelte` is `onEndpointOfflineOpenSettings` per Phase 1 Task 12; if a future rename happens, grep to confirm.
- Task 5 Steps 2–3: `RecordTab` may have other props already — the implementor reads the existing component before adding `onopenSettings`.

These are intentional: the engineer reads the live file once per task before changing it. No required type, function, or constant in this plan is undefined.

---

Plan complete and saved to `docs/superpowers/plans/2026-05-13-server-down-phase2.md`. Two execution options:

**1. Subagent-Driven (recommended)** — fresh subagent per task with two-stage review (spec + code quality). Matches your CLAUDE.md convention and the Phase 1 process.

**2. Inline Execution** — execute tasks in this session with checkpoints.

Which approach?
