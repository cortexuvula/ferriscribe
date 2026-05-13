import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { endpointOfflineStore } from './endpointOffline';
import type { EndpointOfflinePayload } from '../api/invokeWithOfflineHandling';

const samplePayload: EndpointOfflinePayload = {
  kind: 'EndpointOffline',
  service: 'AiProvider',
  endpoint: 'http://192.168.1.10:11434',
  reason: 'ConnectionRefused',
  provider_name: 'Ollama',
  message: 'Ollama at http://192.168.1.10:11434 is offline (ConnectionRefused)',
};

describe('endpointOfflineStore', () => {
  beforeEach(() => {
    endpointOfflineStore.close();
  });

  it('starts in a closed state', () => {
    expect(get(endpointOfflineStore)).toBeNull();
  });

  it('openAndWait populates state with the payload', () => {
    void endpointOfflineStore.openAndWait(samplePayload);
    const s = get(endpointOfflineStore);
    expect(s).not.toBeNull();
    expect(s?.payload).toEqual(samplePayload);
  });

  it('openAndWait resolves with retry when _resolve("retry") is called', async () => {
    const pending = endpointOfflineStore.openAndWait(samplePayload);
    endpointOfflineStore._resolve('retry');
    await expect(pending).resolves.toBe('retry');
    expect(get(endpointOfflineStore)).toBeNull();
  });

  it('openAndWait resolves with cancel', async () => {
    const pending = endpointOfflineStore.openAndWait(samplePayload);
    endpointOfflineStore._resolve('cancel');
    await expect(pending).resolves.toBe('cancel');
  });

  it('openAndWait resolves with opened_settings', async () => {
    const pending = endpointOfflineStore.openAndWait(samplePayload);
    endpointOfflineStore._resolve('opened_settings');
    await expect(pending).resolves.toBe('opened_settings');
  });

  it('concurrent open resolves prior promise with the new decision', async () => {
    const first = endpointOfflineStore.openAndWait(samplePayload);
    const second = endpointOfflineStore.openAndWait({
      ...samplePayload,
      provider_name: 'LM Studio',
    });
    endpointOfflineStore._resolve('cancel');
    await expect(first).resolves.toBe('cancel');
    await expect(second).resolves.toBe('cancel');
  });

  it('close() clears state without resolving', () => {
    void endpointOfflineStore.openAndWait(samplePayload);
    endpointOfflineStore.close();
    expect(get(endpointOfflineStore)).toBeNull();
  });
});
