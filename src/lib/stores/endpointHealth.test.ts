import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
const invokeMock = vi.mocked(invoke);

// Mock the new runes-based settings store. The endpointHealth store reads
// settings.state and calls settings.subscribe(cb). Tests mutate state via
// settings.set() / settings.update() which trigger all active subscribers,
// mirroring the old writable behaviour.
vi.mock('../stores/settings.svelte', () => {
  let _state: any = {
    ai_provider: 'lmstudio',
    lmstudio_host: '',
    lmstudio_port: 1234,
    ollama_host: '',
    ollama_port: 11434,
    stt_remote_host: '',
    stt_remote_port: 8080,
    stt_remote_api_key: undefined,
    stt_mode: 'local',
  };
  const _subscribers = new Set<(v: any) => void>();
  function notify() { for (const cb of _subscribers) cb(_state); }
  const obj = {
    get state() { return _state; },
    set(next: any) { _state = next; notify(); },
    update(fn: (s: any) => any) { _state = fn(_state); notify(); },
    subscribe(cb: (v: any) => void) {
      cb(_state); // emit current value immediately
      _subscribers.add(cb);
      return () => { _subscribers.delete(cb); };
    },
  };
  return { settings: obj };
});

// Import after mocks are set up.
import { endpointHealth } from './endpointHealth.svelte';
// The vi.mock above replaces settings with a plain object; cast to any so
// svelte-check doesn't complain that .set() / .update() don't exist on the
// real settings store type.
 
import { settings as _settings } from '../stores/settings.svelte';
const settings = _settings as any;

