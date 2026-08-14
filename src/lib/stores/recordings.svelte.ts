import { invoke } from '@tauri-apps/api/core';
import type { SyncSummary } from '../api/contentSync';
import type { Recording, RecordingSummary } from '../types';
import {
  listRecordings,
  getRecording,
  searchRecordings,
  deleteRecording,
  restoreRecording,
  deleteAllRecordings,
} from '../api/recordings';
import { syncContentNow } from '../api/contentSync';

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
  /// True while a content sync round-trip is in flight. UI uses this to show
  /// a syncing indicator and to avoid stacking concurrent syncs.
  syncing = $state(false);
  /// True when a sync was requested while another sync was already running.
  /// The queued request is replayed after the in-flight sync completes so
  /// `content-changed` SSE notifications are never silently dropped.
  syncPending = $state(false);
  /// Timestamp of the most recently completed sync cycle. Null until the first
  /// successful sync / `content-sync-complete` event.
  lastSyncedAt = $state<Date | null>(null);

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
          is_remote: r.metadata?.synced_from != null,
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
      // Re-insert the cached summary ONLY if it matches the id being restored.
      // Without this guard, undoing deletion A after deletion B (which
      // overwrote lastDeleted) would insert B's summary where A should be.
      if (this.lastDeleted && this.lastDeleted.id === id) {
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

  /// Sync with server (manual trigger or `content-changed` event). Sets the
  /// `syncing` flag for the duration, reloads the list afterwards so the UI
  /// reflects any merged changes, and stamps `lastSyncedAt`.
  ///
  /// If called while a sync is already in flight (e.g. an SSE `content-changed`
  /// event arrives mid-sync), the request is queued via `syncPending` and
  /// replayed after the current sync completes. This prevents dropped
  /// notifications when events fire during an in-flight sync.
  async syncNow(): Promise<SyncSummary | null> {
    // Guard against concurrent syncs: stacked `content-changed` events would
    // otherwise fire multiple overlapping round-trips (Bug M4). Queue the
    // request instead of dropping it so the missed event is replayed.
    if (this.syncing) {
      this.syncPending = true;
      return null;
    }
    this.syncing = true;
    let summary: SyncSummary | null = null;
    try {
      summary = await invoke<SyncSummary>('sync_content_now');
      if (!summary?.disabled) {
        await this.load();
        this.lastSyncedAt = new Date();
      }
    } catch (err) {
      // Network failures and backend errors are logged here rather than
      // propagating as unhandled promise rejections (Bug M4).
      console.error('Content sync failed:', err);
    } finally {
      this.syncing = false;
      // If another sync was requested while we were busy, run it now. Fire
      // and forget with a short delay to avoid deep recursion and to let the
      // current finally block complete before re-entering.
      if (this.syncPending) {
        this.syncPending = false;
        setTimeout(() => {
          this.syncNow().catch((err) =>
            console.error('Replay content sync failed:', err),
          );
        }, 100);
      }
    }
    return summary;
  }

  /// Debounce timer for batched `recording-updated` events. A sync pull
  /// loop emits one event per merged recording; without debouncing, 200
  /// recordings would fire 200 `load()` calls in rapid succession.
  private remoteUpdateTimer: ReturnType<typeof setTimeout> | null = null;

  /// Handle a `recording-updated` event for a specific recording. If the
  /// affected recording is currently selected, re-fetch it so the open editor
  /// shows the merged content. The list reload is debounced (500ms) so
  /// batch sync updates only trigger one `load()` call.
  ///
  /// If a generation is in flight (syncing flag), skip re-selecting the
  /// recording — the in-flight generation will refresh the data on completion.
  /// This prevents a sync event from clobbering a freshly regenerated note.
  handleRemoteUpdate(recordingId: string): void {
    if (this.selectedRecording?.id === recordingId && !this.syncing) {
      // If the recording was remotely deleted, selectRecording will fail
      // (the row is now soft-deleted). Clear it so the editor doesn't
      // show a stale, now-deleted recording.
      selectRecording(recordingId).catch(() => {
        this.selectedRecording = null;
      });
    }
    if (this.remoteUpdateTimer) clearTimeout(this.remoteUpdateTimer);
    this.remoteUpdateTimer = setTimeout(() => {
      this.remoteUpdateTimer = null;
      this.load();
    }, 500);
  }
}

export const recordings = new RecordingsStore();

// ── Background sync ────────────────────────────────────────────────────────
// Periodic background sync runs every 5 minutes for the app's entire lifetime,
// NOT tied to the ContentSync settings component (which unmounts when the user
// navigates away from Settings). This ensures recordings get pushed even when
// the settings panel is closed.
const BG_SYNC_INTERVAL_MS = 5 * 60 * 1000; // 5 minutes
let bgSyncTimer: ReturnType<typeof setInterval> | null = null;

export function startBackgroundSync(): void {
  stopBackgroundSync();
  bgSyncTimer = setInterval(async () => {
    try {
      await syncContentNow();
    } catch (err) {
      console.error('Background content sync failed:', err);
    }
  }, BG_SYNC_INTERVAL_MS);
  console.warn('Background content sync started (5 min interval)');
}

export function stopBackgroundSync(): void {
  if (bgSyncTimer) {
    clearInterval(bgSyncTimer);
    bgSyncTimer = null;
    console.warn('Background content sync stopped');
  }
}

export async function selectRecording(id: string): Promise<void> {
  try {
    const recording = await getRecording(id);
    recordings.selectedRecording = recording;
  } catch (err) {
    console.error('Failed to select recording:', err);
    throw err;
  }
}
