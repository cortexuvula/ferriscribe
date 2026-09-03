import { describe, it, expect, beforeEach } from 'vitest';
import { confirmStore, confirmDialog } from './confirm.svelte';

describe('confirmDialog service', () => {
  beforeEach(() => {
    // Dismiss any dialog left open by a previous test.
    confirmStore.settle(false);
  });

  it('opens with the given options and resolves true on confirm', async () => {
    const promise = confirmDialog({ title: 'Delete?', message: 'Really?', danger: true });
    expect(confirmStore.state.open).toBe(true);
    expect(confirmStore.state.options?.title).toBe('Delete?');
    confirmStore.settle(true);
    await expect(promise).resolves.toBe(true);
    expect(confirmStore.state.open).toBe(false);
    expect(confirmStore.state.options).toBeNull();
  });

  it('resolves false on cancel', async () => {
    const promise = confirmDialog({ title: 'T', message: 'M' });
    confirmStore.settle(false);
    await expect(promise).resolves.toBe(false);
  });

  it('a superseded dialog resolves false so no caller hangs', async () => {
    const first = confirmDialog({ title: 'First', message: 'M' });
    const second = confirmDialog({ title: 'Second', message: 'M' });
    expect(confirmStore.state.options?.title).toBe('Second');
    await expect(first).resolves.toBe(false);
    confirmStore.settle(true);
    await expect(second).resolves.toBe(true);
  });
});
