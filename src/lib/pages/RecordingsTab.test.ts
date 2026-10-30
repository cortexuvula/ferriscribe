// @vitest-environment jsdom
/**
 * RecordingsTab — Active view wiring for the recording-date calendar
 * (recording-date-calendar, batch 3), plus the pre-existing Trash policy
 * copy pins.
 *
 * Facts pinned:
 *   - The date calendar (REAL component) sits beside the search bar; picking
 *     a day calls store.setSelectedDate; the `Recorded on <date>` chip's
 *     Clear button calls setSelectedDate(null).
 *   - Search rides the SearchBar debounce into store.search; a programmatic
 *     Clear search cancels the pending debounce (cannot be resurrected).
 *   - Distinct zero states (date+search / date-only / search-only / truly
 *     empty) with INDEPENDENT clear actions.
 *   - First-page listError → explicit Retry; loadMoreError keeps valid rows
 *     and offers Retry.
 *   - Global Move all stays visible with activeTotal>0 even under a
 *     zero-match filter, and the dialog names BOTH filters.
 *   - Returning from Trash rides onActiveViewReturn; mount does not.
 *   - dateClearedNotice renders as a polite status once, dismissable.
 *   - The outside-date notice appears ONLY when provable from the selected
 *     recording's created_at local day.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import RecordingsTab from './RecordingsTab.svelte';
import type { Recording, RecordingSummary } from '../types';
import { fullLocalDateFromKey, parseLocalDateKey } from '../utils/recordingDates';

const { store, settings, toasts } = vi.hoisted(() => ({
  store: {
    list: [] as RecordingSummary[], loading: false, loadingMore: false, hasMore: false,
    listError: null as string | null, loadMoreError: null as string | null,
    searchQuery: '', selectedDate: null as string | null,
    dateClearedNotice: null as string | null,
    selectedRecording: null as Recording | null,
    availabilityCounts: new Map<string, number>(), availabilityLoading: false,
    availabilityError: null as string | null, availabilityLoaded: false,
    activeTotal: 4, trashedTotal: 0,
    trashedList: [] as never[], trashedLoading: false, trashedLoadingMore: false,
    trashedHasMore: false, trashedError: null as string | null, restoring: false,
    restoreTrashed: vi.fn(), restoreAllFromTrash: vi.fn(),
    countTrashedOnDate: vi.fn(), restoreTrashedOnDate: vi.fn(),
    load: vi.fn(), loadMore: vi.fn(), search: vi.fn(), setSelectedDate: vi.fn(),
    loadTrashed: vi.fn(), onActiveViewReturn: vi.fn(), refreshAvailability: vi.fn(),
    dismissDateClearedNotice: vi.fn(),
    refreshTrashedTotal: vi.fn(), refreshActiveTotal: vi.fn(),
    removeAll: vi.fn(), remove: vi.fn(), undoMoveAll: vi.fn(), restore: vi.fn(),
  },
  settings: { trashRetentionDays: 30 },
  toasts: { add: vi.fn(), success: vi.fn(), error: vi.fn() },
}));
vi.mock('../stores/recordings.svelte', () => ({ recordings: store, selectRecording: vi.fn() }));
vi.mock('../stores/settings.svelte', () => ({ settings }));
vi.mock('../stores/toasts.svelte', () => ({ toasts }));
vi.mock('../stores/pipeline.svelte', () => ({ pipeline: { retry: vi.fn() } }));
// The REAL RecordingDateCalendar and TrashPanel ride along unmocked — the
// tab tests exercise the actual integration surfaces.

function makeSummary(id: string): RecordingSummary {
  return {
    id, filename: `${id}.wav`, patient_name: null,
    status: { status: 'pending' }, duration_seconds: 60, created_at: '2026-10-01T12:00:00Z',
    tags: [], has_transcript: false, has_soap_note: false, has_referral: false,
    has_letter: false, has_peer_discussion: false, is_remote: false, tokens_per_second: null,
  };
}

/// A timestamp at LOCAL NOON of the given day key — keys back to exactly
/// that day in any runtime zone (noon ± any real offset stays same-day).
function noonIso(key: string): string {
  return new Date(parseLocalDateKey(key).getTime() + 12 * 3_600_000).toISOString();
}

function dayButtonName(key: string, n: number): string {
  return `${fullLocalDateFromKey(key)}, ${n} recording${n === 1 ? '' : 's'}`;
}

beforeEach(() => {
  vi.clearAllMocks();
  settings.trashRetentionDays = 30;
  store.list = [makeSummary('synthetic-recording')];
  store.loading = false;
  store.loadingMore = false;
  store.hasMore = false;
  store.listError = null;
  store.loadMoreError = null;
  store.searchQuery = '';
  store.selectedDate = null;
  store.dateClearedNotice = null;
  store.selectedRecording = null;
  store.availabilityCounts = new Map();
  store.availabilityLoading = false;
  store.availabilityError = null;
  store.availabilityLoaded = true;
  store.activeTotal = 4;
  store.trashedList = [];
  store.trashedTotal = 0;
  store.trashedLoading = false;
  store.trashedLoadingMore = false;
  store.trashedHasMore = false;
  store.trashedError = null;
  store.restoring = false;
  store.removeAll.mockResolvedValue({ count: 4, ids: ['synthetic-recording'] });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe('RecordingsTab — Trash policy copy', () => {
  it.each([7, 14, 21, 30])('uses the %i-day window in move-all confirmation and toast', async days => {
    settings.trashRetentionDays = days;
    render(RecordingsTab);
    await fireEvent.click(screen.getByRole('button', { name: 'Move all to Trash' }));
    const dialog = screen.getByRole('alertdialog');
    expect(dialog.textContent).toContain(`eligible for permanent deletion after ${days} days in Trash`);
    expect(dialog.textContent).toContain('including recordings outside the selected date and current search');
    expect(dialog.textContent).not.toContain(`After ${days} days, they are permanently deleted`);
    await fireEvent.click(screen.getByRole('button', { name: 'Move 4 recordings to Trash' }));
    await waitFor(() => expect(toasts.add).toHaveBeenCalledWith(expect.objectContaining({
      message: `4 moved to Trash. Restore them within ${days} days.`,
      actionLabel: 'Undo', type: 'success',
    })));
    expect(store.removeAll).toHaveBeenCalledTimes(1);
  });

  it('uses the canonical eligibility policy in the single-recording confirmation', async () => {
    settings.trashRetentionDays = 14;
    render(RecordingsTab);
    await fireEvent.click(screen.getByRole('button', { name: 'Delete synthetic-recording.wav' }));
    expect(screen.getByRole('alertdialog').textContent).toContain('eligible for permanent deletion after 14 days in Trash');
  });
});

describe('RecordingsTab — calendar wiring', () => {
  it('picking a populated day routes through store.setSelectedDate', async () => {
    store.availabilityCounts = new Map([['2026-10-05', 2]]);
    render(RecordingsTab);
    await fireEvent.click(screen.getByRole('button', { name: 'Choose recording date' }));
    // Opening the calendar refreshes availability (timezone rederive).
    expect(store.refreshAvailability).toHaveBeenCalledTimes(1);
    await fireEvent.click(screen.getByRole('button', { name: dayButtonName('2026-10-05', 2) }));
    expect(store.setSelectedDate).toHaveBeenCalledTimes(1);
    expect(store.setSelectedDate).toHaveBeenCalledWith('2026-10-05');
  });

  it('selected date renders the chip; its Clear button clears ONLY the date', async () => {
    store.selectedDate = '2026-10-05';
    render(RecordingsTab);
    expect(
      screen.getByText(`Recorded on ${fullLocalDateFromKey('2026-10-05')}`),
    ).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Clear recording date' }));
    expect(store.setSelectedDate).toHaveBeenCalledWith(null);
    expect(store.setSelectedDate).toHaveBeenCalledTimes(1);
    // Clearing the date never touches the search.
    expect(store.search).not.toHaveBeenCalled();
  });
});

describe('RecordingsTab — search wiring', () => {
  it('typing debounces into store.search', async () => {
    vi.useFakeTimers();
    render(RecordingsTab);
    const input = screen.getByLabelText('Search recordings') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'flu' } });
    await act(() => { vi.advanceTimersByTime(300); });
    expect(store.search).toHaveBeenCalledTimes(1);
    expect(store.search).toHaveBeenCalledWith('flu');
  });

  it('the search input shows the persistent query on remount', () => {
    store.searchQuery = 'persistent-term';
    render(RecordingsTab);
    expect((screen.getByLabelText('Search recordings') as HTMLInputElement).value).toBe(
      'persistent-term',
    );
  });

  it('Clear search cancels a pending debounce — the clear cannot be undone', async () => {
    vi.useFakeTimers();
    store.searchQuery = 'zebra';
    store.list = []; // the completed search matched nothing → Clear search offered
    render(RecordingsTab);
    const input = screen.getByLabelText('Search recordings') as HTMLInputElement;
    // The user keeps typing after the zero state appeared…
    await fireEvent.input(input, { target: { value: 'zebrax' } });
    // …then clicks Clear search while that debounce is still pending.
    await fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
    expect(store.search).toHaveBeenCalledTimes(1);
    expect(store.search).toHaveBeenCalledWith('');
    expect((screen.getByLabelText('Search recordings') as HTMLInputElement).value).toBe('');
    await act(() => { vi.advanceTimersByTime(1000); });
    // The stale 'zebrax' callback never fired.
    expect(store.search).toHaveBeenCalledTimes(1);
  });
});

describe('RecordingsTab — distinct zero states', () => {
  it('date + search zero names both filters and offers independent clears', async () => {
    store.list = [];
    store.searchQuery = 'zebra';
    store.selectedDate = '2026-10-05';
    render(RecordingsTab);
    expect(screen.getByText('No recordings on this date match your search.')).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
    expect(store.search).toHaveBeenCalledWith('');
    await fireEvent.click(screen.getByRole('button', { name: 'Show all dates' }));
    expect(store.setSelectedDate).toHaveBeenCalledWith(null);
  });

  it('date-only zero offers a date clear only', () => {
    store.list = [];
    store.selectedDate = '2026-10-05';
    render(RecordingsTab);
    expect(screen.getByText('No recordings on this date.')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Clear search' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Show all dates' })).toBeTruthy();
  });

  it('search-only zero offers Clear search only', () => {
    store.list = [];
    store.searchQuery = 'zebra';
    render(RecordingsTab);
    expect(screen.getByText('No recordings match your search.')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Clear recording date' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Clear search' })).toBeTruthy();
  });

  it('no filters + authoritative activeTotal 0 is the true empty state', () => {
    store.list = [];
    store.activeTotal = 0;
    render(RecordingsTab);
    expect(screen.getByText('No recordings yet.')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Move all to Trash' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Clear search' })).toBeNull();
  });

  it('a filtered zero still shows the filtered-vs-total count and global Move all', async () => {
    store.list = [];
    store.searchQuery = 'zebra';
    store.selectedDate = '2026-10-05';
    render(RecordingsTab);
    expect(screen.getByText('0 of 4 recordings')).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Move all to Trash' }));
    const dialog = screen.getByRole('alertdialog');
    expect(dialog.textContent).toContain('Move all 4 recordings to Trash?');
    expect(dialog.textContent).toContain('including recordings outside the selected date and current search');
  });
});

describe('RecordingsTab — list errors vs empty', () => {
  it('first-page failure renders an explicit Retry instead of rows or fake empty', async () => {
    store.listError = 'unexpected error';
    render(RecordingsTab);
    expect(screen.getByText("Couldn't load recordings.")).toBeTruthy();
    expect(screen.queryByText('No recordings yet.')).toBeNull();
    expect(screen.queryByText('synthetic-recording.wav')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(store.load).toHaveBeenCalled();
  });

  it('load-more failure keeps the valid rows and offers Retry', async () => {
    store.hasMore = true;
    store.loadMoreError = 'unexpected error';
    render(RecordingsTab);
    expect(screen.getByText('synthetic-recording.wav')).toBeTruthy();
    expect(screen.getByText("Couldn't load more recordings.")).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(store.loadMore).toHaveBeenCalledTimes(1);
    // The Load more affordance itself stays.
    expect(screen.getByRole('button', { name: 'Load more' })).toBeTruthy();
  });
});

describe('RecordingsTab — view switching', () => {
  it('mount loads the Active list without onActiveViewReturn; Trash entry loads page 1; Active RETURN refreshes', async () => {
    render(RecordingsTab);
    expect(store.load).toHaveBeenCalledTimes(1);
    expect(store.loadTrashed).not.toHaveBeenCalled();
    expect(store.onActiveViewReturn).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByRole('tab', { name: 'Trash' }));
    expect(store.loadTrashed).toHaveBeenCalledTimes(1);

    await fireEvent.click(screen.getByRole('tab', { name: 'Active' }));
    expect(store.onActiveViewReturn).toHaveBeenCalledTimes(1);
  });
});

describe('RecordingsTab — notices', () => {
  it('dateClearedNotice renders once as a polite status and is dismissable', async () => {
    store.dateClearedNotice =
      `No active recordings remain on ${fullLocalDateFromKey('2026-10-05')}. Showing all dates.`;
    render(RecordingsTab);
    const status = screen.getByRole('status');
    expect(status.textContent).toContain('Showing all dates');
    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(store.dismissDateClearedNotice).toHaveBeenCalledTimes(1);
  });

  it('outside-date notice appears ONLY when provable from the selected recording\'s local day', () => {
    store.selectedDate = '2026-10-05';
    store.selectedRecording = { id: 'other-day', created_at: noonIso('2026-10-04') } as never;
    const { unmount } = render(RecordingsTab);
    expect(screen.getByText('Selected recording is outside the selected date.')).toBeTruthy();
    unmount();
    cleanup();

    // Same day → provably inside → no notice.
    store.selectedRecording = { id: 'same-day', created_at: noonIso('2026-10-05') } as never;
    const { unmount: u2 } = render(RecordingsTab);
    expect(screen.queryByText('Selected recording is outside the selected date.')).toBeNull();
    u2();
    cleanup();

    // No selection loaded → nothing provable → no notice (never inferred
    // from absence in a paginated list).
    store.selectedRecording = null;
    render(RecordingsTab);
    expect(screen.queryByText('Selected recording is outside the selected date.')).toBeNull();
  });
});
