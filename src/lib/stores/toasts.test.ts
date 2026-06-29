import { describe, it, expect, beforeEach, vi } from 'vitest';
import { toasts } from './toasts.svelte';

describe('ToastStore', () => {
  beforeEach(() => {
    toasts.destroy();
  });

  it('starts empty', () => {
    expect(toasts.list).toHaveLength(0);
  });

  it('success adds an auto-dismissing toast', () => {
    const id = toasts.success('Saved');
    expect(toasts.list).toHaveLength(1);
    expect(toasts.list[0].message).toBe('Saved');
    expect(toasts.list[0].type).toBe('success');
    expect(toasts.list[0].autoDismiss).toBe(true);
    expect(toasts.list[0].id).toBe(id);
  });

  it('error adds a persisting toast', () => {
    toasts.error('Failed');
    expect(toasts.list).toHaveLength(1);
    expect(toasts.list[0].type).toBe('error');
    expect(toasts.list[0].autoDismiss).toBe(false);
  });

  it('dismiss removes a toast by id', () => {
    const id = toasts.success('one');
    toasts.error('two');
    expect(toasts.list).toHaveLength(2);
    toasts.dismiss(id);
    expect(toasts.list).toHaveLength(1);
    expect(toasts.list[0].message).toBe('two');
  });

  it('destroy clears all toasts', () => {
    toasts.success('a');
    toasts.error('b');
    toasts.success('c');
    toasts.destroy();
    expect(toasts.list).toHaveLength(0);
  });

  it('auto-dismiss fires after timeout', () => {
    vi.useFakeTimers();
    toasts.success('temporary');
    expect(toasts.list).toHaveLength(1);
    vi.advanceTimersByTime(8000);
    expect(toasts.list).toHaveLength(0);
    vi.useRealTimers();
  });

  it('manual dismiss cancels the auto-dismiss timer', () => {
    vi.useFakeTimers();
    const id = toasts.success('cancel me');
    toasts.dismiss(id);
    vi.advanceTimersByTime(8000);
    // Should still be empty (no double-fire)
    expect(toasts.list).toHaveLength(0);
    vi.useRealTimers();
  });
});
