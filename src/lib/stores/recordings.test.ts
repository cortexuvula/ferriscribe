// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { RecordingSummary } from '../types';

// Mock the API layer so we control listRecordings output + pagination offsets.
const mockListRecordings = vi.fn();
const mockSearchRecordings = vi.fn();
const mockCountRecordings = vi.fn();
const mockDeleteAllRecordings = vi.fn();
const mockRestoreRecordings = vi.fn();
const mockDeleteRecording = vi.fn();
const mockListTrashed = vi.fn();
const mockRestoreAllTrashed = vi.fn();
const mockRestoreBetween = vi.fn();
const mockCountBetween = vi.fn();
vi.mock('../api/recordings', () => ({
  listRecordings: (...args: unknown[]) => mockListRecordings(...args),
  searchRecordings: (...args: unknown[]) => mockSearchRecordings(...args),
  countRecordings: (...args: unknown[]) => mockCountRecordings(...args),
  deleteAllRecordings: (...args: unknown[]) => mockDeleteAllRecordings(...args),
  restoreRecordings: (...args: unknown[]) => mockRestoreRecordings(...args),
  deleteRecording: (...args: unknown[]) => mockDeleteRecording(...args),
  listTrashedRecordings: (...args: unknown[]) => mockListTrashed(...args),
  restoreAllTrashed: (...args: unknown[]) => mockRestoreAllTrashed(...args),
  restoreRecordingsDeletedBetween: (...args: unknown[]) => mockRestoreBetween(...args),
  countRecordingsDeletedBetween: (...args: unknown[]) => mockCountBetween(...args),
  getRecording: vi.fn(),
  restoreRecording: vi.fn(),
}));

function makeSummary(id: string): RecordingSummary {
  return {
    id,
    filename: `${id}.wav`,
    patient_name: null,
    status: { status: 'completed', completed_at: '2026-06-25T00:00:00Z' },
    duration_seconds: 60,
    created_at: '2026-06-25T00:00:00Z',
    tags: [],
    has_transcript: false,
    has_soap_note: false,
    has_referral: false,
    has_letter: false,
    has_peer_discussion: false,
    is_remote: false,
    tokens_per_second: null,
  };
}

// Re-import the module FRESHLY so each test gets a clean store instance.
async function freshStore() {
  vi.resetModules();
  return await import('./recordings.svelte');
}

