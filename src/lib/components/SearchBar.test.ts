// @vitest-environment jsdom
/**
 * SearchBar — debounce contract for the persistent Active search
 * (recording-date-calendar, batch 3).
 *
 * Facts pinned:
 *   - Typing updates the bindable value immediately and fires onSearch ONCE
 *     with the final value after the 300ms debounce.
 *   - A PROGRAMMATIC value change (Clear search / Clear filters writing the
 *     bound value) cancels any pending debounce — a clear can never be
 *     undone 300ms later by a stale callback. Normal typing afterwards
 *     debounces again.
 *   - Unmount (tab remount) cancels the pending debounce too.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';
import SearchBar from './SearchBar.svelte';

beforeEach(() => {
  vi.clearAllMocks();
  cleanup();
});
afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

describe('SearchBar — debounce', () => {
  it('updates the value immediately and fires onSearch once after 300ms', async () => {
    vi.useFakeTimers();
    const onSearch = vi.fn();
    const { rerender } = render(SearchBar, { value: '', onSearch });
    const input = screen.getByLabelText('Search recordings') as HTMLInputElement;

    await fireEvent.input(input, { target: { value: 'smi' } });
    await fireEvent.input(input, { target: { value: 'smith' } });
    expect(input.value).toBe('smith');
    expect(onSearch).not.toHaveBeenCalled();

    vi.advanceTimersByTime(299);
    expect(onSearch).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onSearch).toHaveBeenCalledTimes(1);
    expect(onSearch).toHaveBeenCalledWith('smith');

    // No behavioral change from an unrelated rerender (same value).
    await rerender({ value: 'smith', onSearch });
    vi.advanceTimersByTime(1000);
    expect(onSearch).toHaveBeenCalledTimes(1);
  });

  it('a programmatic clear cancels the pending debounce — the clear cannot be undone', async () => {
    vi.useFakeTimers();
    const onSearch = vi.fn();
    const { rerender } = render(SearchBar, { value: '', onSearch });
    const input = screen.getByLabelText('Search recordings') as HTMLInputElement;

    await fireEvent.input(input, { target: { value: 'zebra' } });
    // Programmatic clear (Clear search / Clear filters) rewrites the bound
    // value while the debounce is still pending.
    await rerender({ value: '', onSearch });
    expect(input.value).toBe('');

    vi.advanceTimersByTime(1000);
    expect(onSearch).not.toHaveBeenCalled();
  });

  it('after a programmatic change, fresh typing debounces normally', async () => {
    vi.useFakeTimers();
    const onSearch = vi.fn();
    const { rerender } = render(SearchBar, { value: '', onSearch });
    const input = screen.getByLabelText('Search recordings') as HTMLInputElement;

    await fireEvent.input(input, { target: { value: 'abandoned' } });
    await rerender({ value: 'kept', onSearch });
    vi.advanceTimersByTime(1000);
    expect(onSearch).not.toHaveBeenCalled();

    await fireEvent.input(input, { target: { value: 'typed' } });
    vi.advanceTimersByTime(300);
    expect(onSearch).toHaveBeenCalledTimes(1);
    expect(onSearch).toHaveBeenCalledWith('typed');
  });

  it('unmount cancels the pending debounce (remount cannot resurrect a cleared filter)', async () => {
    vi.useFakeTimers();
    const onSearch = vi.fn();
    const { unmount } = render(SearchBar, { onSearch });
    const input = screen.getByLabelText('Search recordings');

    await fireEvent.input(input, { target: { value: 'soon-stale' } });
    unmount();
    vi.advanceTimersByTime(1000);
    expect(onSearch).not.toHaveBeenCalled();
  });
});
