import { invoke } from '@tauri-apps/api/core';
import type { SyncSummary } from '../api/contentSync';
import type { Recording, RecordingSummary, TrashedRecordingSummary } from '../types';
import {
  listRecordings,
  getRecording,
  searchRecordings,
  deleteRecording,
  restoreRecording,
  restoreRecordings,
  deleteAllRecordings,
  countRecordings,
  listTrashedRecordings,
  restoreAllTrashed,
  restoreRecordingsDeletedBetween,
  countRecordingsDeletedBetween,
  type DeleteAllResult,
} from '../api/recordings';
import { syncContentNow } from '../api/contentSync';
import { latestTokensPerSecond } from '../utils/generationStats';

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
  /// Error message from the last failed sync, or null. `syncNow()` swallows
  /// errors (callers like the SSE listener don't await safely), so this is
  /// how UI callers distinguish a failed sync from a queued one (both return
  /// null). Cleared at the start of each sync attempt.
  lastSyncError = $state<string | null>(null);
  /// Authoritative count of active recordings from the backend. `list` is a
  /// paginated/search-filtered subset — dialogs that promise "all N
  /// recordings" (Move-all-to-Trash) must use this number, never
  /// `list.length`. Null until the first successful count.
  activeTotal = $state<number | null>(null);

  // ── Trash state (D4/D8) ────────────────────────────────────────────────
  /// Trashed recordings, newest deletion first (paginated like `list`).
  trashedList = $state<TrashedRecordingSummary[]>([]);
  /// Authoritative total of recordings in Trash (the Trash(N) badge and
  /// "M in Trash" copy) — independent of pagination.
  trashedTotal = $state<number>(0);
  trashedLoading = $state<boolean>(false);
  trashedLoadingMore = $state<boolean>(false);
  trashedHasMore = $state<boolean>(false);
  /// Error message from the last failed trash load, or null. The Trash
  /// panel renders this as a retryable error state — never as an empty
  /// trash (a load failure must not look like "nothing to restore").
  trashedError = $state<string | null>(null);
  /// Monotonic request id guarding trash-list writes (same discipline as
  /// `listRequestId`: a stale trash load must never clobber a fresh one).
  trashedRequestId = 0;

  /// Fetch the authoritative active count (COUNT query — cheap). Swallows
  /// errors: a failed count leaves the previous value in place rather than
  /// blanking dialogs that depend on it.
  async refreshActiveTotal(): Promise<void> {
    try {
      this.activeTotal = await countRecordings();
    } catch (err) {
      console.error('Failed to count recordings:', err);
    }
  }

  /// Load the first page, replacing the list. Called on mount and after
  /// mutations that change ordering (new recording, generation, etc).
  async load(limit = PAGE_SIZE, offset = 0): Promise<void> {
    // Keep the authoritative count fresh alongside every list refresh.
    void this.refreshActiveTotal();
    // An active search must survive background refreshes (post-sync load(),
    // content-changed debounces): an unfiltered page here would silently
    // un-filter the list while the search box still shows the query.
    if (this.searchQuery.trim() !== '') {
      await this.search(this.searchQuery);
      return;
    }
    // Request token: overlapping load/search calls must not resolve out of
    // order — a stale response landing last would clobber the fresh list.
    const token = ++this.listRequestId;
    this.loading = true;
    try {
      const items = await listRecordings(limit, offset);
      if (token !== this.listRequestId) return;
      this.list = items;
      this.hasMore = items.length >= limit;
    } catch (err) {
      console.error('Failed to load recordings:', err);
    } finally {
      if (token === this.listRequestId) this.loading = false;
    }
  }

  /// Fetch the next page and append it to the list. No-op if already loading
  /// more or if the previous fetch indicated no more results.
  async loadMore(): Promise<void> {
    if (this.loadingMore || !this.hasMore) return;
    this.loadingMore = true;
    // Snapshot the request id: if a load()/search() replaces the list while
    // this page is in flight, the stale page must NOT be appended onto the
    // fresh results (unfiltered rows under an active query, abandoned
    // pagination state).
    const tokenAtStart = this.listRequestId;
    try {
      const offset = this.list.length;
      const items = await listRecordings(PAGE_SIZE, offset);
      if (tokenAtStart !== this.listRequestId) return;
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
    // Same request-token discipline as load(): the SearchBar debounce means
    // overlapping queries resolve in arbitrary order — only the latest
    // issued request may write the list.
    const token = ++this.listRequestId;
    this.loading = true;
    try {
      if (query.trim() === '') {
        const items = await listRecordings();
        if (token !== this.listRequestId) return;
        this.list = items;
        this.hasMore = items.length >= PAGE_SIZE;
      } else {
        const results = await searchRecordings(query);
        if (token !== this.listRequestId) return;
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
          tokens_per_second: latestTokensPerSecond(r.metadata),
        }));
        this.list = summaries;
        // Search has its own (smaller) limit and no pagination — treat the
        // results as the complete set.
        this.hasMore = false;
      }
    } catch (err) {
      console.error('Failed to search recordings:', err);
    } finally {
      if (token === this.listRequestId) this.loading = false;
    }
  }

  /// Monotonic request id guarding list writes (see load/search).
  listRequestId = 0;
  /// Monotonic request id guarding selected-recording writes: two rapid
  /// `selectRecording` calls resolve last-write-wins without it, so a slow
  /// fetch for recording A can land after the user already switched to B
  /// and clobber the view (same discipline as load/search).
  selectRequestId = 0;

  /** Fetch one recording and make it the selected one. Stale calls (a
   * newer selection superseded this one mid-flight) are discarded. */
  async select(id: string): Promise<void> {
    const token = ++this.selectRequestId;
    const recording = await getRecording(id);
    if (token !== this.selectRequestId) return;
    this.selectedRecording = recording;
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
      void this.refreshActiveTotal();
      this.refreshTrashedAfterMutation();
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
      // Reload BOTH lists + totals for consistent ordering + server-truth:
      // the row just left Trash, so a loaded trash page must drop it and
      // the badge must decrement.
      await this.refreshAfterMutation();
    } catch (err) {
      console.error('Failed to restore recording:', err);
      throw err;
    }
  }

  /** The exact id set of the most recent Move-all-to-Trash, for the batch
   *  Undo toast. Undo restores EXACTLY these ids — a recording deleted
   *  AFTER the move must never be swept into that undo. Cleared after the
   *  undo runs (or the toast is dismissed without acting). */
  lastDeletedAllIds = $state<string[] | null>(null);

  /** Guard against duplicate restore submissions while one is in flight
   *  (double-click on Undo / Restore buttons must be one operation). */
  restoring = $state<boolean>(false);

  async removeAll(): Promise<DeleteAllResult> {
    try {
      const result = await deleteAllRecordings();
      this.list = [];
      this.hasMore = false;
      this.selectedRecording = null;
      this.activeTotal = 0;
      // Everything just moved into Trash — keep its badge AND any loaded
      // trash page honest too.
      this.refreshTrashedAfterMutation();
      // Capture the exact set for the batch Undo toast (D2).
      this.lastDeletedAllIds = result.ids;
      return result;
    } catch (err) {
      console.error('Failed to move all recordings to Trash:', err);
      throw err;
    }
  }

  /** Batch Undo for Move-all-to-Trash: restores EXACTLY the captured id
   *  set — never "everything deleted since T", which would sweep in a
   *  later, unrelated deletion. Duplicate submissions while one is in
   *  flight are rejected (a double-click is one restore, not two racing
   *  transactions). Returns the ACTUAL restored count, which may be lower
   *  if a purge intervened. */
  async undoMoveAll(): Promise<number> {
    const ids = this.lastDeletedAllIds;
    if (!ids || ids.length === 0) {
      throw new Error('No move-all to undo');
    }
    if (this.restoring) {
      throw new Error('A restore is already in progress');
    }
    this.restoring = true;
    try {
      const result = await restoreRecordings(ids);
      this.lastDeletedAllIds = null;
      // Refresh the active list + authoritative count AND the trash
      // list/total — the rows just left Trash.
      await this.refreshAfterMutation();
      return result.count;
    } finally {
      this.restoring = false;
    }
  }

  /// Load the first page of the Trash view, replacing the list. Monotonic
  /// request-id discipline identical to load(): stale responses are
  /// discarded, and a failure sets `trashedError` (rendered as a retryable
  /// error — never as an empty trash).
  async loadTrashed(limit = PAGE_SIZE, offset = 0): Promise<void> {
    const token = ++this.trashedRequestId;
    this.trashedLoading = true;
    this.trashedError = null;
    try {
      const { items, total } = await listTrashedRecordings(limit, offset);
      if (token !== this.trashedRequestId) return;
      this.trashedList = items;
      this.trashedTotal = total;
      this.trashedHasMore = items.length >= limit && items.length < total;
    } catch (err) {
      console.error('Failed to load Trash:', err);
      if (token === this.trashedRequestId) {
        this.trashedError = err instanceof Error ? err.message : String(err);
      }
    } finally {
      if (token === this.trashedRequestId) this.trashedLoading = false;
    }
  }

  /// Fetch the next trash page and append it. No-op while a load is in
  /// flight or when the previous fetch said no more results.
  async loadMoreTrashed(): Promise<void> {
    if (this.trashedLoadingMore || !this.trashedHasMore) return;
    this.trashedLoadingMore = true;
    const tokenAtStart = this.trashedRequestId;
    try {
      const offset = this.trashedList.length;
      const { items, total } = await listTrashedRecordings(PAGE_SIZE, offset);
      if (tokenAtStart !== this.trashedRequestId) return;
      const existing = new Set(this.trashedList.map((r) => r.id));
      const fresh = items.filter((r) => !existing.has(r.id));
      this.trashedList = [...this.trashedList, ...fresh];
      this.trashedTotal = total;
      // hasMore from the RAW page, exactly like loadTrashed — not the
      // deduped slice: a full page that re-delivers already-listed rows
      // (offsets shifted between fetches) still means more rows exist.
      this.trashedHasMore = items.length >= PAGE_SIZE && items.length < total;
    } catch (err) {
      console.error('Failed to load more Trash:', err);
    } finally {
      this.trashedLoadingMore = false;
    }
  }

  /// Refresh ONLY the authoritative trash total (badge updates after
  /// mutations/sync/purge) without disturbing the loaded page.
  async refreshTrashedTotal(): Promise<void> {
    try {
      // limit=0: no rows, just the COUNT — the total is folded into
      // list_trashed_recordings by design (fewer commands).
      const { total } = await listTrashedRecordings(0, 0);
      this.trashedTotal = total;
    } catch (err) {
      console.error('Failed to refresh Trash total:', err);
    }
  }

  /// Shared post-restore refresh: both Active and Trash totals move on
  /// every restore/delete/sync/purge, so both are refreshed together.
  private async refreshAfterMutation(): Promise<void> {
    await Promise.all([this.load(), this.loadTrashed()]);
  }

  /// Keep the Trash view consistent after a mutation that moves rows into
  /// it WITHOUT a full list reload (remove/removeAll patch the active list
  /// surgically). When a trash page is in play — rows listed, or a nonzero
  /// badge saying rows exist — reload page 1 (loadTrashed folds in the
  /// total). Otherwise refresh the badge only, preserving the documented
  /// boot behavior (count fetched, no page loaded; the page loads on every
  /// Trash entry).
  private refreshTrashedAfterMutation(): void {
    if (this.trashedList.length > 0 || this.trashedTotal > 0) {
      void this.loadTrashed();
    } else {
      void this.refreshTrashedTotal();
    }
  }

  /// Restore specific trashed recordings by id (per-row Restore button).
  /// Duplicate submissions while a restore is in flight are rejected.
  /// Returns the actual restored count (0 when the row was purged or
  /// already restored elsewhere).
  async restoreTrashed(ids: string[]): Promise<number> {
    if (ids.length === 0) return 0;
    if (this.restoring) throw new Error('A restore is already in progress');
    this.restoring = true;
    try {
      const result = await restoreRecordings(ids);
      await this.refreshAfterMutation();
      return result.count;
    } finally {
      this.restoring = false;
    }
  }

  /// "Restore all" — every recording in Trash, regardless of what page is
  /// loaded or what search filter is active.
  async restoreAllFromTrash(): Promise<number> {
    if (this.restoring) throw new Error('A restore is already in progress');
    this.restoring = true;
    try {
      const result = await restoreAllTrashed();
      await this.refreshAfterMutation();
      return result.count;
    } finally {
      this.restoring = false;
    }
  }

  /// Preview count for restore-by-date (the dedicated count command —
  /// never derived from the loaded trash pages).
  async countTrashedOnDate(day: Date): Promise<number> {
    const { startIso, endIso } = localDayIntervalUtc(day);
    return countRecordingsDeletedBetween(startIso, endIso);
  }

  /// Restore every recording moved to Trash on the given LOCAL calendar
  /// day (exact day, not "on or after"). Returns the ACTUAL restored
  /// count, which may differ from the preview (purge race).
  async restoreTrashedOnDate(day: Date): Promise<number> {
    if (this.restoring) throw new Error('A restore is already in progress');
    const { startIso, endIso } = localDayIntervalUtc(day);
    this.restoring = true;
    try {
      const result = await restoreRecordingsDeletedBetween(startIso, endIso);
      await this.refreshAfterMutation();
      return result.count;
    } finally {
      this.restoring = false;
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
    this.lastSyncError = null;
    let summary: SyncSummary | null = null;
    try {
      summary = await invoke<SyncSummary>('sync_content_now');
      if (!summary?.disabled) {
        await this.load();
        // Sync merges tombstones and revives from the partner — the Trash
        // badge moves with them.
        void this.refreshTrashedTotal();
        this.lastSyncedAt = new Date();
      }
    } catch (err) {
      // Network failures and backend errors are logged here rather than
      // propagating as unhandled promise rejections (Bug M4). Recorded on
      // lastSyncError so UI callers (Sync Now) can distinguish failure from
      // a queued sync — both return null.
      console.error('Content sync failed:', err);
      this.lastSyncError = err instanceof Error ? err.message : String(err);
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

  /// Recordings with unsaved/in-flight edits in the editor. A remote
  /// update for a protected recording must NOT re-select it — that would
  /// wholesale replace `selectedRecording` and visually revert the user's
  /// edits (the editor's own dirty-checked listener shows the "updated on
  /// another machine" notice instead). The EditorTab maintains membership.
  private editProtected = new Set<string>();

  /** Mark a recording as having unsaved editor state (idempotent). */
  protectFromRemoteUpdate(recordingId: string): void {
    this.editProtected.add(recordingId);
  }

  /** Clear the protection (recording left or edits flushed/saved). */
  unprotectFromRemoteUpdate(recordingId: string): void {
    this.editProtected.delete(recordingId);
  }

  /** Test seam: is this recording currently edit-protected? */
  isEditProtected(recordingId: string): boolean {
    return this.editProtected.has(recordingId);
  }

  /// Handle a `recording-updated` event for a specific recording. If the
  /// affected recording is currently selected, re-fetch it so the open editor
  /// shows the merged content. The list reload is debounced (500ms) so
  /// batch sync updates only trigger one `load()` call.
  ///
  /// Skips the re-select when a generation is in flight (syncing flag —
  /// the in-flight generation refreshes the data on completion) or when
  /// the editor holds unsaved state for the recording (edit protection).
  handleRemoteUpdate(recordingId: string): void {
    const clobberSafe =
      this.selectedRecording?.id === recordingId && !this.syncing && !this.editProtected.has(recordingId);
    if (clobberSafe) {
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
      // Sync-merged tombstones/revivals move the Trash badge with the
      // list — keep it honest on the same debounce (cheap count; the
      // trash page itself reloads on Trash entry).
      void this.refreshTrashedTotal();
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
      const summary = await syncContentNow();
      if (!summary?.disabled) {
        // Sync merges tombstones and revives from the partner — the Trash
        // badge moves with them (mirrors syncNow's post-sync refresh).
        void recordings.refreshTrashedTotal();
      }
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
    await recordings.select(id);
  } catch (err) {
    console.error('Failed to select recording:', err);
    throw err;
  }
}

/// The UTC interval `[startIso, endIso)` covering the LOCAL calendar day
/// of `day` — the exact-day restore window (Andre's ruling: exact day,
/// NOT "on or after"). `new Date(y, m, d)` is local midnight; passing
/// `d + 1` rolls month/year boundaries correctly (and absorbs DST shifts,
/// since both bounds are constructed as calendar midnights, not as
/// 24-hour offsets). The backend compares half-open:
/// `datetime(deleted_at) >= datetime(start) AND < datetime(end)`.
export function localDayIntervalUtc(day: Date): { startIso: string; endIso: string } {
  const start = new Date(day.getFullYear(), day.getMonth(), day.getDate());
  const end = new Date(day.getFullYear(), day.getMonth(), day.getDate() + 1);
  return { startIso: start.toISOString(), endIso: end.toISOString() };
}
