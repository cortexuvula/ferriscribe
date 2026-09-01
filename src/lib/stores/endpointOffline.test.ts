import { describe, it, expect, beforeEach } from 'vitest';
import { endpointOfflineStore } from './endpointOffline.svelte';
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
    expect(endpointOfflineStore.state).toBeNull();
  });

  it('openAndWait populates state with the payload', () => {
    void endpointOfflineStore.openAndWait(samplePayload);
    const s = endpointOfflineStore.state;
    expect(s).not.toBeNull();
    expect(s?.payload).toEqual(samplePayload);
  });

  it('openAndWait resolves with retry when _resolve("retry") is called', async () => {
    const pending = endpointOfflineStore.openAndWait(samplePayload);
    endpointOfflineStore._resolve('retry');
    await expect(pending).resolves.toBe('retry');
    expect(endpointOfflineStore.state).toBeNull();
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

  it('concurrent open chains the prior resolver into the new one', async () => {
    const first = endpointOfflineStore.openAndWait(samplePayload);

    // The second open replaces the active dialog. The prior resolver
    // must be chained into the new one so the first promise still
    // settles when the user picks an action.
    const second = endpointOfflineStore.openAndWait({
      ...samplePayload,
      provider_name: 'LM Studio',
    });

    // Verify the store state reflects the SECOND payload (it overrode the first).
    expect(endpointOfflineStore.state?.payload.provider_name).toBe('LM Studio');

    // Now resolve once — both promises must settle with the same decision,
    // proving the prior resolver was chained.
    endpointOfflineStore._resolve('retry');
    await expect(first).resolves.toBe('retry');
    await expect(second).resolves.toBe('retry');
  });

  it('_resolve is a no-op when no dialog is open', () => {
    // No openAndWait called; state is null.
    expect(() => endpointOfflineStore._resolve('cancel')).not.toThrow();
    expect(endpointOfflineStore.state).toBeNull();
  });

  it('close() clears state without resolving', () => {
    void endpointOfflineStore.openAndWait(samplePayload);
    endpointOfflineStore.close();
    expect(endpointOfflineStore.state).toBeNull();
  });
});
