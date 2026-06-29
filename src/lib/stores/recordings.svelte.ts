import type { Recording, RecordingSummary } from '../types';
import {
  listRecordings,
  getRecording,
  searchRecordings,
  deleteRecording,
  restoreRecording,
  deleteAllRecordings,
} from '../api/recordings';

/// Page size for the Recordings list. The list loads this many at a time and
/// appends more on "Load more". A full page means there may be more; a short
/// page means we've reached the end.
const PAGE_SIZE = 50;

class RecordingsStore {
  list = $state<RecordingSummary[]>([]);
  loading = $state<boolean>(false);
  loadingMore = $state<boolean>(false);
  searchQuery = $state<string>('');
  selectedRecording = $state<Recording | null>(null);
  /// True when the backend likely has more recordings beyond what's loaded.
  /// Derived from the last fetch returning a full page. Reset by load()/search().
  hasMore = $state<boolean>(false);

  /// Load the first page, replacing the list. Called on mount and after
  /// mutations that change ordering (new recording, generation, etc.).
  async load(limit = PAGE_SIZE, offset = 0): Promise<void> {
    this.loading = true;
    try {
      const items = await listRecordings(limit, offset);
      this.list = items;
      this.hasMore = items.length >= limit;
    } catch (err) {
      console.error('Failed to load recordings:', err);
    } finally {
      this.loading = false;
    }
  }

  /// Fetch the next page and append it to the list. No-op if already loading
  /// more or if the previous fetch indicated no more results.
  async loadMore(): Promise<void> {
    if (this.loadingMore || !this.hasMore) return;
    this.loadingMore = true;
    try {
      const offset = this.list.length;
      const items = await listRecordings(PAGE_SIZE, offset);
      // Dedup by id in case a new recording landed between pages and shifted
      // offsets — keeps the list stable without dropping anything.
      const existing = new Set(this.list.map((r) => r.id));
      const fresh = items.filter((r) => !existing.has(r.id));
      this.list = [...this.list, ...fresh];
      this.hasMore = items.length >= PAGE_SIZE;
    } catch (err) {
      console.error('Failed to load more recordings:', err);
    } finally {
      this.loadingMore = false;
    }
  }

  async search(query: string): Promise<void> {
    this.searchQuery = query;
    this.loading = true;
    try {
      if (query.trim() === '') {
        const items = await listRecordings();
        this.list = items;
        this.hasMore = items.length >= PAGE_SIZE;
      } else {
        const results = await searchRecordings(query);
        // Map full Recording to RecordingSummary shape
        const summaries: RecordingSummary[] = results.map((r) => ({
          id: r.id,
          filename: r.filename,
          patient_name: r.patient_name,
          status: r.status,
          duration_seconds: r.duration_seconds,
          created_at: r.created_at,
          tags: r.tags,
          has_transcript: r.transcript !== null,
          has_soap_note: r.soap_note !== null,
          has_referral: r.referral !== null,
          has_letter: r.letter !== null,
          has_peer_discussion: r.peer_discussion !== null,
        }));
        this.list = summaries;
        // Search has its own (smaller) limit and no pagination — treat the
        // results as the complete set.
        this.hasMore = false;
      }
    } catch (err) {
      console.error('Failed to search recordings:', err);
    } finally {
      this.loading = false;
    }
  }

  /** The most recently deleted summary, for undo. Cleared after restore or on next delete. */
  lastDeleted = $state<RecordingSummary | null>(null);

  async remove(id: string): Promise<void> {
    try {
      // Capture the item before removing so the Undo toast can restore it.
      this.lastDeleted = this.list.find((r) => r.id === id) ?? null;
      await deleteRecording(id);
      this.list = this.list.filter((r) => r.id !== id);
      if (this.selectedRecording?.id === id) {
        this.selectedRecording = null;
      }
    } catch (err) {
      console.error('Failed to delete recording:', err);
      this.lastDeleted = null;
      throw err;
    }
  }

  async restore(id: string): Promise<void> {
    try {
      await restoreRecording(id);
      // Re-insert the cached summary at its original position (front, since
      // recordings are sorted newest-first and it was the most recent action).
      if (this.lastDeleted) {
        this.list = [this.lastDeleted, ...this.list];
        this.lastDeleted = null;
      }
      // Reload to ensure consistent ordering + server-truth.
      await this.load();
    } catch (err) {
      console.error('Failed to restore recording:', err);
      throw err;
    }
  }

  async removeAll(): Promise<number> {
    try {
      const count = await deleteAllRecordings();
      this.list = [];
      this.hasMore = false;
      this.selectedRecording = null;
      return count;
    } catch (err) {
      console.error('Failed to delete all recordings:', err);
      throw err;
    }
  }
}

export const recordings = new RecordingsStore();

export async function selectRecording(id: string): Promise<void> {
  try {
    const recording = await getRecording(id);
    recordings.selectedRecording = recording;
  } catch (err) {
    console.error('Failed to select recording:', err);
    throw err;
  }
}
