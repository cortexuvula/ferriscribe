// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { RecordingSummary } from '../types';
import { parseLocalDateKey } from '../utils/recordingDates';

// ── Mocks ───────────────────────────────────────────────────────────────────
// The recordings store imports invoke directly from @tauri-apps/api/core
// (for syncNow's invoke('sync_content_now')) and also calls API wrappers
// from ../api/recordings. We need to control both.

const mockQueryRecordings = vi.fn();
const mockListCreationTimes = vi.fn();
const mockGetRecording = vi.fn();
const mockInvoke = vi.fn();
const mockListTrashed = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}));

vi.mock('../api/recordings', () => ({
  queryRecordings: (...args: unknown[]) => mockQueryRecordings(...args),
  listRecordingCreationTimes: (...args: unknown[]) => mockListCreationTimes(...args),
  getRecording: (...args: unknown[]) => mockGetRecording(...args),
  deleteRecording: vi.fn(async () => {}),
  restoreRecording: vi.fn(async () => {}),
  restoreRecordings: vi.fn(async () => ({ count: 0, ids: [] as string[] })),
  deleteAllRecordings: vi.fn(async () => ({ count: 0, ids: [] as string[] })),
  countRecordings: vi.fn(async () => 0),
  listTrashedRecordings: (...args: unknown[]) => mockListTrashed(...args),
  restoreAllTrashed: vi.fn(async () => ({ count: 0, ids: [] as string[] })),
  restoreRecordingsDeletedBetween: vi.fn(async () => ({ count: 0, ids: [] as string[] })),
  countRecordingsDeletedBetween: vi.fn(async () => 0),
}));

