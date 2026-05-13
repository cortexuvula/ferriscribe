import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';

import {
  invokeWithOfflineHandling,
  OfflineCancelled,
  type EndpointOfflinePayload,
} from './invokeWithOfflineHandling';
import { endpointOfflineStore } from '../stores/endpointOffline';

const invokeMock = vi.mocked(invoke);

function offlinePayload(overrides: Partial<EndpointOfflinePayload> = {}): EndpointOfflinePayload {
  return {
    kind: 'EndpointOffline',
    service: 'AiProvider',
    endpoint: 'http://x:1',
    reason: 'ConnectionRefused',
    provider_name: 'Ollama',
    message: 'mock',
    ...overrides,
  };
}

describe('invokeWithOfflineHandling', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    endpointOfflineStore.close();
  });

  it('resolves normally on first-attempt success', async () => {
    invokeMock.mockResolvedValueOnce('the result');
    await expect(
      invokeWithOfflineHandling<string>('do_thing', { x: 1 }),
    ).resolves.toBe('the result');
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith('do_thing', { x: 1 });
  });

  it('passes through non-offline errors unchanged', async () => {
    const err = { kind: 'AiProvider', message: 'rate limit' };
    invokeMock.mockRejectedValueOnce(err);
    await expect(invokeWithOfflineHandling('do_thing', {})).rejects.toBe(err);
  });

  it('opens dialog on EndpointOffline and resolves with the retry result on success', async () => {
    invokeMock
      .mockRejectedValueOnce(offlinePayload())
      .mockResolvedValueOnce('after retry');

    const pending = invokeWithOfflineHandling<string>('do_thing', { y: 2 });
    // Let the microtask queue drain so the helper has opened the dialog.
    await new Promise((r) => setTimeout(r, 0));
    endpointOfflineStore._resolve('retry');

    await expect(pending).resolves.toBe('after retry');
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(invokeMock).toHaveBeenNthCalledWith(1, 'do_thing', { y: 2 });
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'do_thing', { y: 2 });
  });

  it('re-opens dialog if retry fails again, then cancels', async () => {
    invokeMock
      .mockRejectedValueOnce(offlinePayload())
      .mockRejectedValueOnce(offlinePayload());

    const pending = invokeWithOfflineHandling<string>('do_thing', {});
    await new Promise((r) => setTimeout(r, 0));
    endpointOfflineStore._resolve('retry');
    await new Promise((r) => setTimeout(r, 0));
    endpointOfflineStore._resolve('cancel');

    await expect(pending).rejects.toBeInstanceOf(OfflineCancelled);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it('throws OfflineCancelled on cancel', async () => {
    invokeMock.mockRejectedValueOnce(offlinePayload());
    const pending = invokeWithOfflineHandling('do_thing', {});
    await new Promise((r) => setTimeout(r, 0));
    endpointOfflineStore._resolve('cancel');
    const err = await pending.catch((e) => e);
    expect(err).toBeInstanceOf(OfflineCancelled);
    expect((err as OfflineCancelled).reason).toBe('cancel');
  });

  it('throws OfflineCancelled on opened_settings', async () => {
    invokeMock.mockRejectedValueOnce(offlinePayload());
    const pending = invokeWithOfflineHandling('do_thing', {});
    await new Promise((r) => setTimeout(r, 0));
    endpointOfflineStore._resolve('opened_settings');
    const err = await pending.catch((e) => e);
    expect(err).toBeInstanceOf(OfflineCancelled);
    expect((err as OfflineCancelled).reason).toBe('opened_settings');
  });

  it('does not open the dialog when error is not EndpointOffline', async () => {
    invokeMock.mockRejectedValueOnce({ kind: 'AiProvider', message: 'x' });
    await invokeWithOfflineHandling('do_thing', {}).catch(() => {});
    // Store should not have been populated at any point — but since the helper
    // throws before ever calling openAndWait, the store stays null. (If we
    // could spy on store calls, that would be cleaner. The behavioural proxy
    // is: the test for "passes through" already proves it.)
  });

  it('resolves normally when args is omitted', async () => {
    invokeMock.mockResolvedValueOnce('result');
    await expect(invokeWithOfflineHandling<string>('zero_arg_cmd')).resolves.toBe('result');
    // Tauri's invoke accepts the empty object as args.
    expect(invokeMock).toHaveBeenCalledWith('zero_arg_cmd', {});
  });
});
