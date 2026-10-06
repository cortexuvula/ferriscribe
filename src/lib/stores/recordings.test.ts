// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { RecordingSummary } from '../types';

// Mock the API layer so we control listRecordings output + pagination offsets.
const mockListRecordings = vi.fn();
const mockSearchRecordings = vi.fn();
const mockCountRecordings = vi.fn();
const mockDeleteAllRecordings = vi.fn();
vi.mock('../api/recordings', () => ({
  listRecordings: (...args: unknown[]) => mockListRecordings(...args),
  searchRecordings: (...args: unknown[]) => mockSearchRecordings(...args),
  countRecordings: (...args: unknown[]) => mockCountRecordings(...args),
  deleteAllRecordings: (...args: unknown[]) => mockDeleteAllRecordings(...args),
  getRecording: vi.fn(),
  deleteRecording: vi.fn(),
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
    mockCountRecordings.mockResolvedValue(0);
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
