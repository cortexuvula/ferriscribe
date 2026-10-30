// @vitest-environment jsdom
/**
 * MoveByDateRangeDialog — move active recordings by recorded-date range.
 *
 * Facts pinned:
 *   - ONE dialog (role=dialog, aria-modal, labelled + described), two
 *     native labelled date inputs ("Recorded from" / "Recorded to"), and
 *     the exact helper copy.
 *   - Future dates are rejected (alert + no count call, Move disabled);
 *     a reversed range is rejected with its own alert; same-day valid.
 *   - A valid range runs the DEDICATED count command via the store
 *     (countActiveInRange) — never the paginated list — with "Counting…"
 *     in between, then shows the preview copy with both long-form dates
 *     and the retention sub-copy.
 *   - Zero matches → the zero copy + disabled Move.
 *   - Rapid date changes: a late count for a superseded range is
 *     discarded (monotonic request token).
 *   - Move rides moveActiveRangeToTrash with the local-midnight window
 *     Dates; success reports {count, ids} + the preview to the parent and
 *     closes; failure keeps dates + dialog with the retry copy, notifies
 *     the parent, AND invalidates + re-queries the preview (a stale N
 *     must never back a retry).
 *   - Mid-action (Moving…) both buttons + inputs are disabled.
 */
import { describe, it, expect, vi, beforeEach, type Mock } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import MoveByDateRangeDialog from './MoveByDateRangeDialog.svelte';

const { fakeStore, mockCountActiveInRange, mockMoveActiveRangeToTrash } = vi.hoisted(() => {
  const fakeStore = {
    countActiveInRange: vi.fn(),
    moveActiveRangeToTrash: vi.fn(),
  };
  return {
    fakeStore,
    mockCountActiveInRange: fakeStore.countActiveInRange,
    mockMoveActiveRangeToTrash: fakeStore.moveActiveRangeToTrash,
  };
});

vi.mock('../stores/recordings.svelte', () => ({ recordings: fakeStore }));
vi.mock('../stores/settings.svelte', () => ({ settings: { trashRetentionDays: 30 } }));

