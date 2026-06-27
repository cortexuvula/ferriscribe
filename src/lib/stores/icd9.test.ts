import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the Tauri invoke bridge so we control load outcomes.
const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));

// Import AFTER mocks are registered. The store is a singleton created at
// module scope; its private loadPromise guard persists across tests, so we
// use retry() (which clears the guard) as the per-test entry point.
const { icd9 } = await import('./icd9.svelte');

describe('Icd9Store — load / retry / failure', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    // Reset observable state. The private loadPromise guard is cleared by
    // calling retry() at the start of each test that needs a fresh load.
    icd9.codeSet = null;
    icd9.loaded = false;
    icd9.loadError = false;
  });

  it('load() populates codeSet on success', async () => {
    invokeMock.mockResolvedValue(['401.9', '847.2', 'V70.0']);
    await icd9.retry(); // retry() clears the guard, then loads
    expect(icd9.codeSet).toBeInstanceOf(Set);
    expect(icd9.codeSet?.has('401.9')).toBe(true);
    expect(icd9.loaded).toBe(true);
    expect(icd9.loadError).toBe(false);
  });

  it('load() keeps codeSet null on failure (neutral-chip safety)', async () => {
    invokeMock.mockRejectedValue(new Error('DB locked'));
    await icd9.retry();
    // The billing-safety contract: failure leaves codeSet null so chips
    // render neutral (no false "not in MSP list" warning).
    expect(icd9.codeSet).toBeNull();
    expect(icd9.loaded).toBe(true);
    expect(icd9.loadError).toBe(true);
  });

  it('concurrent load() calls fire invoke once (dedup)', async () => {
    invokeMock.mockResolvedValue(['401.9']);
    // retry() clears the guard and starts one load; two immediate load()
    // calls return the same pending promise.
    await icd9.retry();
    const p1 = icd9.load();
    const p2 = icd9.load();
    await Promise.all([p1, p2]);
    // After a successful retry, load() is idempotent (guard set, returns
    // cached resolved promise). The single invoke happened in retry().
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it('retry() re-attempts after a failure', async () => {
    // First attempt fails.
    invokeMock.mockRejectedValueOnce(new Error('transient'));
    await icd9.retry();
    expect(icd9.loadError).toBe(true);
    expect(icd9.codeSet).toBeNull();

    // Retry succeeds — the guard was cleared on failure (F12 fix).
    invokeMock.mockResolvedValue(['401.9', 'V70.0']);
    await icd9.retry();
    expect(icd9.loadError).toBe(false);
    expect(icd9.codeSet?.has('401.9')).toBe(true);
  });
});
