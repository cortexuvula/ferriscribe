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
// The vi.mock above replaces settings with a plain writable; cast to any so
// svelte-check doesn't complain that .set() / .update() don't exist on the
// real settings store type.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
import { settings as _settings } from '../stores/settings';
const settings = _settings as any;

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
      if (cmd === 'test_lmstudio_connection') return Promise.resolve('Connected');
      if (cmd === 'get_api_key') return Promise.resolve(null);
      if (cmd === 'test_stt_remote_connection') return Promise.reject({ kind: 'SttProvider', message: 'timeout' });
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    const state = get(endpointHealth);
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
    await Promise.resolve();
    const callsBefore = invokeMock.mock.calls.length;

    // Change a probed field.
    settings.update((s: any) => ({ ...s, ollama_host: '192.168.1.99' }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();

    expect(invokeMock.mock.calls.length).toBeGreaterThan(callsBefore);
    expect(invokeMock).toHaveBeenLastCalledWith('test_ollama_connection', {
      host: '192.168.1.99',
      port: 11434,
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
      if (cmd === 'test_stt_remote_connection') return Promise.resolve('Connected');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('get_api_key', {
      provider: 'stt_remote_api_key',
    });
    expect(invokeMock).toHaveBeenCalledWith('test_stt_remote_connection', {
      host: '192.168.1.20',
      port: 8080,
      apiKey: 'secret-token-abc',
    });
    const state = get(endpointHealth);
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
      if (cmd === 'test_stt_remote_connection') return Promise.resolve('Connected');
      return Promise.resolve(undefined);
    });

    await endpointHealth.probeNow();

    expect(invokeMock).toHaveBeenCalledWith('test_stt_remote_connection', {
      host: '192.168.1.20',
      port: 8080,
      apiKey: undefined,
    });
    const state = get(endpointHealth);
    expect(state.stt).toBe('online'); // probe ran, succeeded without auth
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