function localIso(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function midnight(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

async function setRange(
  start: Date,
  end: Date,
) {
  const startInput = screen.getByLabelText('Recorded from') as HTMLInputElement;
  const endInput = screen.getByLabelText('Recorded to') as HTMLInputElement;
  await fireEvent.input(startInput, { target: { value: localIso(start) } });
  await fireEvent.change(startInput, { target: { value: localIso(start) } });
  await fireEvent.input(endInput, { target: { value: localIso(end) } });
  await fireEvent.change(endInput, { target: { value: localIso(end) } });
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('MoveByDateRangeDialog — structure', () => {
  it('renders one labelled dialog with the exact helper copy', () => {
    render(MoveByDateRangeDialog, {
      onClose: () => {},
      onMoved: () => {},
    });

    const dialog = screen.getByRole('dialog');
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    expect(dialog.getAttribute('aria-labelledby')).toBe('move-range-title');
    expect(dialog.getAttribute('aria-describedby')).toBe('move-range-helper');
    expect(screen.getByText('Move recordings by date to Trash')).toBeTruthy();
    expect(screen.getByLabelText('Recorded from')).toBeTruthy();
    expect(screen.getByLabelText('Recorded to')).toBeTruthy();
    expect(
      screen.getByText(
        "Uses the date each recording was made, not the date it was moved to Trash. Includes all active recordings in the range, regardless of the current search and selected date. Times use this machine's timezone.",
      ),
    ).toBeTruthy();
    // Move is disabled before a valid range is chosen.
    expect((screen.getByRole('button', { name: 'Move to Trash' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });
});

describe('MoveByDateRangeDialog — date validation', () => {
  it('rejects reversed ranges without counting', async () => {
    render(MoveByDateRangeDialog, { onClose: () => {}, onMoved: () => {} });

    const later = new Date();
    later.setDate(later.getDate() - 1);
    const earlier = new Date();
    earlier.setDate(earlier.getDate() - 5);
    await setRange(later, earlier); // end before start

    expect(screen.getByText("The end date can't be before the start date.")).toBeTruthy();
    expect(mockCountActiveInRange).not.toHaveBeenCalled();
    expect(
      (screen.getByRole('button', { name: 'Move to Trash' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it('rejects future dates without counting', async () => {
    render(MoveByDateRangeDialog, { onClose: () => {}, onMoved: () => {} });

    const past = new Date();
    past.setDate(past.getDate() - 3);
    const future = new Date();
    future.setDate(future.getDate() + 1);
    await setRange(past, future);

    expect(screen.getByText('Choose a date that has already happened.')).toBeTruthy();
    expect(mockCountActiveInRange).not.toHaveBeenCalled();
    expect(
      (screen.getByRole('button', { name: 'Move to Trash' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it('a same-day range is valid and counts', async () => {
    mockCountActiveInRange.mockResolvedValueOnce(1);
    render(MoveByDateRangeDialog, { onClose: () => {}, onMoved: () => {} });

    const day = new Date();
    day.setDate(day.getDate() - 2);
    await setRange(day, day);

    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Move 1 recording to Trash' })).toBeTruthy(),
    );
    expect(screen.queryByText("The end date can't be before the start date.")).toBeNull();
  });
});

describe('MoveByDateRangeDialog — preview', () => {
  it('counts via the dedicated count command, shows Counting… then the preview copy', async () => {
    let resolveCount: (n: number) => void = () => {};
    mockCountActiveInRange.mockImplementationOnce(
      () => new Promise((res) => { resolveCount = res; }),
    );
    render(MoveByDateRangeDialog, { onClose: () => {}, onMoved: () => {} });

    const start = new Date();
    start.setDate(start.getDate() - 5);
    const end = new Date();
    end.setDate(end.getDate() - 3);
    await setRange(start, end);

    expect(screen.getByText('Counting…')).toBeTruthy();
    resolveCount(5);
    await waitFor(() =>
      expect(
        screen.getByText(/Move 5 recordings recorded .+ through .+ to Trash\?/),
      ).toBeTruthy(),
    );
    expect(
      screen.getByText(
        'They’ll stay in Trash and can be restored for 30 days. After that, cleanup permanently deletes their audio, transcripts, SOAP notes and generated documents.',
      ),
    ).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Move 5 recordings to Trash' })).toBeTruthy();
    // The dialog normalizes both ends to local CALENDAR midnights.
    expect(mockCountActiveInRange).toHaveBeenCalledWith(midnight(start), midnight(end));
  });

  it('zero matches → exact zero copy + disabled Move', async () => {
    mockCountActiveInRange.mockResolvedValueOnce(0);
    render(MoveByDateRangeDialog, { onClose: () => {}, onMoved: () => {} });

    const start = new Date();
    start.setDate(start.getDate() - 2);
    const end = new Date();
    end.setDate(end.getDate() - 1);
    await setRange(start, end);

    await waitFor(() =>
      expect(screen.getByText('No active recordings match this date range.')).toBeTruthy(),
    );
    expect(
      (screen.getByRole('button', { name: 'Move to Trash' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe('MoveByDateRangeDialog — stale count guard', () => {
  it('discards a late count for a superseded range (monotonic request token)', async () => {
    let resolveOlder: (n: number) => void = () => {};
    mockCountActiveInRange
      .mockImplementationOnce(() => new Promise((res) => { resolveOlder = res; }))
      .mockResolvedValueOnce(2);
    render(MoveByDateRangeDialog, { onClose: () => {}, onMoved: () => {} });

    const startInput = screen.getByLabelText('Recorded from') as HTMLInputElement;
    const endInput = screen.getByLabelText('Recorded to') as HTMLInputElement;

    const olderStart = new Date();
    olderStart.setDate(olderStart.getDate() - 9);
    const olderEnd = new Date();
    olderEnd.setDate(olderEnd.getDate() - 8);
    await fireEvent.input(startInput, { target: { value: localIso(olderStart) } });
    await fireEvent.change(startInput, { target: { value: localIso(olderStart) } });
    await fireEvent.input(endInput, { target: { value: localIso(olderEnd) } });
    await fireEvent.change(endInput, { target: { value: localIso(olderEnd) } });
    expect(screen.getByText('Counting…')).toBeTruthy();

    // The user picks a different range before the first count resolves.
    const newerEnd = new Date();
    newerEnd.setDate(newerEnd.getDate() - 1);
    await fireEvent.input(endInput, { target: { value: localIso(newerEnd) } });
    await fireEvent.change(endInput, { target: { value: localIso(newerEnd) } });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Move 2 recordings to Trash' })).toBeTruthy(),
    );

    // The abandoned range's count lands LAST — range A's N must never show
    // under range B's heading.
    resolveOlder(9);
    await Promise.resolve();
    await Promise.resolve();
    expect(screen.queryByText(/Move 9/)).toBeNull();
    expect(screen.getByRole('button', { name: 'Move 2 recordings to Trash' })).toBeTruthy();
  });

  it('invalidates the in-flight count when a bound changes to a future day', async () => {
    let resolveCount: (n: number) => void = () => {};
    mockCountActiveInRange.mockImplementationOnce(
      () => new Promise((res) => { resolveCount = res; }),
    );
    render(MoveByDateRangeDialog, { onClose: () => {}, onMoved: () => {} });

    const start = new Date();
    start.setDate(start.getDate() - 2);
    const end = new Date();
    end.setDate(end.getDate() - 1);
    await setRange(start, end);
    expect(screen.getByText('Counting…')).toBeTruthy();

    const future = new Date();
    future.setDate(future.getDate() + 1);
    const endInput = screen.getByLabelText('Recorded to') as HTMLInputElement;
    await fireEvent.input(endInput, { target: { value: localIso(future) } });
    await fireEvent.change(endInput, { target: { value: localIso(future) } });
    expect(screen.getByText('Choose a date that has already happened.')).toBeTruthy();
    expect(screen.queryByText('Counting…')).toBeNull();

    // The abandoned count resolves late — it must not surface a preview
    // for a range the dialog just rejected.
    resolveCount(7);
    await Promise.resolve();
    await Promise.resolve();
    expect(screen.queryByText(/Move 7/)).toBeNull();
    expect(
      (screen.getByRole('button', { name: 'Move to Trash' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe('MoveByDateRangeDialog — move', () => {
  async function previewedDialog(onClose: Mock, onMoved: Mock, onMoveFailed?: Mock) {
    mockCountActiveInRange.mockResolvedValueOnce(4);
    render(MoveByDateRangeDialog, {
      onClose: () => onClose(),
      onMoved: (r: { count: number; ids: string[] }, p: number) => onMoved(r, p),
      onMoveFailed: onMoveFailed ? (e: unknown) => onMoveFailed(e) : undefined,
    });
    const start = new Date();
    start.setDate(start.getDate() - 4);
    const end = new Date();
    end.setDate(end.getDate() - 2);
    await setRange(start, end);
    await screen.findByRole('button', { name: 'Move 4 recordings to Trash' });
    return { start, end };
  }

  it('success reports the result + preview to the parent and closes', async () => {
    const onClose = vi.fn();
    const onMoved = vi.fn();
    const { start, end } = await previewedDialog(onClose, onMoved);
    const result = { count: 4, ids: ['a', 'b', 'c', 'd'] };
    mockMoveActiveRangeToTrash.mockResolvedValueOnce(result);

    await fireEvent.click(screen.getByRole('button', { name: 'Move 4 recordings to Trash' }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(mockMoveActiveRangeToTrash).toHaveBeenCalledWith(midnight(start), midnight(end));
    expect(onMoved).toHaveBeenCalledWith(result, 4);
  });

  it('failure keeps dates + dialog open, notifies the parent, and re-queries the preview', async () => {
    const onClose = vi.fn();
    const onMoved = vi.fn();
    const onMoveFailed = vi.fn();
    await previewedDialog(onClose, onMoved, onMoveFailed);
    mockMoveActiveRangeToTrash.mockRejectedValueOnce(new Error('db busy'));
    // The recount the failure path must run (stale N must never back a
    // retry).
    mockCountActiveInRange.mockResolvedValueOnce(4);

    await fireEvent.click(screen.getByRole('button', { name: 'Move 4 recordings to Trash' }));

    await waitFor(() =>
      expect(screen.getByText("Couldn't move recordings. Try again.")).toBeTruthy(),
    );
    expect(onClose).not.toHaveBeenCalled();
    expect(onMoveFailed).toHaveBeenCalledTimes(1);
    expect(
      (screen.getByLabelText('Recorded from') as HTMLInputElement).value,
    ).not.toBe('');
    // The stale preview was invalidated and re-queried: the count command
    // ran again and a FRESH preview stands for the retry.
    expect(mockCountActiveInRange).toHaveBeenCalledTimes(2);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Move 4 recordings to Trash' })).toBeTruthy(),
    );
  });

  it('retry after a failure executes against the RE-QUERIED count, not the stale one', async () => {
    const onClose = vi.fn();
    const onMoved = vi.fn();
    const { start, end } = await previewedDialog(onClose, onMoved);
    mockMoveActiveRangeToTrash.mockRejectedValueOnce(new Error('db busy'));
    mockCountActiveInRange.mockResolvedValueOnce(3); // one deleted since

    await fireEvent.click(screen.getByRole('button', { name: 'Move 4 recordings to Trash' }));
    // Recount lands: the button now promises the fresh N.
    const retry = await screen.findByRole('button', { name: 'Move 3 recordings to Trash' });
    mockMoveActiveRangeToTrash.mockResolvedValueOnce({ count: 3, ids: ['a', 'b', 'c'] });

    await fireEvent.click(retry);

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(mockMoveActiveRangeToTrash).toHaveBeenLastCalledWith(midnight(start), midnight(end));
    // Reported against the fresh preview (3 of 3 — no mismatch line).
    expect(onMoved).toHaveBeenCalledWith({ count: 3, ids: ['a', 'b', 'c'] }, 3);
  });

  it('Moving state disables both buttons and both inputs, guards duplicate submissions', async () => {
    const onClose = vi.fn();
    const onMoved = vi.fn();
    await previewedDialog(onClose, onMoved);
    let resolveMove: (r: { count: number; ids: string[] }) => void = () => {};
    mockMoveActiveRangeToTrash.mockImplementationOnce(
      () => new Promise((res) => { resolveMove = res; }),
    );

    await fireEvent.click(screen.getByRole('button', { name: 'Move 4 recordings to Trash' }));
    // 'Moving…' appears both as the status line and the confirm label.
    expect(screen.getAllByText('Moving…').length).toBeGreaterThan(0);
    expect(
      (screen.getByRole('button', { name: 'Moving…' }) as HTMLButtonElement).disabled,
    ).toBe(true);
    const cancel = screen.getByRole('button', { name: 'Cancel' }) as HTMLButtonElement;
    expect(cancel.disabled).toBe(true);
    expect((screen.getByLabelText('Recorded from') as HTMLInputElement).disabled).toBe(true);
    expect((screen.getByLabelText('Recorded to') as HTMLInputElement).disabled).toBe(true);

    resolveMove({ count: 4, ids: ['a', 'b', 'c', 'd'] });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(mockMoveActiveRangeToTrash).toHaveBeenCalledTimes(1);
  });
});
