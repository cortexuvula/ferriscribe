// @vitest-environment jsdom
/**
 * RestoreByDateDialog — exact-calendar-day restore (D6).
 *
 * Facts pinned:
 *   - ONE dialog (role=dialog, aria-modal, labelled + described), a native
 *     labelled date input ("Deleted on"), and the exact helper copy.
 *   - Future dates are rejected (alert + no count call, Restore disabled).
 *   - A valid date runs the DEDICATED count command via the store
 *     (countTrashedOnDate) — never paginated pages — with "Counting…" in
 *     between, then shows the preview copy with the long-form date.
 *   - Zero matches → the zero copy + disabled Restore.
 *   - Restore rides restoreTrashedOnDate with a duplicate-submit guard;
 *     success closes the dialog and announces ACTUAL vs preview (they can
 *     differ when a purge intervened); failure keeps date + dialog with the
 *     retry copy AND invalidates + re-queries the preview (polish Item 4:
 *     a stale N must never survive a failure — the retry executes against
 *     a fresh count).
 */
import { describe, it, expect, vi, beforeEach, type Mock } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import RestoreByDateDialog from './RestoreByDateDialog.svelte';

const { fakeStore, mockCountTrashedOnDate, mockRestoreTrashedOnDate } = vi.hoisted(() => {
  const fakeStore = {
    countTrashedOnDate: vi.fn(),
    restoreTrashedOnDate: vi.fn(),
  };
  return {
    fakeStore,
    mockCountTrashedOnDate: fakeStore.countTrashedOnDate,
    mockRestoreTrashedOnDate: fakeStore.restoreTrashedOnDate,
  };
});

vi.mock('../stores/recordings.svelte', () => ({ recordings: fakeStore }));

