// @vitest-environment jsdom
/**
 * Toast auto-dismiss pause/resume (trash-restore D9) + announcement roles.
 *
 * An actionable toast (Undo/View) pauses its auto-dismiss while focused or
 * hovered — the button can't vanish mid-reach — and resumes with the
 * REMAINING budget, not a fresh 8 s. Persistent error toasts have no timer
 * to pause (no-op).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { render, screen, cleanup, fireEvent } from '@testing-library/svelte';
import ToastContainer from '../components/ToastContainer.svelte';
import { toasts } from './toasts.svelte';

beforeEach(() => {
  cleanup();
  toasts.destroy();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('toasts — pause/resume', () => {
  it('an actionable toast survives its auto-dismiss window while paused', async () => {
    render(ToastContainer);
    toasts.add({
      message: '3 recordings moved to Trash. Available to restore for 30 days.',
      type: 'success',
      autoDismiss: true,
      actionLabel: 'Undo',
      onAction: () => {},
    });
    await tick();

    const toastEl = screen.getByText(/moved to Trash/).closest('.toast') as HTMLElement;
    // Real focus fires a bubbling focusin (what the container listens for).
    const undoBtn = toastEl.querySelector('.toast-btn-view') as HTMLElement;
    undoBtn.focus();

    vi.advanceTimersByTime(20_000);
    expect(toasts.list).toHaveLength(1);

    // Blur → resume with the remaining budget (well under 8 s now).
    undoBtn.blur();
    vi.advanceTimersByTime(9_000);
    expect(toasts.list).toHaveLength(0);
  });

  it('pausing a persistent error toast is a no-op', () => {
    render(ToastContainer);
    const id = toasts.error('Something failed');
    toasts.pause(id);
    vi.advanceTimersByTime(60_000);
    expect(toasts.list).toHaveLength(1);
    toasts.dismiss(id);
  });

  it('resume without a prior pause does not double-arm', () => {
    render(ToastContainer);
    const id = toasts.success('plain success');
    toasts.resume(id); // no-op guard
    vi.advanceTimersByTime(8_500);
    expect(toasts.list).toHaveLength(0);
  });
});

describe('toasts — announcement roles', () => {
  it('success toasts are polite (role=status); error toasts are alerts', async () => {
    render(ToastContainer);
    toasts.success('2 recordings restored to Active.');
    toasts.error('Could not reach the sync server');
    await tick();

    expect(screen.getByRole('status').textContent).toContain('restored to Active');
    expect(screen.getByRole('alert').textContent).toContain('sync server');
  });

  it('hovering an actionable toast also pauses dismissal', async () => {
    render(ToastContainer);
    toasts.add({
      message: 'Recording moved to Trash.',
      type: 'success',
      autoDismiss: true,
      actionLabel: 'Undo',
      onAction: () => {},
    });
    await tick();
    const toastEl = screen.getByText('Recording moved to Trash.').closest('.toast') as HTMLElement;
    // testing-library has no fireEvent.mouseenter helper — dispatch it
    // directly (the container binds the event on the element).
    fireEvent(toastEl, new MouseEvent('mouseenter'));
    vi.advanceTimersByTime(20_000);
    expect(toasts.list).toHaveLength(1);
    fireEvent(toastEl, new MouseEvent('mouseleave'));
    vi.advanceTimersByTime(9_000);
    expect(toasts.list).toHaveLength(0);
  });
});