describe('RecordingsStore — pagination + dedup', () => {
  beforeEach(() => {
    mockListRecordings.mockReset();
    mockCountRecordings.mockReset();
    mockCountRecordings.mockResolvedValue(0);
    vi.clearAllMocks();
  });

  it('load() sets hasMore=true when a full page returns', async () => {
    const page = Array.from({ length: 50 }, (_, i) => makeSummary(`r${i}`));
    mockListRecordings.mockResolvedValue(page);
    const { recordings } = await freshStore();

    await recordings.load(50, 0);
    expect(recordings.list).toHaveLength(50);
    expect(recordings.hasMore).toBe(true);
  });

  it('load() sets hasMore=false when a partial page returns', async () => {
    mockListRecordings.mockResolvedValue([makeSummary('a'), makeSummary('b')]);
    const { recordings } = await freshStore();

    await recordings.load(50, 0);
    expect(recordings.list).toHaveLength(2);
    expect(recordings.hasMore).toBe(false);
  });

  it('loadMore() appends the next page and dedupes by id', async () => {
    // First page: 50 items.
    const page1 = Array.from({ length: 50 }, (_, i) => makeSummary(`r${i}`));
    // Second page: 50 items, but r49 is a duplicate (new recording landed
    // between pages and shifted offsets).
    const page2 = Array.from({ length: 50 }, (_, i) => makeSummary(`r${50 + i}`));
    page2[0] = makeSummary('r49'); // duplicate of last item in page1

    mockListRecordings
      .mockResolvedValueOnce(page1)
      .mockResolvedValueOnce(page2);

    const { recordings } = await freshStore();
    await recordings.load(50, 0);
    await recordings.loadMore();

    // 50 + 49 (one deduped) = 99, not 100.
    expect(recordings.list).toHaveLength(99);
    // No duplicate ids.
    const ids = recordings.list.map((r) => r.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it('loadMore() is a no-op when hasMore is false', async () => {
    mockListRecordings.mockResolvedValue([makeSummary('a')]);
    const { recordings } = await freshStore();

    await recordings.load(50, 0);
    expect(recordings.hasMore).toBe(false);

    const callsBefore = mockListRecordings.mock.calls.length;
    await recordings.loadMore();
    expect(mockListRecordings.mock.calls.length).toBe(callsBefore);
  });

  it('loadMore() is a no-op when already loading more', async () => {
    const page = Array.from({ length: 50 }, (_, i) => makeSummary(`r${i}`));
    mockListRecordings.mockResolvedValue(page);
    const { recordings } = await freshStore();

    await recordings.load(50, 0);
    // Simulate in-flight loadMore.
    recordings.loadingMore = true;
    const callsBefore = mockListRecordings.mock.calls.length;
    await recordings.loadMore();
    expect(mockListRecordings.mock.calls.length).toBe(callsBefore);
  });

  it('loadMore() sets hasMore=false when the next page is partial', async () => {
    const page1 = Array.from({ length: 50 }, (_, i) => makeSummary(`r${i}`));
    const page2 = [makeSummary('r50'), makeSummary('r51')]; // only 2 = partial
    mockListRecordings
      .mockResolvedValueOnce(page1)
      .mockResolvedValueOnce(page2);

    const { recordings } = await freshStore();
    await recordings.load(50, 0);
    await recordings.loadMore();

    expect(recordings.hasMore).toBe(false);
    expect(recordings.list).toHaveLength(52);
  });

  it('a stale search response cannot clobber the latest results', async () => {
    // The SearchBar debounce fires overlapping queries; without the
    // request token the first query's response landing LAST would win.
    const { recordings } = await freshStore();

    // "smi" resolves slowly; "smith" (issued later) resolves immediately.
    let resolveSlow: (v: ReturnType<typeof makeSummary>[]) => void = () => {};
    mockSearchRecordings.mockImplementationOnce(
      () => new Promise((res) => { resolveSlow = res; }),
    );
    mockSearchRecordings.mockResolvedValueOnce([makeSummary('smith-1')]);

    const p1 = recordings.search('smi');
    const p2 = recordings.search('smith');
    await p2;
    expect(recordings.list.map((r) => r.id)).toEqual(['smith-1']);

    // The stale "smi" response arrives now — it must be discarded.
    resolveSlow([makeSummary('stale-smi')]);
    await p1;
    expect(recordings.list.map((r) => r.id)).toEqual(['smith-1']);
    expect(recordings.loading).toBe(false);
  });
});

describe('RecordingsStore — Move all to Trash (D1/D2 contract)', () => {
  beforeEach(() => {
    mockListRecordings.mockReset();
    mockSearchRecordings.mockReset();
    mockCountRecordings.mockReset();
    mockDeleteAllRecordings.mockReset();
    mockRestoreRecordings.mockReset();
    mockDeleteRecording.mockReset();
    mockListTrashed.mockReset();
    mockRestoreAllTrashed.mockReset();
    mockRestoreBetween.mockReset();
    mockCountBetween.mockReset();
    mockCountRecordings.mockResolvedValue(0);
    mockListTrashed.mockResolvedValue({ items: [], total: 0 });
    vi.clearAllMocks();
  });

  it('load() refreshes the authoritative active count alongside the list', async () => {
    mockListRecordings.mockResolvedValue([makeSummary('a'), makeSummary('b')]);
    mockCountRecordings.mockResolvedValue(42);
    const { recordings } = await freshStore();

    await recordings.load();
    // The count is the backend truth, not the loaded page length.
    expect(recordings.activeTotal).toBe(42);
  });

  it('refreshActiveTotal() leaves the previous count in place on failure', async () => {
    mockCountRecordings.mockResolvedValue(7);
    const { recordings } = await freshStore();
    await recordings.refreshActiveTotal();
    expect(recordings.activeTotal).toBe(7);

    mockCountRecordings.mockRejectedValue(new Error('db locked'));
    await recordings.refreshActiveTotal();
    expect(recordings.activeTotal).toBe(7);
  });

  it('removeAll() captures the EXACT id set for the batch Undo toast', async () => {
    mockListRecordings.mockResolvedValue([makeSummary('a')]);
    mockDeleteAllRecordings.mockResolvedValue({
      count: 2,
      ids: ['id-a', 'id-b'],
    });
    const { recordings } = await freshStore();
    await recordings.load();

    const result = await recordings.removeAll();

    expect(result.count).toBe(2);
    expect(recordings.lastDeletedAllIds).toEqual(['id-a', 'id-b']);
    expect(recordings.list).toHaveLength(0);
    expect(recordings.activeTotal).toBe(0);
    expect(recordings.selectedRecording).toBeNull();
  });

  it('undoMoveAll() restores exactly the captured set — never a later deletion', async () => {
    mockListRecordings.mockResolvedValue([]);
    mockCountRecordings.mockResolvedValue(0);
    mockDeleteAllRecordings.mockResolvedValue({
      count: 2,
      ids: ['id-a', 'id-b'],
    });
    mockRestoreRecordings.mockResolvedValue({ count: 2, ids: ['id-a', 'id-b'] });
    const { recordings } = await freshStore();

    await recordings.removeAll();
    // A recording deleted AFTER the move (single delete of id-c) must not
    // be swept into the undo — the undo restores the snapshot, not "everything
    // deleted since the move".
    mockDeleteRecording.mockResolvedValue(undefined);
    await recordings.remove('id-c');

    const restored = await recordings.undoMoveAll();

    expect(restored).toBe(2);
    expect(mockRestoreRecordings).toHaveBeenCalledWith(['id-a', 'id-b']);
    expect(mockRestoreRecordings).toHaveBeenCalledTimes(1);
    expect(recordings.lastDeletedAllIds).toBeNull();
    expect(recordings.restoring).toBe(false);
  });

  it('undoMoveAll() rejects a duplicate submission while one is in flight', async () => {
    mockListRecordings.mockResolvedValue([]);
    mockCountRecordings.mockResolvedValue(0);
    mockDeleteAllRecordings.mockResolvedValue({ count: 1, ids: ['id-a'] });
    let resolveRestore: (v: { count: number; ids: string[] }) => void = () => {};
    mockRestoreRecordings.mockImplementationOnce(
      () => new Promise((res) => { resolveRestore = res; }),
    );
    const { recordings } = await freshStore();
    await recordings.removeAll();

    const first = recordings.undoMoveAll();
    // Second click while the first restore is still in flight.
    await expect(recordings.undoMoveAll()).rejects.toThrow(/already in progress/);
    resolveRestore({ count: 1, ids: ['id-a'] });
    await expect(first).resolves.toBe(1);
    expect(mockRestoreRecordings).toHaveBeenCalledTimes(1);
  });

  it('undoMoveAll() reports the ACTUAL restored count when some ids were purged', async () => {
    mockListRecordings.mockResolvedValue([]);
    mockCountRecordings.mockResolvedValue(0);
    mockDeleteAllRecordings.mockResolvedValue({ count: 3, ids: ['a', 'b', 'c'] });
    mockRestoreRecordings.mockResolvedValue({ count: 1, ids: ['a'] });
    const { recordings } = await freshStore();

    await recordings.removeAll();
    await expect(recordings.undoMoveAll()).resolves.toBe(1);
  });

  it('undoMoveAll() with nothing captured is an error', async () => {
    const { recordings } = await freshStore();
    await expect(recordings.undoMoveAll()).rejects.toThrow(/no move-all to undo/i);
  });
});

function makeTrashed(id: string, deletedAt = '2026-10-01T12:00:00Z') {
  return {
    id,
    filename: `${id}.wav`,
    patient_name: null,
    duration_seconds: 60,
    created_at: '2026-09-01T10:00:00Z',
    deleted_at: deletedAt,
  };
}

describe('RecordingsStore — Trash state + restore paths (D4/D8)', () => {
  beforeEach(() => {
    mockListRecordings.mockReset();
    mockSearchRecordings.mockReset();
    mockCountRecordings.mockReset();
    mockDeleteAllRecordings.mockReset();
    mockRestoreRecordings.mockReset();
    mockDeleteRecording.mockReset();
    mockListTrashed.mockReset();
    mockRestoreAllTrashed.mockReset();
    mockRestoreBetween.mockReset();
    mockCountBetween.mockReset();
    mockCountRecordings.mockResolvedValue(0);
    mockListRecordings.mockResolvedValue([]);
    mockListTrashed.mockResolvedValue({ items: [], total: 0 });
    vi.clearAllMocks();
  });

  it('loadTrashed() fills list + authoritative total; a full page under the total means hasMore', async () => {
    const page = Array.from({ length: 2 }, (_, i) => makeTrashed(`t${i}`));
    mockListTrashed.mockResolvedValue({ items: page, total: 5 });
    const { recordings } = await freshStore();

    await recordings.loadTrashed(2, 0);

    expect(recordings.trashedList).toHaveLength(2);
    expect(recordings.trashedTotal).toBe(5);
    expect(recordings.trashedHasMore).toBe(true);
    expect(mockListTrashed).toHaveBeenCalledWith(2, 0);
  });

  it('loadTrashed() failure sets trashedError — a load failure is never an empty trash', async () => {
    mockListTrashed.mockRejectedValue(new Error('db busy'));
    const { recordings } = await freshStore();

    await recordings.loadTrashed();

    expect(recordings.trashedError).toBe('db busy');
    expect(recordings.trashedList).toHaveLength(0);
    expect(recordings.trashedLoading).toBe(false);
  });

  it('a stale trash load cannot clobber a fresh one (monotonic request id)', async () => {
    let resolveSlow: (v: { items: unknown[]; total: number }) => void = () => {};
    mockListTrashed.mockImplementationOnce(
      () => new Promise((res) => { resolveSlow = res; }),
    );
    mockListTrashed.mockResolvedValueOnce({ items: [makeTrashed('fresh')], total: 1 });
    const { recordings } = await freshStore();

    const slow = recordings.loadTrashed();
    await recordings.loadTrashed();
    expect(recordings.trashedList.map((r) => r.id)).toEqual(['fresh']);

    resolveSlow({ items: [makeTrashed('stale')], total: 99 });
    await slow;
    expect(recordings.trashedList.map((r) => r.id)).toEqual(['fresh'], 'stale response discarded');
    expect(recordings.trashedTotal).toBe(1);
  });

  it('loadMoreTrashed() appends the next page and refreshes the total', async () => {
    mockListTrashed
      .mockResolvedValueOnce({ items: [makeTrashed('t0'), makeTrashed('t1')], total: 3 })
      .mockResolvedValueOnce({ items: [makeTrashed('t2')], total: 3 });
    const { recordings } = await freshStore();

    await recordings.loadTrashed(2, 0);
    await recordings.loadMoreTrashed();

    expect(recordings.trashedList.map((r) => r.id)).toEqual(['t0', 't1', 't2']);
    expect(recordings.trashedHasMore).toBe(false);
  });

  it('refreshTrashedTotal() fetches only the count (limit 0)', async () => {
    mockListTrashed.mockResolvedValue({ items: [], total: 7 });
    const { recordings } = await freshStore();

    await recordings.refreshTrashedTotal();

    expect(mockListTrashed).toHaveBeenCalledWith(0, 0);
    expect(recordings.trashedTotal).toBe(7);
  });

  it('restoreTrashed() restores, refreshes BOTH lists, and guards duplicate submissions', async () => {
    mockRestoreRecordings.mockResolvedValue({ count: 1, ids: ['t0'] });
    const { recordings } = await freshStore();
    let blockRefresh: () => void = () => {};
    mockListTrashed.mockImplementationOnce(
      () => new Promise((res) => { blockRefresh = () => res({ items: [], total: 0 }); }),
    );

    const first = recordings.restoreTrashed(['t0']);
    await expect(recordings.restoreTrashed(['t1'])).rejects.toThrow(/already in progress/);
    blockRefresh();
    await expect(first).resolves.toBe(1);

    expect(mockRestoreRecordings).toHaveBeenCalledWith(['t0']);
    expect(mockRestoreRecordings).toHaveBeenCalledTimes(1);
  });

  it('restoreAllFromTrash() rides the dedicated command and refreshes both lists', async () => {
    mockRestoreAllTrashed.mockResolvedValue({ count: 3, ids: ['a', 'b', 'c'] });
    const { recordings } = await freshStore();

    await expect(recordings.restoreAllFromTrash()).resolves.toBe(3);

    expect(mockRestoreAllTrashed).toHaveBeenCalledTimes(1);
    expect(mockListRecordings).toHaveBeenCalled();
    expect(mockListTrashed).toHaveBeenCalled();
  });

  it('restore-by-date sends the LOCAL day as a half-open UTC interval', async () => {
    // Local timezone in jsdom is machine-dependent — assert shape via the
    // exported helper, and that the store forwards exactly its output.
    const { recordings, localDayIntervalUtc } = await freshStore();
    mockCountBetween.mockResolvedValue(2);
    mockRestoreBetween.mockResolvedValue({ count: 2, ids: ['x', 'y'] });

    const day = new Date(2026, 9, 6); // Oct 6 2026, local midnight
    const { startIso, endIso } = localDayIntervalUtc(day);
    expect(new Date(startIso).getTime()).toBeLessThan(new Date(endIso).getTime());

    await expect(recordings.countTrashedOnDate(day)).resolves.toBe(2);
    expect(mockCountBetween).toHaveBeenCalledWith(startIso, endIso);

    await expect(recordings.restoreTrashedOnDate(day)).resolves.toBe(2);
    expect(mockRestoreBetween).toHaveBeenCalledWith(startIso, endIso);
  });
});

describe('localDayIntervalUtc — exact local calendar day', () => {
  it('covers exactly one local day, starting at local midnight', async () => {
    const { localDayIntervalUtc } = await freshStore();
    // Machine-independent property: start is the local midnight of the
    // given day, end is the NEXT local midnight (23h/25h under DST because
    // both bounds are calendar midnights, not 24h offsets).
    const { startIso: s, endIso: e } = localDayIntervalUtc(new Date(2026, 9, 6));
    const start = new Date(s);
    const end = new Date(e);
    expect(start.getHours()).toBe(0);
    expect(start.getMinutes()).toBe(0);
    expect(end.getHours()).toBe(0);
    const dayMs = end.getTime() - start.getTime();
    // A calendar day is 23, 24, or 25 hours depending on DST — never
    // anything else.
    const hours = dayMs / 3_600_000;
    expect([23, 24, 25]).toContain(hours);
  });

  it('rolls month and year boundaries correctly', async () => {
    const { localDayIntervalUtc } = await freshStore();
    // Dec 31 → Jan 1 next year.
    const { startIso, endIso } = localDayIntervalUtc(new Date(2026, 11, 31));
    expect(new Date(endIso).getUTCFullYear()).toBe(2027);
    expect(new Date(endIso).getUTCMonth()).toBe(0);
    expect(new Date(endIso).getUTCDate()).toBe(1);
    // Jan 31 → Feb 1 (non-31-day month).
    const feb = localDayIntervalUtc(new Date(2027, 0, 31));
    expect(new Date(feb.endIso).getUTCMonth()).toBe(1);
    expect(new Date(feb.endIso).getUTCDate()).toBeGreaterThanOrEqual(1);
  });
});

describe('RecordingsStore — edit protection on remote updates', () => {
  it('a protected recording is not re-selected by handleRemoteUpdate', async () => {
    vi.resetModules();
    const mockGetRecording = vi.fn(async () => ({ id: 'rec-1' }));
    vi.doMock('../api/recordings', () => ({
      listRecordings: () => Promise.resolve([]),
      searchRecordings: () => Promise.resolve([]),
      getRecording: mockGetRecording,
    }));
    const { recordings } = await import('./recordings.svelte');
    vi.useFakeTimers();

    recordings.selectedRecording = { id: 'rec-1' } as never;
    recordings.protectFromRemoteUpdate('rec-1');
    expect(recordings.isEditProtected('rec-1')).toBe(true);

    recordings.handleRemoteUpdate('rec-1');
    await Promise.resolve(); // let any (wrongly) started selectRecording run
    expect(mockGetRecording).not.toHaveBeenCalled(),
      'protected recording must not be re-fetched';

    recordings.unprotectFromRemoteUpdate('rec-1');
    recordings.handleRemoteUpdate('rec-1');
    await Promise.resolve();
    expect(mockGetRecording).toHaveBeenCalledWith('rec-1');

    vi.useRealTimers();
    vi.doUnmock('../api/recordings');
  });
});
