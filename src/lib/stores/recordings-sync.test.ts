// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { RecordingSummary } from '../types';

// ── Mocks ───────────────────────────────────────────────────────────────────
// The recordings store imports invoke directly from @tauri-apps/api/core
// (for syncNow's invoke('sync_content_now')) and also calls API wrappers
// from ../api/recordings. We need to control both.

const mockListRecordings = vi.fn();
const mockGetRecording = vi.fn();
const mockInvoke = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}));

vi.mock('../api/recordings', () => ({
  listRecordings: (...args: unknown[]) => mockListRecordings(...args),
  getRecording: (...args: unknown[]) => mockGetRecording(...args),
  searchRecordings: vi.fn(async () => []),
  deleteRecording: vi.fn(async () => {}),
  restoreRecording: vi.fn(async () => {}),
  deleteAllRecordings: vi.fn(async () => 0),
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

// Re-import the module FRESHLY so each test gets a clean store instance.
async function freshStore() {
  vi.resetModules();
  return await import('./recordings.svelte');
}

beforeEach(() => {
  vi.useFakeTimers();
  mockListRecordings.mockReset();
  mockGetRecording.mockReset();
  mockInvoke.mockReset();
  mockInvoke.mockResolvedValue(undefined);
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
    mockListRecordings.mockResolvedValue([]);
    const { recordings } = await freshStore();

    const promise = recordings.syncNow();
    expect(recordings.syncing).toBe(true);

    resolveSync!();
    await promise;
    await vi.advanceTimersByTimeAsync(0);

    expect(recordings.syncing).toBe(false);
  });

  it('stamps lastSyncedAt after success', async () => {
    mockInvoke.mockResolvedValue(undefined);
    mockListRecordings.mockResolvedValue([]);
    const { recordings } = await freshStore();

    expect(recordings.lastSyncedAt).toBeNull();
    await recordings.syncNow();
    expect(recordings.lastSyncedAt).toBeInstanceOf(Date);
  });

  it('does not stamp lastSyncedAt on failure', async () => {
    mockInvoke.mockRejectedValue(new Error('network down'));
    mockListRecordings.mockResolvedValue([]);
    const { recordings } = await freshStore();

    await recordings.syncNow();
    expect(recordings.lastSyncedAt).toBeNull();
  });

  it('reloads the list after a successful sync', async () => {
    mockInvoke.mockResolvedValue(undefined);
    mockListRecordings.mockResolvedValue([makeSummary('synced-1')]);
    const { recordings } = await freshStore();

    await recordings.syncNow();
    expect(recordings.list).toHaveLength(1);
    expect(recordings.list[0].id).toBe('synced-1');
  });

  it('guards against concurrent syncs (skips if already syncing)', async () => {
    let resolveFirst: () => void;
    mockInvoke.mockReturnValueOnce(
      new Promise<void>((r) => { resolveFirst = r; }),
    );
    mockListRecordings.mockResolvedValue([]);
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
    mockListRecordings.mockResolvedValue([]);
    const { recordings } = await freshStore();
    await recordings.load(); // initial load

    mockListRecordings.mockClear();

    recordings.handleRemoteUpdate('rec-1');
    recordings.handleRemoteUpdate('rec-2');
    recordings.handleRemoteUpdate('rec-3');

    // Not yet — debounced.
    expect(mockListRecordings).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(500);

    // Exactly one reload for 3 batched events.
    expect(mockListRecordings).toHaveBeenCalledTimes(1);
  });

  it('refetches selectedRecording when it matches the updated id', async () => {
    mockListRecordings.mockResolvedValue([]);
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
    mockListRecordings.mockResolvedValue([]);
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
    mockListRecordings.mockResolvedValue([]);
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