describe('endpointHealth store', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    vi.useFakeTimers();
  });

  it('starts in hidden state before any subscriber', () => {
    const state = endpointHealth.state;
    expect(state.ai).toBe('skipped');
    expect(state.stt).toBe('skipped');
    expect(state.overall).toBe('hidden');
    expect(state.lastCheckedAt).toBeNull();
  });

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

    // get_api_key (lmstudio_api_key) fires first, then the probe.
    invokeMock.mockResolvedValueOnce(null); // get_api_key → no key
    invokeMock.mockResolvedValueOnce('Connected — 3 models available');

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('get_api_key', { provider: 'lmstudio_api_key' });
    expect(invokeMock).toHaveBeenCalledWith('probe_endpoint_reachable', {
      service: 'AiProvider',
      providerName: 'LM Studio',
      host: '192.168.1.10',
      port: 1234,
      probePath: '/v1/models',
      apiKey: undefined,
    });
    const state = endpointHealth.state;
    expect(state.ai).toBe('online');
    expect(state.stt).toBe('skipped');
    expect(state.overall).toBe('online');
    expect(state.lastCheckedAt).not.toBeNull();
  });

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

    // get_api_key (ollama_api_key) fires first, then the probe rejects.
    invokeMock.mockResolvedValueOnce(null); // get_api_key → no key
    invokeMock.mockRejectedValueOnce({
      kind: 'AiProvider',
      message: 'Connection refused — is Ollama running at 192.168.1.10:11434?',
    });

    await endpointHealth.probeNow();

    const state = endpointHealth.state;
    expect(state.ai).toBe('offline');
    expect(state.overall).toBe('offline');
  });

  it('overall is partial when one service is online and the other offline', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '192.168.1.10',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '192.168.1.10',
      stt_remote_port: 8080,
      stt_mode: 'remote',
    } as any);

    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'probe_endpoint_reachable') return Promise.resolve('Connected');
      if (cmd === 'get_api_key') return Promise.resolve(null); // handles lmstudio_api_key and stt_remote_api_key
      return Promise.resolve(undefined);
    });

    // Override: STT probe should reject (to simulate offline)
    invokeMock.mockImplementation((cmd: string, args: any) => {
      if (cmd === 'get_api_key') return Promise.resolve(null);
      if (cmd === 'probe_endpoint_reachable' && args?.service === 'RemoteStt') {
        return Promise.reject({ kind: 'SttProvider', message: 'timeout' });
      }
      if (cmd === 'probe_endpoint_reachable') return Promise.resolve('Connected');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    const state = endpointHealth.state;
    expect(state.ai).toBe('online');
    expect(state.stt).toBe('offline');
    expect(state.overall).toBe('partial');
  });

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
    const state = endpointHealth.state;
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

    // get_api_key (lmstudio_api_key) then test_lmstudio_connection. STT skipped (empty host).
    invokeMock.mockResolvedValueOnce(null); // get_api_key
    invokeMock.mockResolvedValueOnce('Connected'); // test_lmstudio_connection

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(invokeMock).toHaveBeenCalledWith('probe_endpoint_reachable', expect.anything());
    const state = endpointHealth.state;
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
    const state = endpointHealth.state;
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
    const state = endpointHealth.state;
    expect(state.ai).toBe('skipped');
  });

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
    // First probe fires immediately on subscribe: get_api_key + test_lmstudio_connection = 2 calls.
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(2);

    // Second probe after 10 s tick: 2 more calls.
    await vi.advanceTimersByTimeAsync(10_000);
    expect(invokeMock).toHaveBeenCalledTimes(4);

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
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'get_api_key') return Promise.resolve(null);
      return Promise.resolve('Connected');
    });

    const unsub = endpointHealth.subscribe(() => {});
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    const callsBefore = invokeMock.mock.calls.length;

    // Change a probed field.
    settings.update((s: any) => ({ ...s, ollama_host: '192.168.1.99' }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    expect(invokeMock.mock.calls.length).toBeGreaterThan(callsBefore);
    expect(invokeMock).toHaveBeenLastCalledWith('probe_endpoint_reachable', {
      service: 'AiProvider',
      providerName: 'Ollama',
      host: '192.168.1.99',
      port: 11434,
      probePath: '/api/tags',
      apiKey: undefined,
    });
    unsub();
  });

  it('fetches stt_remote_api_key from keychain and forwards it to the STT probe', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '127.0.0.1',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '192.168.1.20',
      stt_remote_port: 8080,
      stt_mode: 'remote',
    } as any);

    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'get_api_key') return Promise.resolve('secret-token-abc');
      if (cmd === 'probe_endpoint_reachable') return Promise.resolve('Connected');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('get_api_key', {
      provider: 'stt_remote_api_key',
    });
    expect(invokeMock).toHaveBeenCalledWith('probe_endpoint_reachable', {
      service: 'RemoteStt',
      providerName: 'Whisper STT',
      host: '192.168.1.20',
      port: 8080,
      probePath: '/v1/models',
      apiKey: 'secret-token-abc',
    });
    const state = endpointHealth.state;
    expect(state.stt).toBe('online');
  });

  it('STT probe continues without auth if keychain fetch fails', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '127.0.0.1',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '192.168.1.20',
      stt_remote_port: 8080,
      stt_mode: 'remote',
    } as any);

    invokeMock.mockImplementation((cmd: string, _args: any) => {
      if (cmd === 'get_api_key') return Promise.reject(new Error('keychain locked'));
      if (cmd === 'probe_endpoint_reachable') return Promise.resolve('Connected');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('probe_endpoint_reachable', {
      service: 'RemoteStt',
      providerName: 'Whisper STT',
      host: '192.168.1.20',
      port: 8080,
      probePath: '/v1/models',
      apiKey: undefined,
    });
    const state = endpointHealth.state;
    expect(state.stt).toBe('online'); // probe ran, succeeded without auth
  });

  it('fetches ollama_api_key from keychain and forwards it to the Ollama probe', async () => {
    settings.set({
      ai_provider: 'ollama',
      lmstudio_host: '',
      lmstudio_port: 1234,
      ollama_host: '192.168.1.10',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_mode: 'local',
    } as any);

    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'get_api_key') return Promise.resolve('bearer-token-xyz');
      if (cmd === 'probe_endpoint_reachable') return Promise.resolve('Connected — 2 models installed');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('get_api_key', { provider: 'ollama_api_key' });
    expect(invokeMock).toHaveBeenCalledWith('probe_endpoint_reachable', {
      service: 'AiProvider',
      providerName: 'Ollama',
      host: '192.168.1.10',
      port: 11434,
      probePath: '/api/tags',
      apiKey: 'bearer-token-xyz',
    });
    const state = endpointHealth.state;
    expect(state.ai).toBe('online');
  });

  it('fetches lmstudio_api_key from keychain and forwards it to the LM Studio probe', async () => {
    settings.set({
      ai_provider: 'lmstudio',
      lmstudio_host: '192.168.1.10',
      lmstudio_port: 1234,
      ollama_host: '',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_mode: 'local',
    } as any);

    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'get_api_key') return Promise.resolve('lm-bearer-token');
      if (cmd === 'probe_endpoint_reachable') return Promise.resolve('Connected — 1 model available');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('get_api_key', { provider: 'lmstudio_api_key' });
    expect(invokeMock).toHaveBeenCalledWith('probe_endpoint_reachable', {
      service: 'AiProvider',
      providerName: 'LM Studio',
      host: '192.168.1.10',
      port: 1234,
      probePath: '/v1/models',
      apiKey: 'lm-bearer-token',
    });
    const state = endpointHealth.state;
    expect(state.ai).toBe('online');
  });

  it('AI probe continues without auth if keychain fetch fails', async () => {
    settings.set({
      ai_provider: 'ollama',
      lmstudio_host: '',
      lmstudio_port: 1234,
      ollama_host: '192.168.1.10',
      ollama_port: 11434,
      stt_remote_host: '',
      stt_remote_port: 8080,
      stt_mode: 'local',
    } as any);

    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'get_api_key') return Promise.reject(new Error('keychain locked'));
      if (cmd === 'probe_endpoint_reachable') return Promise.resolve('Connected');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('probe_endpoint_reachable', {
      service: 'AiProvider',
      providerName: 'Ollama',
      host: '192.168.1.10',
      port: 11434,
      probePath: '/api/tags',
      apiKey: undefined,
    });
    const state = endpointHealth.state;
    expect(state.ai).toBe('online'); // probe ran, succeeded without auth
  });

  it('clears interval on visibilitychange to hidden; resumes on visible', async () => {
    // Set up a minimal document stub in the node environment so the visibility
    // handler path in the store gets exercised. The real browser environment
    // uses the native document; here we simulate it.
    const listeners: Record<string, (() => void)[]> = {};
    let currentVisibilityState = 'visible';

    const docStub = {
      get visibilityState() { return currentVisibilityState; },
      addEventListener(type: string, handler: () => void) {
        (listeners[type] ??= []).push(handler);
      },
      removeEventListener(type: string, handler: () => void) {
        listeners[type] = (listeners[type] ?? []).filter((h) => h !== handler);
      },
      dispatchEvent(_event: unknown) { return true; },
    };

    // Inject the stub so the store's startPolling() sees document.
    Object.defineProperty(globalThis, 'document', {
      value: docStub,
      configurable: true,
      writable: true,
    });

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
    await Promise.resolve();
    const initialCalls = invokeMock.mock.calls.length;

    // Simulate visibility change to hidden.
    currentVisibilityState = 'hidden';
    for (const h of listeners['visibilitychange'] ?? []) h();

    // Advance time; no probes should fire.
    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock.mock.calls.length).toBe(initialCalls);

    // Simulate return to visible.
    currentVisibilityState = 'visible';
    for (const h of listeners['visibilitychange'] ?? []) h();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(invokeMock.mock.calls.length).toBeGreaterThan(initialCalls);

    unsub();

    // Clean up the document stub.
    Object.defineProperty(globalThis, 'document', {
      value: undefined,
      configurable: true,
      writable: true,
    });
  });
});
