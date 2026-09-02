import { describe, expect, it, vi } from 'vitest';
import { useTestConnection } from './useTestConnection.svelte';

describe('useTestConnection', () => {
  it('walks idle → testing → success with the test message', async () => {
    const t = useTestConnection();
    expect(t.status).toBe('idle');

    const pending = t.run(async () => '3 models visible');
    expect(t.status).toBe('testing');
    await pending;
    expect(t.status).toBe('success');
    expect(t.message).toBe('3 models visible');
  });

  it('maps a rejected test to error with a formatted message', async () => {
    const t = useTestConnection();
    await t.run(async () => {
      throw new Error('Ollama at http://x is offline');
    });
    expect(t.status).toBe('error');
    expect(t.message).toContain('offline');
  });

  it('ignores re-entrant runs while a test is in flight', async () => {
    const t = useTestConnection();
    const inner = vi.fn(async () => 'first');
    const second = vi.fn(async () => 'second');
    const first = t.run(inner);
    await t.run(second); // ignored — status is 'testing'
    await first;
    expect(inner).toHaveBeenCalledTimes(1);
    expect(second).not.toHaveBeenCalled();
    expect(t.message).toBe('first');
  });

  it('reset clears status and message', async () => {
    const t = useTestConnection();
    await t.run(async () => 'ok');
    t.reset();
    expect(t.status).toBe('idle');
    expect(t.message).toBe('');
  });
});
