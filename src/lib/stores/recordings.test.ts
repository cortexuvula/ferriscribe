// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Recording, RecordingSummary } from '../types';
import type { QueryRecordingsOptions } from '../api/recordings';
import {
  dayIntervalUtcForKey,
  parseLocalDateKey,
  fullLocalDateFromKey,
} from '../utils/recordingDates';

// Mock the API layer so we control queryRecordings output + pagination
// offsets (the composite {date, search} Active query) and availability.
const mockQueryRecordings = vi.fn();
const mockListCreationTimes = vi.fn();
const mockCountRecordings = vi.fn();
const mockDeleteAllRecordings = vi.fn();
const mockRestoreRecordings = vi.fn();
const mockDeleteRecording = vi.fn();
const mockRestoreRecording = vi.fn();
const mockListTrashed = vi.fn();
const mockRestoreAllTrashed = vi.fn();
const mockRestoreBetween = vi.fn();
const mockCountBetween = vi.fn();
vi.mock('../api/recordings', () => ({
  queryRecordings: (...args: unknown[]) => mockQueryRecordings(...args),
  listRecordingCreationTimes: (...args: unknown[]) => mockListCreationTimes(...args),
  countRecordings: (...args: unknown[]) => mockCountRecordings(...args),
  deleteAllRecordings: (...args: unknown[]) => mockDeleteAllRecordings(...args),
  restoreRecordings: (...args: unknown[]) => mockRestoreRecordings(...args),
  deleteRecording: (...args: unknown[]) => mockDeleteRecording(...args),
  restoreRecording: (...args: unknown[]) => mockRestoreRecording(...args),
  listTrashedRecordings: (...args: unknown[]) => mockListTrashed(...args),
  restoreAllTrashed: (...args: unknown[]) => mockRestoreAllTrashed(...args),
  restoreRecordingsDeletedBetween: (...args: unknown[]) => mockRestoreBetween(...args),
  countRecordingsDeletedBetween: (...args: unknown[]) => mockCountBetween(...args),
  getRecording: vi.fn(),
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

/// A timestamp at LOCAL NOON of the given day key — constructed in the
/// runtime zone, so it keys back to exactly that day in any runtime zone
/// (a UTC-midnight stamp would flip days in nonzero-offset zones).
function noonIso(key: string): string {
  return new Date(parseLocalDateKey(key).getTime() + 12 * 3_600_000).toISOString();
}

/// Deferred-promise helper for stale-response tests.
function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

/// The options of the most recent composite query (fails loudly if none).
function lastQueryOptions(): QueryRecordingsOptions {
  const last = mockQueryRecordings.mock.lastCall;
  if (!last) throw new Error('queryRecordings was never called');
  return last[0] as QueryRecordingsOptions;
}

function pageOf(prefix: string, n: number, startAt = 0): RecordingSummary[] {
  return Array.from({ length: n }, (_, i) => makeSummary(`${prefix}${startAt + i}`));
}

// Re-import the module FRESHLY so each test gets a clean store instance.
async function freshStore() {
  vi.resetModules();
  return await import('./recordings.svelte');
}

function resetAllMocks() {
  mockQueryRecordings.mockReset();
  mockListCreationTimes.mockReset();
  mockCountRecordings.mockReset();
  mockDeleteAllRecordings.mockReset();
  mockRestoreRecordings.mockReset();
  mockDeleteRecording.mockReset();
  mockRestoreRecording.mockReset();
  mockListTrashed.mockReset();
  mockRestoreAllTrashed.mockReset();
  mockRestoreBetween.mockReset();
  mockCountBetween.mockReset();
  vi.clearAllMocks();
  // Shared defaults: empty DB, everything succeeds unless a test says
  // otherwise — a mutation's fire-and-forget refreshes never explode.
  mockQueryRecordings.mockResolvedValue([]);
  mockListCreationTimes.mockResolvedValue([]);
  mockCountRecordings.mockResolvedValue(0);
  mockListTrashed.mockResolvedValue({ items: [], total: 0 });
}

describe('RecordingsStore — composite query pagination + dedup', () => {
  beforeEach(resetAllMocks);

  it('load() sets hasMore=true when a full page returns', async () => {
    mockQueryRecordings.mockResolvedValue(pageOf('r', 50));
    const { recordings } = await freshStore();

    await recordings.load(50, 0);
    expect(recordings.list).toHaveLength(50);
    expect(recordings.hasMore).toBe(true);
    expect(recordings.nextRawOffset).toBe(50);
  });

  it('load() sets hasMore=false when a partial page returns', async () => {
    mockQueryRecordings.mockResolvedValue([makeSummary('a'), makeSummary('b')]);
    const { recordings } = await freshStore();

    await recordings.load(50, 0);
    expect(recordings.list).toHaveLength(2);
    expect(recordings.hasMore).toBe(false);
    expect(recordings.nextRawOffset).toBe(2);
  });

  it('loadMore() appends the next page, dedupes visible ids, and advances the RAW offset', async () => {
    // First page: 50 items. Second page: 50 items, but r49 is a duplicate
    // (a new recording landed between pages and shifted offsets).
    const page1 = pageOf('r', 50);
    const page2 = pageOf('r', 50, 50);
    page2[0] = makeSummary('r49'); // duplicate of page1's last row

    mockQueryRecordings
      .mockResolvedValueOnce(page1)
      .mockResolvedValueOnce(page2);

    const { recordings } = await freshStore();
    await recordings.load(50, 0);
    await recordings.loadMore();

    // 50 + 49 (one deduped) = 99 VISIBLE rows…
    expect(recordings.list).toHaveLength(99);
    const ids = recordings.list.map((r) => r.id);
    expect(new Set(ids).size).toBe(ids.length);
    // …but the backend consumed 100 RAW rows — the offset must follow the
    // raw count, not the deduped visible length (which would re-fetch
    // r50-forever and strand the tail).
    expect(recordings.nextRawOffset).toBe(100);
    expect(recordings.hasMore).toBe(true);
  });

  it('loadMore() is a no-op when hasMore is false', async () => {
    mockQueryRecordings.mockResolvedValue([makeSummary('a')]);
    const { recordings } = await freshStore();

    await recordings.load(50, 0);
    expect(recordings.hasMore).toBe(false);

    const callsBefore = mockQueryRecordings.mock.calls.length;
    await recordings.loadMore();
    expect(mockQueryRecordings.mock.calls.length).toBe(callsBefore);
  });

  it('loadMore() is a no-op when already loading more', async () => {
    mockQueryRecordings.mockResolvedValue(pageOf('r', 50));
    const { recordings } = await freshStore();

    await recordings.load(50, 0);
    // Simulate in-flight loadMore.
    recordings.loadingMore = true;
    const callsBefore = mockQueryRecordings.mock.calls.length;
    await recordings.loadMore();
    expect(mockQueryRecordings.mock.calls.length).toBe(callsBefore);
  });

  it('loadMore() is a no-op during a first-page load (no paging into an unknown list)', async () => {
    const first = deferred<RecordingSummary[]>();
    mockQueryRecordings.mockImplementationOnce(() => first.promise);
    const { recordings } = await freshStore();

    const loading = recordings.load(50, 0);
    await recordings.loadMore();
    // Only the first-page query was issued.
    expect(mockQueryRecordings).toHaveBeenCalledTimes(1);

    first.resolve(pageOf('r', 50));
    await loading;
    expect(recordings.list).toHaveLength(50);
  });

  it('loadMore() sets hasMore=false when the next page is partial', async () => {
    mockQueryRecordings
      .mockResolvedValueOnce(pageOf('r', 50))
      .mockResolvedValueOnce([makeSummary('r50'), makeSummary('r51')]);
    const { recordings } = await freshStore();

    await recordings.load(50, 0);
    await recordings.loadMore();

    expect(recordings.hasMore).toBe(false);
    expect(recordings.list).toHaveLength(52);
  });

  it('remove() decrements the raw offset when the deleted row was consumed — paging stays complete', async () => {
    mockQueryRecordings
      .mockResolvedValueOnce(pageOf('r', 50))
      .mockResolvedValueOnce([makeSummary('r4'), ...pageOf('n', 49)]);
    mockDeleteRecording.mockResolvedValue(undefined);
    const { recordings } = await freshStore();
    await recordings.load(50, 0);

    await recordings.remove('r5');

    // The removed row occupied a consumed raw position; the next page must
    // start one earlier or the row that shifted into its place is lost.
    expect(recordings.nextRawOffset).toBe(49);
    await recordings.loadMore();
    expect(mockQueryRecordings).toHaveBeenLastCalledWith(
      expect.objectContaining({ offset: 49 }),
    );
    // 49 visible after the delete + 49 fresh (r4 deduped) = 98.
    expect(recordings.list).toHaveLength(98);
    expect(recordings.nextRawOffset).toBe(99);
  });

  it('a stale search response cannot clobber the latest results', async () => {
    // The SearchBar debounce fires overlapping queries; without the
    // request token the first query's response landing LAST would win.
    const { recordings } = await freshStore();

    const slow = deferred<RecordingSummary[]>();
    mockQueryRecordings
      .mockImplementationOnce(() => slow.promise)
      .mockResolvedValueOnce([makeSummary('smith-1')]);

    const p1 = recordings.search('smi');
    const p2 = recordings.search('smith');
    await p2;
    expect(recordings.list.map((r) => r.id)).toEqual(['smith-1']);

    // The stale "smi" response arrives now — it must be discarded.
    slow.resolve([makeSummary('stale-smi')]);
    await p1;
    expect(recordings.list.map((r) => r.id)).toEqual(['smith-1']);
    expect(recordings.loading).toBe(false);
  });

  it('a stale first-page response is discarded and its finally cannot clear a newer busy flag', async () => {
    const { recordings } = await freshStore();

    // A: slow first page. B: supersedes it and resolves immediately.
    const a = deferred<RecordingSummary[]>();
    mockQueryRecordings
      .mockImplementationOnce(() => a.promise)
      .mockResolvedValueOnce([makeSummary('b1')]);

    const pA = recordings.load(50, 0);
    const pB = recordings.load(50, 0);
    await pB;
    expect(recordings.list.map((r) => r.id)).toEqual(['b1']);
    expect(recordings.loading).toBe(false);

    // C: a THIRD load goes in flight while stale A is still pending. If
    // A's finally block cleared `loading` unconditionally it would kill
    // C's spinner and a "done" UI would sit on an unfinished query.
    const c = deferred<RecordingSummary[]>();
    mockQueryRecordings.mockImplementationOnce(() => c.promise);
    const pC = recordings.load(50, 0);
    expect(recordings.loading).toBe(true);

    a.resolve([makeSummary('stale-a')]);
    await pA;
    expect(recordings.loading).toBe(true); // A's stale finally skipped
    expect(recordings.list.map((r) => r.id)).toEqual(['b1']);

    c.resolve([makeSummary('c1')]);
    await pC;
    expect(recordings.loading).toBe(false);
    expect(recordings.list.map((r) => r.id)).toEqual(['c1']);
  });

  it('a stale loadMore response cannot append onto a newer filter', async () => {
    mockQueryRecordings.mockResolvedValueOnce(pageOf('r', 50));
    const { recordings } = await freshStore();
    await recordings.load(50, 0);

    const more = deferred<RecordingSummary[]>();
    mockQueryRecordings.mockImplementationOnce(() => more.promise);
    const pMore = recordings.loadMore();
    expect(recordings.loadingMore).toBe(true);

    // The user narrows the filter mid-flight — the pending page belongs
    // to the OLD filter and must never be appended.
    mockQueryRecordings.mockResolvedValueOnce([makeSummary('s1')]);
    await recordings.search('zebra');
    expect(recordings.list.map((r) => r.id)).toEqual(['s1']);

    more.resolve(pageOf('old', 50));
    await pMore;
    expect(recordings.list.map((r) => r.id)).toEqual(['s1']);
    // The superseding load() reset the flag; the stale finally skipped.
    expect(recordings.loadingMore).toBe(false);
  });
});

describe('RecordingsStore — composite {date, search} filter contract', () => {
  beforeEach(resetAllMocks);

  it('date + search ride ONE composite query with local-midnight bounds', async () => {
    const day = '2026-10-05';
    const { startIso, endIso } = dayIntervalUtcForKey(day);
    mockQueryRecordings.mockResolvedValue([]);
    // The day must exist in availability or the (correct) auto-clear path
    // wipes the selected date the moment the empty availability lands.
    mockListCreationTimes.mockResolvedValue([noonIso(day)]);
    const { recordings } = await freshStore();

    await recordings.setSelectedDate(day);
    await recordings.search('covid');

    expect(mockQueryRecordings).toHaveBeenLastCalledWith({
      search: 'covid',
      startIso,
      endIso,
      limit: 50,
      offset: 0,
    });
  });

  it('a date-only filter sends bounds and no search; a bare load sends neither', async () => {
    const { startIso, endIso } = dayIntervalUtcForKey('2026-10-05');
    mockQueryRecordings.mockResolvedValue([]);
    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-05')]);
    const { recordings } = await freshStore();

    await recordings.setSelectedDate('2026-10-05');
    expect(mockQueryRecordings).toHaveBeenLastCalledWith({
      startIso,
      endIso,
      limit: 50,
      offset: 0,
    });

    await recordings.setSelectedDate(null);
    await recordings.search('');
    const opts = lastQueryOptions();
    expect(Object.keys(opts).sort()).toEqual(['limit', 'offset']);
  });

  it('whitespace-only search means no search filter', async () => {
    mockQueryRecordings.mockResolvedValue([]);
    const { recordings } = await freshStore();

    await recordings.search('   ');
    const opts = lastQueryOptions();
    expect(opts).not.toHaveProperty('search');
  });

  it('date+search results beyond the old list-50/search-20 caps page completely', async () => {
    // The old store ran two unpaginated paths: a 50-row unfiltered list
    // and a top-20 FTS search. A day holding 60 matching recordings was
    // unreachable. The composite query pages all of them.
    const matches = pageOf('m', 60);
    mockQueryRecordings
      .mockResolvedValueOnce(matches.slice(0, 50)) // date-only first page
      .mockResolvedValueOnce(matches.slice(0, 50)) // date+search first page
      .mockResolvedValueOnce(matches.slice(50)); // second (final) page
    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-05')]);
    const { recordings } = await freshStore();

    await recordings.setSelectedDate('2026-10-05');
    await recordings.search('marker');

    expect(recordings.list).toHaveLength(50);
    expect(recordings.hasMore).toBe(true);

    await recordings.loadMore();
    expect(mockQueryRecordings).toHaveBeenLastCalledWith(
      expect.objectContaining({ offset: 50, search: 'marker' }),
    );
    expect(recordings.list).toHaveLength(60);
    expect(recordings.hasMore).toBe(false);
  });

  it('selected day + search survive every background load/refresh', async () => {
    const day = '2026-10-05';
    const { startIso, endIso } = dayIntervalUtcForKey(day);
    mockQueryRecordings.mockResolvedValue([]);
    mockListCreationTimes.mockResolvedValue([noonIso(day)]);
    const { recordings } = await freshStore();

    await recordings.setSelectedDate(day);
    await recordings.search('followup');
    mockQueryRecordings.mockClear();

    // Post-sync loads, post-mutation refreshes — all must carry BOTH.
    await recordings.load();
    await recordings.load();
    await recordings.load();
    expect(mockQueryRecordings).toHaveBeenCalledTimes(3);
    for (const call of mockQueryRecordings.mock.calls) {
      expect(call[0]).toEqual({ search: 'followup', startIso, endIso, limit: 50, offset: 0 });
    }
  });

  it('changing either filter resets paging: offset back to 0, list replaced (not appended)', async () => {
    mockQueryRecordings
      .mockResolvedValueOnce(pageOf('d1', 50)) // date-only first page
      .mockResolvedValueOnce(pageOf('d1', 50)) // date+search first page
      .mockResolvedValueOnce(pageOf('d1', 10, 50)); // second (final) page
    // All three days populated so the auto-clear never fires mid-test.
    mockListCreationTimes.mockResolvedValue([
      noonIso('2026-10-01'),
      noonIso('2026-10-02'),
      noonIso('2026-10-03'),
    ]);
    const { recordings } = await freshStore();

    await recordings.setSelectedDate('2026-10-01');
    await recordings.search('q');
    await recordings.loadMore();
    expect(recordings.list).toHaveLength(60);
    expect(recordings.nextRawOffset).toBe(60);

    // Date change → first page of the new filter only.
    const day2 = dayIntervalUtcForKey('2026-10-02');
    mockQueryRecordings.mockResolvedValueOnce([makeSummary('d2-only')]);
    await recordings.setSelectedDate('2026-10-02');
    expect(mockQueryRecordings).toHaveBeenLastCalledWith({
      search: 'q',
      startIso: day2.startIso,
      endIso: day2.endIso,
      limit: 50,
      offset: 0,
    });
    expect(recordings.list.map((r) => r.id)).toEqual(['d2-only']);
    expect(recordings.nextRawOffset).toBe(1);

    // Clear date preserves search.
    mockQueryRecordings.mockResolvedValueOnce([]);
    await recordings.setSelectedDate(null);
    const afterClear = lastQueryOptions();
    expect(afterClear.search).toBe('q');
    expect(afterClear).not.toHaveProperty('startIso');

    // Clear search preserves date.
    mockQueryRecordings.mockResolvedValueOnce([]);
    await recordings.setSelectedDate('2026-10-03');
    await recordings.search('');
    const afterSearchClear = lastQueryOptions();
    expect(afterSearchClear).not.toHaveProperty('search');
    expect(afterSearchClear.startIso).toBe(dayIntervalUtcForKey('2026-10-03').startIso);
  });

  it('selecting the same date is a no-op (no redundant reload)', async () => {
    mockQueryRecordings.mockResolvedValue([]);
    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-05')]);
    const { recordings } = await freshStore();

    await recordings.setSelectedDate('2026-10-05');
    mockQueryRecordings.mockClear();
    await recordings.setSelectedDate('2026-10-05');
    expect(mockQueryRecordings).not.toHaveBeenCalled();
  });

  it('filter changes and refreshes never touch the editor selection (no automatic re-choose)', async () => {
    mockQueryRecordings.mockResolvedValue([]);
    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-05'), noonIso('2026-10-06')]);
    const { recordings } = await freshStore();
    recordings.selectedRecording = { id: 'keep-me' } as unknown as Recording;

    await recordings.setSelectedDate('2026-10-05');
    await recordings.search('narrow');
    await recordings.load();
    await recordings.load();
    await recordings.setSelectedDate(null);

    expect(recordings.selectedRecording?.id).toBe('keep-me');
  });
});

describe('RecordingsStore — availability (date calendar data)', () => {
  beforeEach(resetAllMocks);

  it('load() refreshes availability; counts group whole timestamps into local days', async () => {
    mockListCreationTimes.mockResolvedValue([
      noonIso('2026-10-05'),
      noonIso('2026-10-05'),
      noonIso('2026-10-06'),
    ]);
    const { recordings } = await freshStore();

    await recordings.load();

    expect(mockListCreationTimes).toHaveBeenCalled();
    expect(recordings.availabilityCounts.get('2026-10-05')).toBe(2);
    expect(recordings.availabilityCounts.get('2026-10-06')).toBe(1);
    expect(recordings.availabilityLoaded).toBe(true);
    expect(recordings.availabilityError).toBeNull();
    expect(recordings.availabilityLoading).toBe(false);
  });

  it('availability covers the WHOLE database, independent of the active search', async () => {
    mockListCreationTimes.mockResolvedValue([
      noonIso('2026-10-05'),
      noonIso('2026-10-05'),
      noonIso('2026-10-06'),
    ]);
    // The filtered list shows only one match — availability must not
    // inherit the filter (the calendar's populated days are global).
    mockQueryRecordings.mockResolvedValue([makeSummary('only-match')]);
    const { recordings } = await freshStore();

    await recordings.search('rare-term');

    expect(recordings.list).toHaveLength(1);
    expect(recordings.availabilityCounts.get('2026-10-05')).toBe(2);
    expect(recordings.availabilityCounts.get('2026-10-06')).toBe(1);
  });

  it('deleting the selected day\'s last recording clears ONLY the date, keeps search, reloads, and announces politely', async () => {
    const day = '2026-10-05';
    mockListCreationTimes.mockResolvedValue([noonIso(day)]);
    mockQueryRecordings.mockResolvedValue([makeSummary('solo')]);
    mockDeleteRecording.mockResolvedValue(undefined);
    const { recordings } = await freshStore();

    await recordings.setSelectedDate(day);
    await recordings.search('solo');
    expect(recordings.selectedDate).toBe(day);

    // Post-delete availability: the day is gone. The reload after the
    // auto-clear runs with search but NO date bounds.
    mockListCreationTimes.mockResolvedValue([]);
    mockQueryRecordings.mockResolvedValue([]);

    await recordings.remove('solo');

    await vi.waitFor(() => {
      expect(recordings.selectedDate).toBeNull();
      expect(recordings.dateClearedNotice).toContain(fullLocalDateFromKey(day));
      expect(recordings.dateClearedNotice).toContain('Showing all dates');
    });
    expect(recordings.searchQuery).toBe('solo'); // search preserved
    const lastOpts = lastQueryOptions();
    expect(lastOpts.search).toBe('solo');
    expect(lastOpts).not.toHaveProperty('startIso');
    expect(recordings.availabilityCounts.size).toBe(0);
    // The auto-clear reload re-triggers availability exactly once more —
    // no recursive loop (date is null by then; calls stay bounded).
    const calls = mockListCreationTimes.mock.calls.length;
    await recordings.load();
    expect(mockListCreationTimes.mock.calls.length).toBe(calls + 1);
  });

  it('an availability response issued BEFORE a mutation cannot clear the date', async () => {
    const day = '2026-10-05';
    mockListCreationTimes.mockResolvedValue([noonIso(day)]);
    mockQueryRecordings.mockResolvedValue([makeSummary('row')]);
    const { recordings } = await freshStore();
    await recordings.setSelectedDate(day);

    // R1: issued now, resolves LATE, with pre-mutation data where the day
    // is already absent (a stale pre-delete snapshot).
    const r1 = deferred<string[]>();
    mockListCreationTimes.mockImplementationOnce(() => r1.promise);
    const pR1 = recordings.refreshAvailability();

    // The mutation bumps the data generation; R2 (post-mutation truth)
    // says the day still has a recording (e.g. restored from elsewhere).
    mockListCreationTimes.mockResolvedValueOnce([noonIso(day)]);
    mockDeleteRecording.mockResolvedValue(undefined);
    mockQueryRecordings.mockResolvedValue([]);
    await recordings.remove('other-row');

    await vi.waitFor(() => expect(recordings.availabilityCounts.get(day)).toBe(1));

    r1.resolve([]); // stale pre-mutation reply: day "gone"
    await pR1;
    expect(recordings.selectedDate).toBe(day);
    expect(recordings.availabilityCounts.get(day)).toBe(1);
    expect(recordings.dateClearedNotice).toBeNull();
  });

  it('an availability response issued BEFORE a mutation cannot repopulate dead days', async () => {
    const day = '2026-10-05';
    mockListCreationTimes.mockResolvedValue([noonIso(day), noonIso(day)]);
    mockQueryRecordings.mockResolvedValue(pageOf('d', 2));
    const { recordings } = await freshStore();
    await recordings.setSelectedDate(day);

    // R1 in flight with BOTH rows (pre-delete shape).
    const r1 = deferred<string[]>();
    mockListCreationTimes.mockImplementationOnce(() => r1.promise);
    const pR1 = recordings.refreshAvailability();

    // Post-delete world: the day is empty. Both removes fetch CURRENT
    // (post-mutation) availability saying so — the second remove is the
    // one that truly ends the day.
    mockListCreationTimes.mockResolvedValue([]);
    mockQueryRecordings.mockResolvedValue([]);
    await recordings.remove('d1');
    await recordings.remove('d0');

    await vi.waitFor(() => expect(recordings.availabilityCounts.size).toBe(0));
    // The day really ended — the auto-clear fires from CURRENT data.
    await vi.waitFor(() => expect(recordings.selectedDate).toBeNull());

    // Now the stale pre-delete reply lands with the day populated — it
    // must be discarded, not applied over the dead-day truth.
    r1.resolve([noonIso(day), noonIso(day)]);
    await pR1;
    expect(recordings.availabilityCounts.size).toBe(0);
  });

  it('a failed availability fetch never looks like empty dates and never clears the date', async () => {
    const day = '2026-10-05';
    mockListCreationTimes.mockResolvedValue([noonIso(day)]);
    const { recordings } = await freshStore();
    await recordings.setSelectedDate(day);
    expect(recordings.availabilityCounts.get(day)).toBe(1);

    mockListCreationTimes.mockRejectedValue(new Error('db locked'));
    await recordings.refreshAvailability();

    // Previous counts stay visible with a retryable error; the date
    // filter is untouched (a fetch failure is not "day disappeared").
    expect(recordings.availabilityError).toBe('unexpected error');
    expect(recordings.availabilityCounts.get(day)).toBe(1);
    expect(recordings.selectedDate).toBe(day);
    expect(recordings.availabilityLoading).toBe(false);
    expect(recordings.dateClearedNotice).toBeNull();
  });

  it('availabilityLoading is set during flight; an in-flight same-generation fetch is reused, not stacked', async () => {
    const { recordings } = await freshStore();

    const r1 = deferred<string[]>();
    mockListCreationTimes.mockImplementationOnce(() => r1.promise);
    const pR1 = recordings.refreshAvailability();
    expect(recordings.availabilityLoading).toBe(true);

    // A filter refresh while R1 is in flight (no data mutation) reuses it.
    await recordings.load();
    expect(mockListCreationTimes).toHaveBeenCalledTimes(1);

    r1.resolve([noonIso('2026-10-05')]);
    await pR1;
    expect(recordings.availabilityCounts.get('2026-10-05')).toBe(1);
    expect(recordings.availabilityLoading).toBe(false);

    // Once idle, the next load fetches again.
    mockListCreationTimes.mockResolvedValue([]);
    await recordings.load();
    expect(mockListCreationTimes).toHaveBeenCalledTimes(2);
  });

  it('a manually chosen date clears a previous auto-clear notice', async () => {
    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-05')]);
    const { recordings } = await freshStore();
    await recordings.setSelectedDate('2026-10-05');

    mockListCreationTimes.mockResolvedValue([]);
    await recordings.refreshAvailability();
    await vi.waitFor(() => expect(recordings.dateClearedNotice).not.toBeNull());

    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-02')]);
    mockQueryRecordings.mockResolvedValue([]);
    await recordings.setSelectedDate('2026-10-02');
    expect(recordings.dateClearedNotice).toBeNull();
  });
});

describe('RecordingsStore — list/loadMore error states', () => {
  beforeEach(resetAllMocks);

  it('a first-page failure after a filter change shows a retryable error, never stale wrong-filter rows', async () => {
    mockQueryRecordings.mockResolvedValue([makeSummary('a'), makeSummary('b')]);
    const { recordings } = await freshStore();
    await recordings.load();
    expect(recordings.list).toHaveLength(2);

    mockQueryRecordings.mockRejectedValue(new Error('boom'));
    await recordings.setSelectedDate('2026-10-05');

    // The old rows belonged to the previous filter — cleared, not shown
    // under the new one. The failure is an explicit sanitized error, not
    // a fake empty success.
    expect(recordings.list).toHaveLength(0);
    expect(recordings.listError).toBe('unexpected error');
    expect(recordings.loading).toBe(false);

    // Retry succeeds and clears the error.
    mockQueryRecordings.mockResolvedValue([makeSummary('d1')]);
    await recordings.load();
    expect(recordings.list.map((r) => r.id)).toEqual(['d1']);
    expect(recordings.listError).toBeNull();
  });

  it('a same-filter refresh failure retains the current rows alongside the error', async () => {
    mockQueryRecordings.mockResolvedValue([makeSummary('a'), makeSummary('b')]);
    const { recordings } = await freshStore();
    await recordings.load();

    mockQueryRecordings.mockRejectedValueOnce(new Error('db busy'));
    await recordings.load();

    expect(recordings.list).toHaveLength(2); // still current-filter rows
    expect(recordings.listError).toBe('unexpected error');
    expect(recordings.loading).toBe(false);
  });

  it('a loadMore failure retains valid results with a retry; retry appends and clears the error', async () => {
    mockQueryRecordings
      .mockResolvedValueOnce(pageOf('r', 50))
      .mockRejectedValueOnce(new Error('io'))
      .mockResolvedValueOnce([makeSummary('r50')]);
    const { recordings } = await freshStore();

    await recordings.load(50, 0);
    await recordings.loadMore();

    expect(recordings.list).toHaveLength(50);
    expect(recordings.loadMoreError).toBe('unexpected error');
    expect(recordings.hasMore).toBe(true);
    expect(recordings.loadingMore).toBe(false);

    await recordings.loadMore();
    expect(recordings.list).toHaveLength(51);
    expect(recordings.loadMoreError).toBeNull();
    expect(recordings.hasMore).toBe(false);
  });
});

describe('RecordingsStore — mutation invalidation + success/refresh-error separation', () => {
  beforeEach(resetAllMocks);

  it('an in-flight first-page query is invalidated BEFORE the delete — its pre-delete page never lands', async () => {
    mockQueryRecordings.mockResolvedValue([]);
    const { recordings } = await freshStore();
    await recordings.load();

    // A slow refresh whose (pre-delete) page still contains the row
    // being deleted.
    const stale = deferred<RecordingSummary[]>();
    mockQueryRecordings.mockImplementationOnce(() => stale.promise);
    const pStale = recordings.load();

    mockDeleteRecording.mockResolvedValue(undefined);
    await recordings.remove('x');

    stale.resolve([makeSummary('x'), makeSummary('y')]);
    await pStale;
    expect(recordings.list).toHaveLength(0); // stale page discarded whole
  });

  it('restore() never injects the cached summary outside the current filters', async () => {
    // The active search filters to 'zebra' rows; the restored recording
    // does not match. The old optimistic prepend put it in the list
    // anyway — the reload must be the only source of truth.
    mockQueryRecordings.mockResolvedValue([]);
    const { recordings } = await freshStore();
    await recordings.search('zebra');
    recordings.lastDeleted = makeSummary('non-zebra');

    mockRestoreRecording.mockResolvedValue(undefined);
    await recordings.restore('non-zebra');

    expect(recordings.list.some((r) => r.id === 'non-zebra')).toBe(false);
    expect(mockQueryRecordings).toHaveBeenLastCalledWith(
      expect.objectContaining({ search: 'zebra' }),
    );
  });

  it('a successful destructive mutation stays successful when follow-up refreshes fail', async () => {
    mockQueryRecordings.mockResolvedValue([makeSummary('a')]);
    const { recordings } = await freshStore();
    await recordings.load();

    mockDeleteRecording.mockResolvedValue(undefined);
    mockListCreationTimes.mockRejectedValue(new Error('db busy'));
    await expect(recordings.remove('a')).resolves.toBeUndefined();
    expect(recordings.availabilityError).toBe('unexpected error'); // refresh error, distinct

    // Same contract on the restore path, where the list reload fails.
    mockRestoreRecording.mockResolvedValue(undefined);
    mockQueryRecordings.mockRejectedValue(new Error('db busy'));
    await expect(recordings.restore('a')).resolves.toBeUndefined();
    expect(recordings.listError).toBe('unexpected error'); // refresh error, not mutation failure
  });

  it('every mutation path refreshes availability', async () => {
    mockDeleteRecording.mockResolvedValue(undefined);
    mockDeleteAllRecordings.mockResolvedValue({ count: 1, ids: ['a'] });
    mockRestoreRecording.mockResolvedValue(undefined);
    mockRestoreRecordings.mockResolvedValue({ count: 1, ids: ['a'] });
    mockRestoreAllTrashed.mockResolvedValue({ count: 0, ids: [] });
    mockRestoreBetween.mockResolvedValue({ count: 0, ids: [] });
    const { recordings } = await freshStore();

    const countAfter = async (label: string, action: () => Promise<unknown>) => {
      const before = mockListCreationTimes.mock.calls.length;
      await action();
      await vi.waitFor(() =>
        expect(mockListCreationTimes.mock.calls.length).toBeGreaterThan(before),
        { timeout: 2000 },
      ).catch(() => {
        throw new Error(`${label} did not refresh availability`);
      });
    };

    await countAfter('remove', () => recordings.remove('a'));
    await countAfter('restore', () => recordings.restore('a'));
    await countAfter('removeAll', () => recordings.removeAll());
    await countAfter('undoMoveAll', () => recordings.undoMoveAll());
    await countAfter('restoreTrashed', () => recordings.restoreTrashed(['a']));
    await countAfter('restoreAllFromTrash', () => recordings.restoreAllFromTrash());
    await countAfter('restoreTrashedOnDate', () =>
      recordings.restoreTrashedOnDate(new Date(2026, 9, 5)),
    );
  });

  it('removeAll() clears pagination state and empties the selected date via the availability path', async () => {
    mockQueryRecordings.mockResolvedValue(pageOf('r', 50));
    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-05')]);
    const { recordings } = await freshStore();
    await recordings.setSelectedDate('2026-10-05');
    await recordings.loadMore();

    mockDeleteAllRecordings.mockResolvedValue({ count: 60, ids: [] });
    mockListCreationTimes.mockResolvedValue([]);
    mockQueryRecordings.mockResolvedValue([]);
    await recordings.removeAll();

    expect(recordings.list).toHaveLength(0);
    expect(recordings.hasMore).toBe(false);
    expect(recordings.nextRawOffset).toBe(0);
    await vi.waitFor(() => {
      expect(recordings.selectedDate).toBeNull();
      expect(recordings.dateClearedNotice).toContain('Showing all dates');
    });
  });
});

describe('RecordingsStore — Active return hook (UI batch)', () => {
  beforeEach(resetAllMocks);

  it('onActiveViewReturn() reloads the first page + availability under CURRENT filters', async () => {
    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-05')]);
    mockQueryRecordings.mockResolvedValue([]);
    const { recordings } = await freshStore();
    await recordings.setSelectedDate('2026-10-05');
    await recordings.search('q');
    mockQueryRecordings.mockClear();
    mockListCreationTimes.mockClear();

    await recordings.onActiveViewReturn();

    expect(mockQueryRecordings).toHaveBeenCalledTimes(1);
    expect(mockQueryRecordings).toHaveBeenCalledWith(
      expect.objectContaining({ search: 'q' }),
    );
    expect(mockListCreationTimes).toHaveBeenCalledTimes(1);
  });
});

describe('RecordingsStore — Move all to Trash (D1/D2 contract)', () => {
  beforeEach(resetAllMocks);

  it('load() refreshes the authoritative active count alongside the list', async () => {
    mockQueryRecordings.mockResolvedValue([makeSummary('a'), makeSummary('b')]);
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
    mockQueryRecordings.mockResolvedValue([makeSummary('a')]);
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
    mockDeleteAllRecordings.mockResolvedValue({ count: 1, ids: ['id-a'] });
    const restore1 = deferred<{ count: number; ids: string[] }>();
    mockRestoreRecordings.mockImplementationOnce(() => restore1.promise);
    const { recordings } = await freshStore();
    await recordings.removeAll();

    const first = recordings.undoMoveAll();
    // Second click while the first restore is still in flight.
    await expect(recordings.undoMoveAll()).rejects.toThrow(/already in progress/);
    restore1.resolve({ count: 1, ids: ['id-a'] });
    await expect(first).resolves.toBe(1);
    expect(mockRestoreRecordings).toHaveBeenCalledTimes(1);
  });

  it('undoMoveAll() reports the ACTUAL restored count when some ids were purged', async () => {
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

describe('RecordingsStore — Trash staleness after mutations', () => {
  beforeEach(resetAllMocks);

  it('remove() refreshes a loaded trash list — the new row appears and the total matches', async () => {
    mockQueryRecordings.mockResolvedValue([makeSummary('a')]);
    mockDeleteRecording.mockResolvedValue(undefined);
    mockListTrashed
      .mockResolvedValueOnce({ items: [makeTrashed('older')], total: 1 })
      .mockResolvedValueOnce({
        items: [makeTrashed('older'), makeTrashed('a')],
        total: 2,
      });
    const { recordings } = await freshStore();
    await recordings.load();
    await recordings.loadTrashed();

    await recordings.remove('a');

    // The trash reload is fire-and-forget — wait for it to land.
    await vi.waitFor(() => {
      expect(recordings.trashedList.map((r) => r.id)).toEqual(['older', 'a']);
      expect(recordings.trashedTotal).toBe(2);
    });
  });

  it('remove() in the badge-only boot state (empty list, zero total) refreshes just the badge', async () => {
    mockQueryRecordings.mockResolvedValue([makeSummary('a')]);
    mockDeleteRecording.mockResolvedValue(undefined);
    mockListTrashed.mockResolvedValue({ items: [], total: 1 });
    const { recordings } = await freshStore();
    await recordings.load();

    await recordings.remove('a');

    // No trash page was ever loaded — only the count runs, preserving the
    // documented boot behavior (the page loads on Trash entry).
    await vi.waitFor(() => expect(mockListTrashed).toHaveBeenCalledTimes(1));
    expect(mockListTrashed).toHaveBeenCalledWith(0, 0);
    expect(recordings.trashedList).toHaveLength(0);
    expect(recordings.trashedTotal).toBe(1);
  });

  it('restore() refreshes BOTH lists — the row leaves the trash and the badge decrements', async () => {
    mockQueryRecordings.mockResolvedValue([makeSummary('a')]);
    mockRestoreRecording.mockResolvedValue(undefined);
    const { recordings } = await freshStore();
    recordings.lastDeleted = makeSummary('a');
    recordings.trashedList = [makeTrashed('a')];
    recordings.trashedTotal = 1;

    await recordings.restore('a');

    expect(recordings.trashedList).toHaveLength(0);
    expect(recordings.trashedTotal).toBe(0);
    // The trash reload was a PAGE fetch, not just the (0,0) count.
    expect(mockListTrashed).toHaveBeenCalledWith(50, 0);
  });

  it('removeAll() reloads a loaded trash list with the moved rows', async () => {
    mockDeleteAllRecordings.mockResolvedValue({ count: 2, ids: ['a', 'b'] });
    mockListTrashed
      .mockResolvedValueOnce({ items: [makeTrashed('x')], total: 1 })
      .mockResolvedValueOnce({
        items: [makeTrashed('x'), makeTrashed('a'), makeTrashed('b')],
        total: 3,
      });
    const { recordings } = await freshStore();
    await recordings.loadTrashed();

    await recordings.removeAll();

    await vi.waitFor(() => {
      expect(recordings.trashedList.map((r) => r.id)).toEqual(['x', 'a', 'b']);
      expect(recordings.trashedTotal).toBe(3);
    });
  });

  it('undoMoveAll() refreshes the trash list + total, not just the active list', async () => {
    mockDeleteAllRecordings.mockResolvedValue({ count: 2, ids: ['a', 'b'] });
    mockRestoreRecordings.mockResolvedValue({ count: 2, ids: ['a', 'b'] });
    const { recordings } = await freshStore();
    await recordings.removeAll();

    const restored = await recordings.undoMoveAll();

    expect(restored).toBe(2);
    // refreshAfterMutation ran the trash PAGE reload — the rows left Trash.
    expect(mockListTrashed).toHaveBeenCalledWith(50, 0);
    expect(recordings.trashedList).toHaveLength(0);
    expect(recordings.trashedTotal).toBe(0);
  });

  it('loadMoreTrashed() keeps hasMore=true when a full page re-delivers an existing row', async () => {
    const page1 = Array.from({ length: 50 }, (_, i) => makeTrashed(`t${i}`));
    // Full page, but t0 is a re-delivery of a row already listed (offsets
    // shifted between fetches) — only 49 rows are fresh.
    const page2 = [
      makeTrashed('t0'),
      ...Array.from({ length: 49 }, (_, i) => makeTrashed(`u${i}`)),
    ];
    mockListTrashed
      .mockResolvedValueOnce({ items: page1, total: 120 })
      .mockResolvedValueOnce({ items: page2, total: 120 });
    const { recordings } = await freshStore();

    await recordings.loadTrashed();
    await recordings.loadMoreTrashed();

    expect(recordings.trashedList).toHaveLength(99);
    // The RAW page was full and under the total — pagination must continue
    // (the old fresh.length-based flag ended it early at 120 total).
    expect(recordings.trashedHasMore).toBe(true);
  });
});

describe('RecordingsStore — Trash state + restore paths (D4/D8)', () => {
  beforeEach(resetAllMocks);

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
    const slow = deferred<{ items: unknown[]; total: number }>();
    mockListTrashed
      .mockImplementationOnce(() => slow.promise)
      .mockResolvedValueOnce({ items: [makeTrashed('fresh')], total: 1 });
    const { recordings } = await freshStore();

    const slowP = recordings.loadTrashed();
    await recordings.loadTrashed();
    expect(recordings.trashedList.map((r) => r.id)).toEqual(['fresh']);

    slow.resolve({ items: [makeTrashed('stale')], total: 99 });
    await slowP;
    expect(recordings.trashedList.map((r) => r.id)).toEqual(['fresh']);
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
    const blocked = deferred<{ items: unknown[]; total: number }>();
    mockListTrashed.mockImplementationOnce(() => blocked.promise);

    const first = recordings.restoreTrashed(['t0']);
    await expect(recordings.restoreTrashed(['t1'])).rejects.toThrow(/already in progress/);
    blocked.resolve({ items: [], total: 0 });
    await expect(first).resolves.toBe(1);

    expect(mockRestoreRecordings).toHaveBeenCalledWith(['t0']);
    expect(mockRestoreRecordings).toHaveBeenCalledTimes(1);
  });

  it('restoreAllFromTrash() rides the dedicated command and refreshes both lists', async () => {
    mockRestoreAllTrashed.mockResolvedValue({ count: 3, ids: ['a', 'b', 'c'] });
    const { recordings } = await freshStore();

    await expect(recordings.restoreAllFromTrash()).resolves.toBe(3);

    expect(mockRestoreAllTrashed).toHaveBeenCalledTimes(1);
    expect(mockQueryRecordings).toHaveBeenCalled();
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
    // Dec 31 → Jan 1 next year. Compare with LOCAL getters — the bounds
    // are local midnights, so their local calendar fields are exact
    // regardless of the machine's timezone.
    const dec = localDayIntervalUtc(new Date(2026, 11, 31));
    expect(new Date(dec.startIso).getDate()).toBe(31);
    expect(new Date(dec.startIso).getMonth()).toBe(11);
    expect(new Date(dec.endIso).getDate()).toBe(1);
    expect(new Date(dec.endIso).getMonth()).toBe(0);
    expect(new Date(dec.endIso).getFullYear()).toBe(2027);
    // Jan 31 → Feb 1 (non-31-day month).
    const feb = localDayIntervalUtc(new Date(2027, 0, 31));
    expect(new Date(feb.endIso).getMonth()).toBe(1);
    expect(new Date(feb.endIso).getDate()).toBe(1);
  });

  it('dayIntervalUtcForKey(key) matches localDayIntervalUtc semantics in the runtime zone', async () => {
    // The date filter's day bounds and the Trash restore-by-date window
    // must classify boundary instants identically — pinned in whatever
    // zone the test process runs (DST days included; see the fixed-zone
    // proofs in utils/recordingDates.test.ts).
    const { localDayIntervalUtc } = await freshStore();
    for (const key of ['2026-03-08', '2026-10-05', '2026-12-31', '2027-01-31']) {
      expect(dayIntervalUtcForKey(key)).toEqual(
        localDayIntervalUtc(parseLocalDateKey(key)),
      );
    }
  });
});

// ── RC-1: mutation COMPLETION fences ─────────────────────────────────────
//
// Codie's blocking finding (review digest 26f74507…): invalidation bumps
// the list token + data generation BEFORE the mutation's await, but a read
// issued DURING that await window (search debounce, tab return, calendar
// pick) snapshots PRE-commit state while holding the post-bump token and
// the still-current generation — BOTH fences pass. The correction
// re-invalidates AFTER each successful mutation await (at commit), before
// surgery/post-commit refreshes, on every mutation path.
//
// The harness mirrors Codie's browser probe semantics: reads snapshot the
// world SYNCHRONOUSLY at request time (pre-commit while a mutation
// pends — WAL concurrent readers) and deliver only when the test says;
// PRE-commit snapshots are delivered LAST — after post-commit truth —
// the most adversarial ordering a broken fence can face.

interface WorldRow {
  id: string;
  day: string;
  trashed?: boolean;
}

function useScriptedWorld(seed: WorldRow[]) {
  const rows = seed.map((r) => ({ trashed: false, ...r }));
  const active = () => rows.filter((r) => !r.trashed);
  const pending: { idx: number; delivered: boolean; deliver: () => void }[] = [];
  let seq = 0;
  let hold = false;

  /// A read mock: snapshots `compute()` at REQUEST time, defers delivery
  /// while `hold` is armed (the test delivers in scripted order).
  function scripted<T>(compute: () => T): () => Promise<T> {
    return () => {
      const value = compute();
      let resolve!: (v: T) => void;
      const promise = new Promise<T>((res) => {
        resolve = res;
      });
      const entry: { idx: number; delivered: boolean; deliver: () => void } = {
        idx: seq++,
        delivered: false,
        deliver: () => {
          entry.delivered = true;
          resolve(value);
        },
      };
      pending.push(entry);
      if (!hold) entry.deliver();
      return promise;
    };
  }

  mockQueryRecordings.mockImplementation(scripted(() => active().map((r) => makeSummary(r.id))));
  mockListCreationTimes.mockImplementation(scripted(() => active().map((r) => noonIso(r.day))));
  mockCountRecordings.mockImplementation(scripted(() => active().length));
  // Trash reads are not part of the race under test — resolve immediately
  // from the live world so loadTrashed/refreshTrashedTotal never wedge.
  mockListTrashed.mockImplementation(async () => ({
    items: [],
    total: rows.filter((r) => r.trashed).length,
  }));

  const flush = () => new Promise<void>((r) => setTimeout(r, 0));

  return {
    rows,
    active,
    flush,
    arm() {
      hold = true;
    },
    nextSeq() {
      return seq;
    },
    /// Deliver every read issued at/after `seq` — cascading, because a
    /// delivered availability can trigger a reload that issues more reads.
    /// The leading flush lets the mutation's post-await tail ISSUE its
    /// reads before the first sweep looks for them.
    async deliverSince(seqAtCommit: number): Promise<void> {
      for (let round = 0; round < 12; round++) {
        await flush();
        const fresh = pending.filter((e) => !e.delivered && e.idx >= seqAtCommit);
        if (fresh.length === 0) return;
        for (const e of fresh) e.deliver();
        await flush();
      }
      throw new Error('post-commit reads never settled (cascade too deep)');
    },
    /// Deliver everything still pending (the pre-commit snapshots) — LAST.
    deliverRemaining(): void {
      for (const e of pending) if (!e.delivered) e.deliver();
    },
    expectedCounts(): Map<string, number> {
      const counts = new Map<string, number>();
      for (const r of active()) counts.set(r.day, (counts.get(r.day) ?? 0) + 1);
      return counts;
    },
  };
}

type FenceKind =
  | 'remove'
  | 'removeAll'
  | 'restore'
  | 'undoMoveAll'
  | 'restoreTrashed'
  | 'restoreAllFromTrash'
  | 'restoreByDate';

const FENCE_KINDS: FenceKind[] = [
  'remove',
  'removeAll',
  'restore',
  'undoMoveAll',
  'restoreTrashed',
  'restoreAllFromTrash',
  'restoreByDate',
];

describe('RecordingsStore — mutation completion fences (RC-1)', () => {
  beforeEach(resetAllMocks);

  for (const kind of FENCE_KINDS) {
    for (const concurrent of [true, false]) {
      it(`${kind}: a read issued DURING the mutation delivers pre-commit state that cannot survive the commit${concurrent ? ' (concurrent load)' : ' (no-concurrent-load control)'}`, async () => {
        const D = '2026-10-05'; // the moved row's day
        const E = '2026-09-01'; // two stable rows on another day
        const isDelete = kind === 'remove' || kind === 'removeAll';
        const seed: WorldRow[] = isDelete
          ? [
              { id: 'victim', day: D },
              { id: 'e1', day: E },
              { id: 'e2', day: E },
            ]
          : [
              { id: 'victim', day: D, trashed: true },
              { id: 'e1', day: E },
              { id: 'e2', day: E },
            ];
        const w = useScriptedWorld(seed);
        const { recordings } = await freshStore();

        // Stable initial state (reads auto-deliver while hold is off).
        await recordings.load();
        if (isDelete) await recordings.setSelectedDate(D);
        if (kind === 'restore') recordings.lastDeleted = makeSummary('victim');
        if (kind === 'undoMoveAll') recordings.lastDeletedAllIds = ['victim'];
        // Editor selection/drafts must survive every fenced mutation.
        recordings.selectedRecording = { id: 'e1' } as unknown as Recording;

        // Hold the mutation on a deferred; hold read delivery.
        const mut = deferred<unknown>();
        if (kind === 'remove') mockDeleteRecording.mockReturnValue(mut.promise);
        else if (kind === 'removeAll') mockDeleteAllRecordings.mockReturnValue(mut.promise);
        else if (kind === 'restore') mockRestoreRecording.mockReturnValue(mut.promise);
        else if (kind === 'undoMoveAll' || kind === 'restoreTrashed')
          mockRestoreRecordings.mockReturnValue(mut.promise);
        else if (kind === 'restoreAllFromTrash') mockRestoreAllTrashed.mockReturnValue(mut.promise);
        else mockRestoreBetween.mockReturnValue(mut.promise);
        w.arm();

        const start = (): Promise<unknown> => {
          switch (kind) {
            case 'remove':
              return recordings.remove('victim');
            case 'removeAll':
              return recordings.removeAll();
            case 'restore':
              return recordings.restore('victim');
            case 'undoMoveAll':
              return recordings.undoMoveAll();
            case 'restoreTrashed':
              return recordings.restoreTrashed(['victim']);
            case 'restoreAllFromTrash':
              return recordings.restoreAllFromTrash();
            case 'restoreByDate':
              return recordings.restoreTrashedOnDate(parseLocalDateKey(D));
          }
        };
        const pMut = start();
        await w.flush();

        // THE INTERLEAVING (Codie probe shape): a load() starts while the
        // mutation is pending — search debounce / Active-tab return. Its
        // reads snapshot PRE-commit and deliver AFTER the commit.
        if (concurrent) {
          void recordings.load();
          await w.flush();
        }

        // COMMIT: the world changes, then the mutation promise resolves.
        if (kind === 'removeAll') {
          for (const r of w.rows) if (!r.trashed) r.trashed = true;
        } else {
          const victim = w.rows.find((r) => r.id === 'victim')!;
          victim.trashed = isDelete;
        }
        const seqAtCommit = w.nextSeq();
        mut.resolve(
          kind === 'remove' || kind === 'restore'
            ? undefined
            : {
                count: kind === 'removeAll' ? 3 : 1,
                ids: kind === 'removeAll' ? ['victim', 'e1', 'e2'] : ['victim'],
              },
        );

        // Post-commit truth lands first (completing the mutation chain).
        await w.deliverSince(seqAtCommit);
        await pMut;

        // ADVERSARIAL: only now deliver the PRE-commit snapshots — a
        // broken completion fence lets them overwrite the truth.
        w.deliverRemaining();
        await w.flush();
        await w.flush();

        // ── Post-commit truth ──
        expect(recordings.list.map((r) => r.id)).toEqual(w.active().map((r) => r.id));
        expect(recordings.availabilityCounts).toEqual(w.expectedCounts());
        expect(recordings.activeTotal).toBe(w.active().length);
        // Busy flags stay generation-safe — no fence may strand spinners.
        expect(recordings.loading).toBe(false);
        expect(recordings.loadingMore).toBe(false);
        expect(recordings.availabilityLoading).toBe(false);
        expect(recordings.restoring).toBe(false);
        expect(recordings.selectedRecording?.id ?? null).toBe(
          kind === 'removeAll' ? null : 'e1',
        );

        if (isDelete) {
          // The emptied selected day auto-clears with the polite notice
          // (spec §9) — the exact scenario-A behavior Codie's probe showed
          // being silently skipped by the coalescing hole.
          expect(recordings.selectedDate).toBeNull();
          expect(recordings.dateClearedNotice).toContain('No active recordings remain on');
          expect(recordings.dateClearedNotice).toContain('Showing all dates');
        } else {
          // The restored day is offered again by the calendar (Codie
          // scenario C: the list was fine, the calendar never heard).
          expect(recordings.availabilityCounts.get(D)).toBe(1);
          expect(recordings.selectedDate).toBeNull();
          expect(recordings.dateClearedNotice).toBeNull();
        }
      });
    }
  }

  it('concurrent mutations cannot defeat the completion fence — mutation 1’s post-commit refresh dies at mutation 2’s commit', async () => {
    const D1 = '2026-10-05';
    const D2 = '2026-10-04';
    const E = '2026-09-01';
    const w = useScriptedWorld([
      { id: 'v1', day: D1 },
      { id: 'v2', day: D2 },
      { id: 'e', day: E },
    ]);
    const { recordings } = await freshStore();
    await recordings.load();

    const del1 = deferred<void>();
    const del2 = deferred<void>();
    mockDeleteRecording.mockImplementationOnce(() => del1.promise);
    mockDeleteRecording.mockImplementationOnce(() => del2.promise);
    w.arm();

    const p1 = recordings.remove('v1');
    await w.flush();
    const p2 = recordings.remove('v2');
    await w.flush();

    // Mutation 1 commits: its post-commit refresh snapshots INTERMEDIATE
    // truth (v1 gone, v2 still active) and stays pending.
    w.rows.find((r) => r.id === 'v1')!.trashed = true;
    del1.resolve(undefined);
    await p1;

    // A load fires during mutation 2's window (pre-v2-commit snapshot).
    void recordings.load();
    await w.flush();

    // Mutation 2 commits: the fence must kill mutation 1's pending
    // intermediate-truth refresh AND the during-mutation reads.
    w.rows.find((r) => r.id === 'v2')!.trashed = true;
    const seqAtCommit = w.nextSeq();
    del2.resolve(undefined);
    await w.deliverSince(seqAtCommit);
    await p2;

    // Everything earlier — including mutation 1's intermediate truth —
    // delivers LAST and must not survive.
    w.deliverRemaining();
    await w.flush();
    await w.flush();

    expect(recordings.list.map((r) => r.id)).toEqual(['e']);
    expect(recordings.availabilityCounts).toEqual(
      new Map([[E, 1]]),
    );
    expect(recordings.activeTotal).toBe(1);
    expect(recordings.loading).toBe(false);
    expect(recordings.availabilityLoading).toBe(false);
  });
});

describe('RecordingsStore — refreshActiveTotal fencing (S-2)', () => {
  beforeEach(resetAllMocks);

  it('an out-of-order count cannot overwrite a newer count', async () => {
    const slow = deferred<number>();
    mockCountRecordings
      .mockImplementationOnce(() => slow.promise)
      .mockResolvedValueOnce(7);
    const { recordings } = await freshStore();

    const pSlow = recordings.refreshActiveTotal();
    await recordings.refreshActiveTotal();
    expect(recordings.activeTotal).toBe(7);

    slow.resolve(3); // the older request lands LAST
    await pSlow;
    expect(recordings.activeTotal).toBe(7);
  });
});

describe('RecordingsStore — edit protection on remote updates', () => {
  it('a protected recording is not re-selected by handleRemoteUpdate', async () => {
    vi.resetModules();
    const mockGetRecording = vi.fn(async () => ({ id: 'rec-1' }));
    vi.doMock('../api/recordings', () => ({
      queryRecordings: () => Promise.resolve([]),
      listRecordingCreationTimes: () => Promise.resolve([]),
      countRecordings: () => Promise.resolve(0),
      listTrashedRecordings: () => Promise.resolve({ items: [], total: 0 }),
      getRecording: mockGetRecording,
    }));
    const { recordings } = await import('./recordings.svelte');
    vi.useFakeTimers();

    recordings.selectedRecording = { id: 'rec-1' } as never;
    recordings.protectFromRemoteUpdate('rec-1');
    expect(recordings.isEditProtected('rec-1')).toBe(true);

    recordings.handleRemoteUpdate('rec-1');
    await Promise.resolve(); // let any (wrongly) started selectRecording run
    // A protected recording must not be re-fetched.
    expect(mockGetRecording).not.toHaveBeenCalled();

    recordings.unprotectFromRemoteUpdate('rec-1');
    recordings.handleRemoteUpdate('rec-1');
    await Promise.resolve();
    expect(mockGetRecording).toHaveBeenCalledWith('rec-1');

    vi.useRealTimers();
    vi.doUnmock('../api/recordings');
  });
});
