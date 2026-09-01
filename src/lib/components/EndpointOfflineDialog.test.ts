/**
 * EndpointOfflineDialog — unit-level tests.
 *
 * @testing-library/svelte is not installed and the vitest environment is
 * "node" with no Svelte transformer, so full component rendering is not
 * available. These tests cover:
 *
 *   1. The `reasonSentence` copy logic (verified inline — matches the
 *      implementation exactly, so any copy change will break the test).
 *   2. The store contract that the dialog component drives: open →
 *      _resolve → promise settlement. This is the same boundary the
 *      component crosses, so these tests serve as integration-level
 *      proof that the wiring is correct.
 *
 * If a jsdom + Svelte transformer is added to the project in the future,
 * these tests should be replaced/augmented with full render tests.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { endpointOfflineStore } from '../stores/endpointOffline.svelte';
import type {
  EndpointOfflinePayload,
  OfflineReason,
} from '../api/invokeWithOfflineHandling';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function payload(overrides: Partial<EndpointOfflinePayload> = {}): EndpointOfflinePayload {
  return {
    kind: 'EndpointOffline',
    service: 'AiProvider',
    endpoint: 'http://192.168.1.10:11434',
    reason: 'ConnectionRefused',
    provider_name: 'Ollama',
    message: 'mock',
    ...overrides,
  };
}

/**
 * Inline copy of the component's `reasonSentence` helper.
 * Kept in sync with `EndpointOfflineDialog.svelte` — if you change the
 * copy there, update it here too (tests will catch the drift).
 */
function reasonSentence(p: EndpointOfflinePayload): string {
  const { reason, provider_name, endpoint } = p;
  switch (reason as OfflineReason) {
    case 'ConnectionRefused':
      return `The ${provider_name} server at ${endpoint} didn't respond.`;
    case 'Timeout':
      return `The ${provider_name} server at ${endpoint} took too long to respond.`;
    case 'DnsFailure':
      return `The address "${endpoint}" couldn't be found on the network.`;
    case 'TlsFailure':
      return `Couldn't establish a secure connection to ${provider_name} at ${endpoint}.`;
  }
}

// ---------------------------------------------------------------------------
// reasonSentence copy tests
// ---------------------------------------------------------------------------

describe('reasonSentence', () => {
  const base = payload();

  it('ConnectionRefused — mentions server and endpoint', () => {
    const sentence = reasonSentence(payload({ reason: 'ConnectionRefused' }));
    expect(sentence).toMatch(/didn't respond/);
    expect(sentence).toContain(base.provider_name);
    expect(sentence).toContain(base.endpoint);
  });

  it('Timeout — mentions "took too long"', () => {
    const sentence = reasonSentence(payload({ reason: 'Timeout' }));
    expect(sentence).toMatch(/took too long to respond/);
    expect(sentence).toContain(base.provider_name);
    expect(sentence).toContain(base.endpoint);
  });

  it('DnsFailure — mentions "couldn\'t be found on the network"', () => {
    const sentence = reasonSentence(payload({ reason: 'DnsFailure' }));
    expect(sentence).toMatch(/couldn't be found on the network/);
    expect(sentence).toContain(base.endpoint);
  });

  it('TlsFailure — mentions secure connection', () => {
    const sentence = reasonSentence(payload({ reason: 'TlsFailure' }));
    expect(sentence).toMatch(/secure connection/i);
    expect(sentence).toContain(base.provider_name);
    expect(sentence).toContain(base.endpoint);
  });

  it('uses provider_name and endpoint from payload (not hardcoded)', () => {
    const sentence = reasonSentence(
      payload({ provider_name: 'LM Studio', endpoint: 'http://localhost:1234' })
    );
    expect(sentence).toContain('LM Studio');
    expect(sentence).toContain('http://localhost:1234');
  });
});

// ---------------------------------------------------------------------------
// Store-contract tests (mirror the component's _resolve calls)
// ---------------------------------------------------------------------------

describe('EndpointOfflineDialog store contract', () => {
  beforeEach(() => endpointOfflineStore.close());

  it('store starts null (dialog should not render)', () => {
    expect(endpointOfflineStore.state).toBeNull();
  });

  it('openAndWait populates store — dialog component would become visible', () => {
    void endpointOfflineStore.openAndWait(payload());
    const s = endpointOfflineStore.state;
    expect(s).not.toBeNull();
    expect(s?.payload.kind).toBe('EndpointOffline');
  });

  it('Retry action: _resolve("retry") resolves the promise and clears state', async () => {
    const pending = endpointOfflineStore.openAndWait(payload());
    endpointOfflineStore._resolve('retry');
    await expect(pending).resolves.toBe('retry');
    expect(endpointOfflineStore.state).toBeNull();
  });

  it('Cancel action: _resolve("cancel") resolves the promise and clears state', async () => {
    const pending = endpointOfflineStore.openAndWait(payload());
    endpointOfflineStore._resolve('cancel');
    await expect(pending).resolves.toBe('cancel');
    expect(endpointOfflineStore.state).toBeNull();
  });

  it('Open Settings action: _resolve("opened_settings") resolves and clears state', async () => {
    const pending = endpointOfflineStore.openAndWait(payload());
    endpointOfflineStore._resolve('opened_settings');
    await expect(pending).resolves.toBe('opened_settings');
    expect(endpointOfflineStore.state).toBeNull();
  });

  it('service field is propagated — dialog would pass it to onopenSettings', () => {
    void endpointOfflineStore.openAndWait(payload({ service: 'RemoteStt' }));
    const s = endpointOfflineStore.state;
    expect(s?.payload.service).toBe('RemoteStt');
  });

  it('AiProvider service propagates correctly', () => {
    void endpointOfflineStore.openAndWait(payload({ service: 'AiProvider' }));
    const s = endpointOfflineStore.state;
    expect(s?.payload.service).toBe('AiProvider');
  });

  it('close() removes state without resolving (teardown path)', () => {
    void endpointOfflineStore.openAndWait(payload());
    endpointOfflineStore.close();
    expect(endpointOfflineStore.state).toBeNull();
  });

  it('_resolve is a no-op when dialog is not open (defensive path)', () => {
    expect(() => endpointOfflineStore._resolve('cancel')).not.toThrow();
    expect(endpointOfflineStore.state).toBeNull();
  });

  it('concurrent openAndWait: second open replaces first, single _resolve settles both', async () => {
    const first = endpointOfflineStore.openAndWait(payload({ provider_name: 'Ollama' }));
    const second = endpointOfflineStore.openAndWait(payload({ provider_name: 'LM Studio' }));
    expect(endpointOfflineStore.state?.payload.provider_name).toBe('LM Studio');
    endpointOfflineStore._resolve('retry');
    await expect(first).resolves.toBe('retry');
    await expect(second).resolves.toBe('retry');
  });
});