vi.mock('../api/contentSync', () => ({
  syncContentNow: () => mockInvoke('sync_content_now'),
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

/// A timestamp at LOCAL NOON of the given day key — keys back to exactly
/// that day in whatever zone the test process runs.
function noonIso(key: string): string {
  return new Date(parseLocalDateKey(key).getTime() + 12 * 3_600_000).toISOString();
}

// Re-import the module FRESHLY so each test gets a clean store instance.
async function freshStore() {
  vi.resetModules();
  return await import('./recordings.svelte');
}

beforeEach(() => {
  vi.useFakeTimers();
  mockQueryRecordings.mockReset();
  mockQueryRecordings.mockResolvedValue([]);
  mockListCreationTimes.mockReset();
  mockListCreationTimes.mockResolvedValue([]);
  mockGetRecording.mockReset();
  mockInvoke.mockReset();
  mockInvoke.mockResolvedValue(undefined);
  mockListTrashed.mockReset();
  mockListTrashed.mockResolvedValue({ items: [], total: 0 });
  vi.clearAllMocks();
});

afterEach(() => {
  vi.useRealTimers();
});

// ── syncNow ─────────────────────────────────────────────────────────────────

describe('RecordingsStore.syncNow', () => {
  it('sets syncing=true during the round-trip, false after', async () => {
    let resolveSync: () => void;
    mockInvoke.mockReturnValue(new Promise<void>((r) => { resolveSync = r; }));
    const { recordings } = await freshStore();

    const promise = recordings.syncNow();
    expect(recordings.syncing).toBe(true);

    resolveSync!();
    await promise;
    await vi.advanceTimersByTimeAsync(0);

    expect(recordings.syncing).toBe(false);
  });

  it('stamps lastSyncedAt after success', async () => {
    const { recordings } = await freshStore();

    expect(recordings.lastSyncedAt).toBeNull();
    await recordings.syncNow();
    expect(recordings.lastSyncedAt).toBeInstanceOf(Date);
  });

  it('does not stamp lastSyncedAt on failure', async () => {
    mockInvoke.mockRejectedValue(new Error('network down'));
    const { recordings } = await freshStore();

    await recordings.syncNow();
    expect(recordings.lastSyncedAt).toBeNull();
  });

  it('reloads the list after a successful sync', async () => {
    mockQueryRecordings.mockResolvedValue([makeSummary('synced-1')]);
    const { recordings } = await freshStore();

    await recordings.syncNow();
    expect(recordings.list).toHaveLength(1);
    expect(recordings.list[0].id).toBe('synced-1');
  });

  it('refreshes date availability after a successful sync', async () => {
    // Sync merges remote rows — the calendar's populated days move with
    // them, so the post-sync reload must refresh availability too.
    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-05')]);
    const { recordings } = await freshStore();

    await recordings.syncNow();
    expect(mockListCreationTimes).toHaveBeenCalled();
    expect(recordings.availabilityCounts.get('2026-10-05')).toBe(1);
  });

  it('discards an availability response issued before the sync round', async () => {
    vi.useRealTimers(); // this test has no debounce/timer dependence
    const day = '2026-10-05';
    mockListCreationTimes.mockResolvedValue([noonIso(day)]);
    mockInvoke.mockResolvedValue({ disabled: false });
    const { recordings } = await freshStore();
    await recordings.setSelectedDate(day);
    expect(recordings.selectedDate).toBe(day);

    // R1: issued BEFORE the sync, resolves LATE, with the day populated
    // (pre-merge data).
    let resolveR1: (v: string[]) => void = () => {};
    mockListCreationTimes.mockImplementationOnce(
      () => new Promise((res) => { resolveR1 = res; }),
    );
    const pR1 = recordings.refreshAvailability();

    // The sync round purged the day's recordings; the post-sync fetch
    // (current data) legitimately empties it.
    mockListCreationTimes.mockResolvedValueOnce([]);
    await recordings.syncNow();
    await vi.waitFor(() => expect(recordings.selectedDate).toBeNull());

    resolveR1([noonIso(day)]); // stale pre-merge reply: day "still there"
    await pR1;
    // The stale reply neither repopulated the calendar…
    expect(recordings.availabilityCounts.size).toBe(0);
    // …nor resurrected the cleared date filter.
    expect(recordings.selectedDate).toBeNull();
  });

  it('guards against concurrent syncs (skips if already syncing)', async () => {
    let resolveFirst: () => void;
    mockInvoke.mockReturnValueOnce(
      new Promise<void>((r) => { resolveFirst = r; }),
    );
    const { recordings } = await freshStore();

    const first = recordings.syncNow();
    // Fire a second syncNow while the first is still in flight.
    const second = recordings.syncNow();

    await second;
    expect(mockInvoke).toHaveBeenCalledTimes(1);

    resolveFirst!();
    await first;
  });
});

// ── handleRemoteUpdate ──────────────────────────────────────────────────────

describe('RecordingsStore.handleRemoteUpdate', () => {
  it('reloads the list after the 500ms debounce', async () => {
    const { recordings } = await freshStore();
    await recordings.load(); // initial load

    mockQueryRecordings.mockClear();

    recordings.handleRemoteUpdate('rec-1');
    recordings.handleRemoteUpdate('rec-2');
    recordings.handleRemoteUpdate('rec-3');

    // Not yet — debounced.
    expect(mockQueryRecordings).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(500);

    // Exactly one reload for 3 batched events.
    expect(mockQueryRecordings).toHaveBeenCalledTimes(1);
  });

  it('refreshes the Trash badge and availability alongside the debounced reload (sync tombstones)', async () => {
    mockListTrashed.mockResolvedValue({ items: [], total: 3 });
    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-05')]);
    const { recordings } = await freshStore();
    await recordings.load();
    mockQueryRecordings.mockClear();
    mockListTrashed.mockClear();
    mockListCreationTimes.mockClear();

    recordings.handleRemoteUpdate('rec-1');
    await vi.advanceTimersByTimeAsync(500);

    // The debounced callback refreshed the trash COUNT and the date
    // availability too — a sync-pulled tombstone must move the Trash(N)
    // badge and empty the day on the calendar without entering the view.
    expect(mockQueryRecordings).toHaveBeenCalledTimes(1);
    expect(mockListTrashed).toHaveBeenCalledWith(0, 0);
    expect(recordings.trashedTotal).toBe(3);
    expect(mockListCreationTimes).toHaveBeenCalledTimes(1);
    expect(recordings.availabilityCounts.get('2026-10-05')).toBe(1);
  });

  it('refetches selectedRecording when it matches the updated id', async () => {
    mockGetRecording.mockResolvedValue({ id: 'rec-1', transcript: 'updated' });
    const { recordings, selectRecording } = await freshStore();
    await recordings.load();

    await selectRecording('rec-1');
    expect(recordings.selectedRecording).not.toBeNull();

    mockGetRecording.mockClear();

    recordings.handleRemoteUpdate('rec-1');
    await vi.advanceTimersByTimeAsync(0); // microtasks

    expect(mockGetRecording).toHaveBeenCalledWith('rec-1');
  });

  it('clears selectedRecording when the remote delete fails the refetch (M5 fix)', async () => {
    mockGetRecording.mockResolvedValue({ id: 'rec-1' });
    const { recordings, selectRecording } = await freshStore();
    await recordings.load();

    await selectRecording('rec-1');
    expect(recordings.selectedRecording?.id).toBe('rec-1');

    // Simulate remote delete: selectRecording throws because the row is gone.
    mockGetRecording.mockRejectedValue(new Error('not found'));

    recordings.handleRemoteUpdate('rec-1');
    await vi.advanceTimersByTimeAsync(0); // let the catch fire

    expect(recordings.selectedRecording).toBeNull();
  });

  it('does not refetch when the updated id is not the selected recording', async () => {
    mockGetRecording.mockResolvedValue({ id: 'rec-1' });
    const { recordings, selectRecording } = await freshStore();
    await recordings.load();

    await selectRecording('rec-1');
    mockGetRecording.mockClear();

    recordings.handleRemoteUpdate('different-rec');
    await vi.advanceTimersByTimeAsync(0);

    expect(mockGetRecording).not.toHaveBeenCalled();
  });
});

// ── startBackgroundSync ─────────────────────────────────────────────────────

describe('RecordingsStore.startBackgroundSync', () => {
  it('refreshes the Trash badge and availability after each background sync round', async () => {
    mockListTrashed.mockResolvedValue({ items: [], total: 4 });
    mockListCreationTimes.mockResolvedValue([noonIso('2026-10-05')]);
    mockInvoke.mockResolvedValue({
      pulled: 1,
      pushed: 0,
      merge_conflicts: 0,
      push_conflicts: 0,
      disabled: false,
    });
    const { recordings, startBackgroundSync, stopBackgroundSync } = await freshStore();

    startBackgroundSync();
    await vi.advanceTimersByTimeAsync(5 * 60 * 1000);
    stopBackgroundSync();

    expect(mockInvoke).toHaveBeenCalledWith('sync_content_now');
    // Sync merges tombstones/revivals — the badge count AND the calendar
    // availability ran afterwards…
    expect(mockListTrashed).toHaveBeenCalledWith(0, 0);
    expect(recordings.trashedTotal).toBe(4);
    expect(mockListCreationTimes).toHaveBeenCalledTimes(1);
    expect(recordings.availabilityCounts.get('2026-10-05')).toBe(1);
    // …but the visible list was NOT reloaded — a 5-minute timer must not
    // yank the user's scroll position (list changes ride the next natural
    // load or the day-clear path).
    expect(mockQueryRecordings).not.toHaveBeenCalled();
  });

  it('skips the Trash/availability refresh when content sync is disabled', async () => {
    mockInvoke.mockResolvedValue({
      pulled: 0,
      pushed: 0,
      merge_conflicts: 0,
      push_conflicts: 0,
      disabled: true,
    });
    const { startBackgroundSync, stopBackgroundSync } = await freshStore();

    startBackgroundSync();
    await vi.advanceTimersByTimeAsync(5 * 60 * 1000);
    stopBackgroundSync();

    expect(mockInvoke).toHaveBeenCalledWith('sync_content_now');
    expect(mockListTrashed).not.toHaveBeenCalled();
    expect(mockListCreationTimes).not.toHaveBeenCalled();
  });
});