function localIso(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('RestoreByDateDialog — structure', () => {
  it('renders one labelled dialog with the exact helper copy', () => {
    render(RestoreByDateDialog, { onClose: () => {}, onAnnounce: () => {} });

    const dialog = screen.getByRole('dialog');
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    expect(dialog.getAttribute('aria-labelledby')).toBe('restore-date-title');
    expect(dialog.getAttribute('aria-describedby')).toBe('restore-date-helper');
    expect(screen.getByLabelText('Deleted on')).toBeTruthy();
    expect(
      screen.getByText(
        "Uses the date recordings were moved to Trash, not the date they were recorded. Searches all of Trash, regardless of the current search. Times use this machine's timezone.",
      ),
    ).toBeTruthy();
    // Restore is disabled before any date is chosen.
    expect((screen.getByRole('button', { name: 'Restore' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });
});

describe('RestoreByDateDialog — date validation', () => {
  it('rejects future dates without counting', async () => {
    render(RestoreByDateDialog, { onClose: () => {}, onAnnounce: () => {} });

    const future = new Date();
    future.setDate(future.getDate() + 30);
    const input = screen.getByLabelText('Deleted on') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: localIso(future) } });
    await fireEvent.change(input, { target: { value: localIso(future) } });

    expect(screen.getByText('Choose a date that has already happened.')).toBeTruthy();
    expect(mockCountTrashedOnDate).not.toHaveBeenCalled();
    expect((screen.getByRole('button', { name: 'Restore' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });
});

describe('RestoreByDateDialog — preview', () => {
  it('counts via the dedicated count command, shows Counting… then the preview copy', async () => {
    let resolveCount: (n: number) => void = () => {};
    mockCountTrashedOnDate.mockImplementationOnce(
      () => new Promise((res) => { resolveCount = res; }),
    );
    const onClose = vi.fn();
    render(RestoreByDateDialog, { onClose, onAnnounce: () => {} });

    const day = new Date();
    day.setDate(day.getDate() - 3);
    const input = screen.getByLabelText('Deleted on') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: localIso(day) } });
    await fireEvent.change(input, { target: { value: localIso(day) } });

    expect(screen.getByText('Counting…')).toBeTruthy();
    resolveCount(5);
    await waitFor(() =>
      expect(screen.getByText(/Restore 5 recordings moved to Trash on /)).toBeTruthy(),
    );
    expect(
      screen.getByText('They will return to Active with their saved audio and documents.'),
    ).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Restore 5 recordings' })).toBeTruthy();
    // The dialog normalizes to the local CALENDAR day (midnight), not the
    // Date object's time-of-day.
    expect(mockCountTrashedOnDate).toHaveBeenCalledWith(
      new Date(day.getFullYear(), day.getMonth(), day.getDate()),
    );
  });

  it('zero matches → exact zero copy + disabled Restore', async () => {
    mockCountTrashedOnDate.mockResolvedValueOnce(0);
    render(RestoreByDateDialog, { onClose: () => {}, onAnnounce: () => {} });

    const day = new Date();
    day.setDate(day.getDate() - 1);
    const input = screen.getByLabelText('Deleted on') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: localIso(day) } });
    await fireEvent.change(input, { target: { value: localIso(day) } });

    await waitFor(() =>
      expect(screen.getByText('No recordings currently in Trash match this date.')).toBeTruthy(),
    );
    expect((screen.getByRole('button', { name: 'Restore' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });
});

describe('RestoreByDateDialog — restore', () => {
  async function previewedDialog(onClose: Mock, onAnnounce: Mock) {
    mockCountTrashedOnDate.mockResolvedValueOnce(4);
    // Wrap in closures: the component's prop types are plain functions,
    // while the mocks are vi.fn instances.
    render(RestoreByDateDialog, {
      onClose: () => onClose(),
      onAnnounce: (m: string) => onAnnounce(m),
    });
    const day = new Date();
    day.setDate(day.getDate() - 2);
    const input = screen.getByLabelText('Deleted on') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: localIso(day) } });
    await fireEvent.change(input, { target: { value: localIso(day) } });
    await screen.findByRole('button', { name: 'Restore 4 recordings' });
    return day;
  }

  it('success announces the actual count and closes the dialog (stays in Trash)', async () => {
    const onClose = vi.fn();
    const onAnnounce = vi.fn();
    const day = await previewedDialog(onClose, onAnnounce);
    mockRestoreTrashedOnDate.mockResolvedValueOnce(4);

    await fireEvent.click(screen.getByRole('button', { name: 'Restore 4 recordings' }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(mockRestoreTrashedOnDate).toHaveBeenCalledWith(
      new Date(day.getFullYear(), day.getMonth(), day.getDate()),
    );
    expect(onAnnounce).toHaveBeenCalledWith('4 recordings restored to Active.');
  });

  it('reports actual vs preview when a purge shrank the candidate set', async () => {
    const onClose = vi.fn();
    const onAnnounce = vi.fn();
    await previewedDialog(onClose, onAnnounce);
    mockRestoreTrashedOnDate.mockResolvedValueOnce(3); // one was purged

    await fireEvent.click(screen.getByRole('button', { name: 'Restore 4 recordings' }));

    await waitFor(() => expect(onAnnounce).toHaveBeenCalled());
    expect(onAnnounce).toHaveBeenCalledWith(
      '3 recordings restored to Active. 1 was no longer in Trash.',
    );
    expect(onClose).toHaveBeenCalled();
  });

  it('failure keeps the date + dialog open with the retry copy and re-queries the preview', async () => {
    const onClose = vi.fn();
    const onAnnounce = vi.fn();
    await previewedDialog(onClose, onAnnounce);
    mockRestoreTrashedOnDate.mockRejectedValueOnce(new Error('db busy'));
    // The recount the failure path must run (polish Item 4).
    mockCountTrashedOnDate.mockResolvedValueOnce(4);

    await fireEvent.click(screen.getByRole('button', { name: 'Restore 4 recordings' }));

    await waitFor(() =>
      expect(screen.getByText("Couldn't restore recordings. Try again.")).toBeTruthy(),
    );
    expect(onClose).not.toHaveBeenCalled();
    expect(
      (screen.getByLabelText('Deleted on') as HTMLInputElement).value,
    ).not.toBe('');
    // The stale preview was invalidated and re-queried: the count command
    // ran again and a FRESH preview stands for the retry.
    expect(mockCountTrashedOnDate).toHaveBeenCalledTimes(2);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Restore 4 recordings' })).toBeTruthy(),
    );
  });

  it('retry after a failure executes against the RE-QUERIED count, not the stale one', async () => {
    const onClose = vi.fn();
    const onAnnounce = vi.fn();
    const day = await previewedDialog(onClose, onAnnounce);
    mockRestoreTrashedOnDate.mockRejectedValueOnce(new Error('db busy'));
    mockCountTrashedOnDate.mockResolvedValueOnce(3); // one purged since

    await fireEvent.click(screen.getByRole('button', { name: 'Restore 4 recordings' }));
    // Recount lands: the button now promises the fresh N.
    const retry = await screen.findByRole('button', { name: 'Restore 3 recordings' });
    mockRestoreTrashedOnDate.mockResolvedValueOnce(3);

    await fireEvent.click(retry);

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(mockRestoreTrashedOnDate).toHaveBeenLastCalledWith(
      new Date(day.getFullYear(), day.getMonth(), day.getDate()),
    );
    // Announced against the fresh preview (3 vs 3 — no "no longer" line).
    expect(onAnnounce).toHaveBeenCalledWith('3 recordings restored to Active.');
  });

  it('Restoring state guards duplicate submissions', async () => {
    const onClose = vi.fn();
    const onAnnounce = vi.fn();
    await previewedDialog(onClose, onAnnounce);
    let resolveRestore: (n: number) => void = () => {};
    mockRestoreTrashedOnDate.mockImplementationOnce(
      () => new Promise((res) => { resolveRestore = res; }),
    );

    await fireEvent.click(screen.getByRole('button', { name: 'Restore 4 recordings' }));
    expect(screen.getByText('Restoring…')).toBeTruthy();
    const cancel = screen.getByRole('button', { name: 'Cancel' }) as HTMLButtonElement;
    expect(cancel.disabled).toBe(true);

    resolveRestore(4);
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(mockRestoreTrashedOnDate).toHaveBeenCalledTimes(1);
  });
});

describe('RestoreByDateDialog — stale count guard', () => {
  it('discards a late count for a superseded date (monotonic request token)', async () => {
    let resolveOlder: (n: number) => void = () => {};
    mockCountTrashedOnDate
      .mockImplementationOnce(() => new Promise((res) => { resolveOlder = res; }))
      .mockResolvedValueOnce(2);
    render(RestoreByDateDialog, { onClose: () => {}, onAnnounce: () => {} });

    const older = new Date();
    older.setDate(older.getDate() - 5);
    const newer = new Date();
    newer.setDate(newer.getDate() - 1);
    const input = screen.getByLabelText('Deleted on') as HTMLInputElement;

    await fireEvent.input(input, { target: { value: localIso(older) } });
    await fireEvent.change(input, { target: { value: localIso(older) } });
    expect(screen.getByText('Counting…')).toBeTruthy();

    // The user picks a different day before the first count resolves.
    await fireEvent.input(input, { target: { value: localIso(newer) } });
    await fireEvent.change(input, { target: { value: localIso(newer) } });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Restore 2 recordings' })).toBeTruthy(),
    );

    // The abandoned date's count lands LAST — date A's N must never show
    // under date B's heading.
    resolveOlder(9);
    await Promise.resolve();
    await Promise.resolve();
    expect(screen.queryByText(/Restore 9/)).toBeNull();
    expect(screen.getByRole('button', { name: 'Restore 2 recordings' })).toBeTruthy();
  });

  it('invalidates the in-flight count when the date changes to a future day', async () => {
    let resolveCount: (n: number) => void = () => {};
    mockCountTrashedOnDate.mockImplementationOnce(
      () => new Promise((res) => { resolveCount = res; }),
    );
    render(RestoreByDateDialog, { onClose: () => {}, onAnnounce: () => {} });

    const past = new Date();
    past.setDate(past.getDate() - 1);
    const future = new Date();
    future.setDate(future.getDate() + 1);
    const input = screen.getByLabelText('Deleted on') as HTMLInputElement;

    await fireEvent.input(input, { target: { value: localIso(past) } });
    await fireEvent.change(input, { target: { value: localIso(past) } });
    expect(screen.getByText('Counting…')).toBeTruthy();

    await fireEvent.input(input, { target: { value: localIso(future) } });
    await fireEvent.change(input, { target: { value: localIso(future) } });
    expect(screen.getByText('Choose a date that has already happened.')).toBeTruthy();
    expect(screen.queryByText('Counting…')).toBeNull();

    // The abandoned count resolves late — it must not surface a preview
    // for a date the dialog just rejected.
    resolveCount(7);
    await Promise.resolve();
    await Promise.resolve();
    expect(screen.queryByText(/Restore 7/)).toBeNull();
    expect(
      (screen.getByRole('button', { name: 'Restore' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});
