// @vitest-environment jsdom
/**
 * RecordingViewSwitch — accessible Active | Trash tabs.
 *
 * Markup facts these tests rely on:
 *   - Container is <div role="tablist" aria-label="Recordings views">.
 *   - Two <button role="tab"> with ids view-tab-active / view-tab-trash,
 *     aria-selected, aria-controls, and a roving tabindex (selected tab 0,
 *     the other -1).
 *   - The Trash tab shows "Trash (N)" when N > 0, plain "Trash" at 0 — the
 *     switch renders regardless of either list's contents (an empty Active
 *     view must never hide the route to recovery).
 *   - ArrowLeft/ArrowRight on a focused tab switch views (and focus follows).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';
import RecordingViewSwitch from './RecordingViewSwitch.svelte';

beforeEach(cleanup);

describe('RecordingViewSwitch', () => {
  it('renders both tabs with tablist semantics and roving tabindex', () => {
    render(RecordingViewSwitch, { view: 'active', trashCount: 3, onChange: () => {} });

    const list = screen.getByRole('tablist', { name: 'Recordings views' });
    expect(list).toBeTruthy();

    const active = screen.getByRole('tab', { name: 'Active' });
    const trash = screen.getByRole('tab', { name: 'Trash (3)' });
    expect(active.getAttribute('aria-selected')).toBe('true');
    expect(active.tabIndex).toBe(0);
    expect(trash.getAttribute('aria-selected')).toBe('false');
    expect(trash.tabIndex).toBe(-1);
    expect(trash.getAttribute('aria-controls')).toBe('view-panel-trash');
  });

  it('shows the authoritative trashed count in the badge, plain Trash at 0', () => {
    const { rerender } = render(RecordingViewSwitch, {
      view: 'active',
      trashCount: 0,
      onChange: () => {},
    });
    expect(screen.getByRole('tab', { name: 'Trash' })).toBeTruthy();

    rerender({ view: 'active', trashCount: 12, onChange: () => {} });
    expect(screen.getByRole('tab', { name: 'Trash (12)' })).toBeTruthy();
  });

  it('clicking a tab switches the view', async () => {
    const onChange = vi.fn();
    render(RecordingViewSwitch, { view: 'active', trashCount: 1, onChange });

    await fireEvent.click(screen.getByRole('tab', { name: 'Trash (1)' }));
    expect(onChange).toHaveBeenCalledWith('trash');
  });

  it('ArrowRight/ArrowLeft on a tab switches the view', async () => {
    const onChange = vi.fn();
    render(RecordingViewSwitch, { view: 'active', trashCount: 0, onChange });
    const active = screen.getByRole('tab', { name: 'Active' });
    active.focus();

    await fireEvent.keyDown(active, { key: 'ArrowRight' });
    expect(onChange).toHaveBeenCalledWith('trash');

    await fireEvent.keyDown(active, { key: 'ArrowLeft' });
    expect(onChange).toHaveBeenCalledWith('trash');
    expect(onChange).not.toHaveBeenCalledWith('active');
  });

  it('renders even when the trash count is 0 and Active is empty (no list props at all)', () => {
    render(RecordingViewSwitch, { view: 'active', trashCount: 0, onChange: () => {} });
    expect(screen.getByRole('tab', { name: 'Active' })).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'Trash' })).toBeTruthy();
  });
});
