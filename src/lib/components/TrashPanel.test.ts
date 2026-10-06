// @vitest-environment jsdom
/**
 * TrashPanel — the Trash view surface.
 *
 * The store (../stores/recordings.svelte) is mocked with a plain object we
 * mutate per test; TrashPanel reads it directly. Facts pinned:
 *   - Persistent 30-day explanation line is always rendered.
 *   - Authoritative "N recordings in Trash" toolbar count (from
 *     trashedTotal, not the loaded page).
 *   - Four states: loading ("Loading Trash…"), error ("Couldn't load Trash.
 *     Try again." + Retry — NEVER an empty-trash message), empty ("Trash is
 *     empty." + hint), search-empty ("No recordings in Trash match your
 *     search." + Clear search).
 *   - Filtered view shows "N matches · M in Trash" with the authoritative M.
 *   - ONE polite role="status" region (result announcements), plus a
 *     focusable Trash heading (focus fallback for vanished rows).
 *   - Per-row restore calls store.restoreTrashed([id]) and announces.
 *   - Restore-failure toasts are SANITIZED (PHI never reaches the glass):
 *     generic value for raw backend strings, the safe `recording <uuid>`
 *     reference kept when the message carries one.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import TrashPanel from './TrashPanel.svelte';
import type { TrashedRecordingSummary } from '../types';

// ── Store + toasts mocks ───────────────────────────────────────────────────
// vi.mock factories are hoisted above every const, so the store object is
// created inside vi.hoisted and shared through the mocks holder.
const { fakeStore, mockRestoreTrashed, mockLoadTrashed, mockToastsError } = vi.hoisted(() => {
  const fakeStore = {
    trashedList: [] as import('../types').TrashedRecordingSummary[],
    trashedTotal: 0,
    trashedLoading: false,
    trashedLoadingMore: false,
    trashedHasMore: false,
    trashedError: null as string | null,
    restoring: false,
    loadTrashed: vi.fn(),
    loadMoreTrashed: vi.fn(),
    restoreTrashed: vi.fn(),
    restoreAllFromTrash: vi.fn(),
  };
  return {
    fakeStore,
    mockRestoreTrashed: fakeStore.restoreTrashed,
    mockLoadTrashed: fakeStore.loadTrashed,
    mockToastsError: vi.fn(),
  };
});

function makeTrashed(id: string, deletedAt = '2026-10-01T12:00:00Z'): TrashedRecordingSummary {
  return {
    id,
    filename: `${id}.wav`,
    patient_name: null,
    duration_seconds: 60,
    created_at: '2026-09-01T10:00:00Z',
    deleted_at: deletedAt,
  };
}

vi.mock('../stores/recordings.svelte', () => ({ recordings: fakeStore }));
vi.mock('../stores/toasts.svelte', () => ({
  toasts: { add: vi.fn(), error: mockToastsError, success: vi.fn(), dismiss: vi.fn() },
}));

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  fakeStore.trashedList = [];
  fakeStore.trashedTotal = 0;
  fakeStore.trashedLoading = false;
  fakeStore.trashedLoadingMore = false;
  fakeStore.trashedHasMore = false;
  fakeStore.trashedError = null;
  fakeStore.restoring = false;
});

describe('TrashPanel — states', () => {
  it('always shows the persistent 30-day explanation', () => {
    render(TrashPanel);
    expect(
      screen.getByText(
        'Recordings stay in Trash for 30 days. After that, their audio, transcripts, SOAP notes, and generated documents are permanently deleted.',
      ),
    ).toBeTruthy();
  });

  it('loading state renders "Loading Trash…"', () => {
    fakeStore.trashedLoading = true;
    render(TrashPanel);
    expect(screen.getByText('Loading Trash…')).toBeTruthy();
  });

  it('error state renders the retry copy + Retry — never an empty-trash look', () => {
    fakeStore.trashedError = 'db locked';
    render(TrashPanel);
    expect(screen.getByText("Couldn't load Trash. Try again.")).toBeTruthy();
    expect(screen.queryByText('Trash is empty.')).toBeNull();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeTruthy();
  });

  it('Retry re-invokes loadTrashed', async () => {
    fakeStore.trashedError = 'db locked';
    render(TrashPanel);
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(mockLoadTrashed).toHaveBeenCalled();
  });

  it('empty state renders the empty copy + hint', () => {
    render(TrashPanel);
    expect(screen.getByText('Trash is empty.')).toBeTruthy();
    expect(
      screen.getByText(
        'Recordings moved to Trash appear here for 30 days before permanent deletion.',
      ),
    ).toBeTruthy();
  });

  it('shows the authoritative total in the toolbar, independent of the loaded page', () => {
    fakeStore.trashedList = [makeTrashed('a'), makeTrashed('b')];
    fakeStore.trashedTotal = 57; // more than loaded
    render(TrashPanel);
    expect(screen.getByText('57 recordings in Trash')).toBeTruthy();
  });
});

describe('TrashPanel — search', () => {
  it('search-empty state shows the match-less copy + Clear search', async () => {
    fakeStore.trashedList = [makeTrashed('alpha-visit')];
    fakeStore.trashedTotal = 1;
    render(TrashPanel);

    const input = screen.getByLabelText('Search Trash');
    await fireEvent.input(input, { target: { value: 'zzz-no-match' } });
    // SearchBar debounces 300 ms.
    await new Promise((r) => setTimeout(r, 400));

    expect(screen.getByText('No recordings in Trash match your search.')).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
    expect((screen.getByLabelText('Search Trash') as HTMLInputElement).value).toBe('');
  });

  it('filtered view shows "N matches · M in Trash" with the authoritative M', async () => {
    fakeStore.trashedList = [makeTrashed('alpha'), makeTrashed('beta')];
    fakeStore.trashedTotal = 9;
    render(TrashPanel);

    await fireEvent.input(screen.getByLabelText('Search Trash'), {
      target: { value: 'alpha' },
    });
    await new Promise((r) => setTimeout(r, 400));

    expect(screen.getByText('1 match · 9 in Trash')).toBeTruthy();
    // The non-matching row is hidden.
    expect(screen.queryByText('beta.wav')).toBeNull();
    expect(screen.getByText('alpha.wav')).toBeTruthy();
  });

  it('starts with an empty query on mount (no match line)', () => {
    fakeStore.trashedList = [makeTrashed('a')];
    fakeStore.trashedTotal = 1;
    render(TrashPanel);
    expect(screen.queryByText(/matches? ·/)).toBeNull();
    expect((screen.getByLabelText('Search Trash') as HTMLInputElement).value).toBe('');
  });
});

describe('TrashPanel — per-row restore + a11y', () => {
  it('restore button calls store.restoreTrashed([id]) and announces in the ONE status region', async () => {
    fakeStore.trashedList = [makeTrashed('row-a'), makeTrashed('row-b')];
    fakeStore.trashedTotal = 2;
    mockRestoreTrashed.mockResolvedValue(1);
    render(TrashPanel);

    // Exactly ONE polite status region (D9).
    expect(screen.getAllByRole('status')).toHaveLength(1);

    await fireEvent.click(screen.getAllByRole('button', { name: 'Restore' })[0]);
    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toBe('Recording restored to Active.'),
    );
    expect(mockRestoreTrashed).toHaveBeenCalledWith(['row-a']);
  });

  it('moves focus to the NEXT row\u2019s Restore button after the invoking row disappears', async () => {
    fakeStore.trashedList = [makeTrashed('row-a'), makeTrashed('row-b')];
    fakeStore.trashedTotal = 2;
    mockRestoreTrashed.mockImplementation(async () => {
      // The refresh removes the restored row, like the real store would.
      fakeStore.trashedList = [makeTrashed('row-b')];
      return 1;
    });
    render(TrashPanel);

    await fireEvent.click(
      document.querySelector('[data-restore-btn="row-a"]') as HTMLButtonElement,
    );
    await waitFor(() => {
      const focused = document.activeElement as HTMLElement | null;
      expect(focused?.getAttribute('data-restore-btn')).toBe('row-b');
    });
  });

  it('falls back to the Trash heading when the LAST row disappears', async () => {
    fakeStore.trashedList = [makeTrashed('only-row')];
    fakeStore.trashedTotal = 1;
    mockRestoreTrashed.mockImplementation(async () => {
      fakeStore.trashedList = [];
      return 1;
    });
    render(TrashPanel);

    await fireEvent.click(
      document.querySelector('[data-restore-btn="only-row"]') as HTMLButtonElement,
    );
    await waitFor(() => {
      expect((document.activeElement as HTMLElement)?.getAttribute('data-trash-heading')).toBe(
        '',
      );
    });
    expect(document.activeElement).not.toBe(document.body);
  });

  it('load-more renders when the store says more pages exist', () => {
    fakeStore.trashedList = [makeTrashed('a')];
    fakeStore.trashedTotal = 80;
    fakeStore.trashedHasMore = true;
    render(TrashPanel);
    expect(screen.getByRole('button', { name: 'Load more' })).toBeTruthy();
  });
});

describe('TrashPanel — Restore all (D5)', () => {
  it('Restore all… is disabled when Trash is empty', () => {
    render(TrashPanel);
    expect((screen.getByRole('button', { name: 'Restore all…' }) as HTMLButtonElement).disabled)
      .toBe(true);
  });

  it('confirm dialog carries the authoritative N and the all-scope clarification', async () => {
    fakeStore.trashedList = [makeTrashed('a')];
    fakeStore.trashedTotal = 41; // more than the loaded page
    render(TrashPanel);

    await fireEvent.click(screen.getByRole('button', { name: 'Restore all…' }));
    expect(screen.getByText('Restore all 41 recordings from Trash?')).toBeTruthy();
    expect(
      screen.getByText(
        'They will return to Active with their saved audio and documents. This restores every recording in Trash, not just the ones currently listed.',
      ),
    ).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Restore all 41' })).toBeTruthy();
  });

  it('confirming restores via the store and announces the actual count', async () => {
    fakeStore.trashedList = [makeTrashed('a'), makeTrashed('b')];
    fakeStore.trashedTotal = 2;
    fakeStore.restoreAllFromTrash.mockResolvedValueOnce(2);
    render(TrashPanel);

    await fireEvent.click(screen.getByRole('button', { name: 'Restore all…' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Restore all 2' }));
    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toBe('2 recordings restored to Active.'),
    );
    expect(fakeStore.restoreAllFromTrash).toHaveBeenCalledTimes(1);
    // The dialog closes before the await — a double-click hits nothing.
    expect(screen.queryByText('Restore all 2 recordings from Trash?')).toBeNull();
  });
});

describe('TrashPanel — restore-failure toasts are sanitized (PHI)', () => {
  it('per-row failure shows the generic value, never the raw backend message', async () => {
    fakeStore.trashedList = [makeTrashed('row-a')];
    fakeStore.trashedTotal = 1;
    mockRestoreTrashed.mockRejectedValueOnce(
      new Error('failed to open Smith_John_visit.wav'),
    );
    render(TrashPanel);

    await fireEvent.click(screen.getByRole('button', { name: 'Restore' }));
    await waitFor(() =>
      expect(mockToastsError).toHaveBeenCalledWith(
        "Couldn't restore recording: unexpected error",
      ),
    );
    // The raw backend message never reached the glass.
    expect(mockToastsError.mock.calls.flat().join(' ')).not.toContain('Smith');
  });

  it('restore-all failure keeps only the safe recording id reference', async () => {
    fakeStore.trashedList = [makeTrashed('a')];
    fakeStore.trashedTotal = 1;
    fakeStore.restoreAllFromTrash.mockRejectedValueOnce(
      new Error('Not found: recording 3fa85f64-5717-4562-b3fc-2c963f66afa6'),
    );
    render(TrashPanel);

    await fireEvent.click(screen.getByRole('button', { name: 'Restore all…' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Restore all 1' }));
    await waitFor(() =>
      expect(mockToastsError).toHaveBeenCalledWith(
        "Couldn't restore recordings: recording 3fa85f64-5717-4562-b3fc-2c963f66afa6",
      ),
    );
  });
});
