import { invoke } from '@tauri-apps/api/core';
import type { SyncSummary } from '../api/contentSync';
import type { Recording, RecordingSummary, TrashedRecordingSummary } from '../types';
import {
  queryRecordings,
  listRecordingCreationTimes,
  getRecording,
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
  type QueryRecordingsOptions,
} from '../api/recordings';
import { syncContentNow } from '../api/contentSync';
import {
  dayIntervalUtcForKey,
  groupCreationTimesByDay,
  fullLocalDateFromKey,
} from '../utils/recordingDates';
import { sanitizedErr } from '../utils/sanitizedErr';

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
  /// Selected local calendar day ('YYYY-MM-DD') filtering the Active list,
  /// null = all dates. Survives every load/refresh; only an explicit
  /// user clear/change — or an availability-proven "that day's last active
  /// recording disappeared" — may change it (never a failed fetch).
  selectedDate = $state<string | null>(null);
  /// Polite static notice set when availability proved the selected day
  /// emptied and the date filter was auto-cleared ("No active recordings
  /// remain on <locally formatted date>. Showing all dates."). Cleared when
  /// the user picks a date manually.
  dateClearedNotice = $state<string | null>(null);
  /// Sanitized first-page load error (retryable), or null. A failure is an
  /// explicit error state — never a fake empty success, and never stale
  /// rows from a DIFFERENT filter.
  listError = $state<string | null>(null);
  /// Sanitized load-more error (retryable), or null. A failed page keeps
  /// the already-valid rows visible.
  loadMoreError = $state<string | null>(null);
  /// True when the backend likely has more recordings beyond what's loaded.
  /// Derived from the last fetch returning a full RAW page (before dedup).
  /// Reset by filter changes and first-page loads.
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
  /// paginated/filtered subset — dialogs that promise "all N recordings"
  /// (Move-all-to-Trash) must use this number, never `list.length`. Null
  /// until the first successful count.
  activeTotal = $state<number | null>(null);

  // ── Availability (date-calendar data) ─────────────────────────────────
  /// Active-recording counts per LOCAL calendar day key ('YYYY-MM-DD'),
  /// derived from the backend's timestamp-only availability projection.
  /// Independent of search/date filters — it always describes the whole
  /// active DB (the calendar's populated days are global).
  availabilityCounts = $state<Map<string, number>>(new Map());
  availabilityLoading = $state(false);
  /// Sanitized availability fetch error, or null. A failed fetch keeps the
  /// previous counts visible — failure never looks like an empty calendar
  /// and never clears the date filter.
  availabilityError = $state<string | null>(null);
  /// True once availability has loaded successfully at least once.
  availabilityLoaded = $state(false);
  /// Monotonic request id guarding availability writes (reordering guard:
  /// a slow earlier response must never overwrite a later one).
  availabilityRequestId = 0;
  /// Data-generation captured by the availability request currently in
  /// flight (null when idle) — lets a same-generation refresh reuse an
  /// in-flight fetch instead of stacking duplicates.
  availabilityFetchGeneration: number | null = null;
  /// Monotonic mutation counter, bumped BEFORE every destructive mutation,
  /// AGAIN at each mutation's COMMIT (see fenceMutationCompletion), and
  /// whenever remote data changes (sync rounds, remote updates). A bump
  /// fences reads issued before it — their responses land with a stale
  /// generation and are discarded whole. The pre-mutation bump alone does
  /// NOT fence reads issued DURING the mutation window: they snapshot
  /// pre-commit state while holding the post-bump generation (SQLite WAL
  /// serves concurrent readers the old snapshot), so the completion bump
  /// closes exactly that hole.
  dataGeneration = 0;

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

  /// Monotonic request id guarding the authoritative-count write (same
  /// discipline as the list token): an older count landing after a newer
  /// one must never overwrite it — activeTotal feeds the destructive
  /// Move-all-to-Trash confirmation.
  countRequestId = 0;

  /// Fetch the authoritative active count (COUNT query — cheap). Guarded
  /// by a request token (reordering) and the data generation (mutation
  /// boundary, including the post-commit fence): a count issued during a
  /// mutation window snapshots PRE-commit state and is discarded on
  /// landing. Swallows errors: a failed count leaves the previous value
  /// in place rather than blanking dialogs that depend on it.
  async refreshActiveTotal(): Promise<void> {
    const token = ++this.countRequestId;
    const generation = this.dataGeneration;
    try {
      const n = await countRecordings();
      if (token !== this.countRequestId || generation !== this.dataGeneration) {
        return; // superseded by a newer count or a data mutation commit
      }
      this.activeTotal = n;
    } catch (err) {
      console.error('Failed to count recordings:', err);
    }
  }

  /// The composite query options for the CURRENT {date, search} filter.
  /// Only defined keys are set — the backend treats absent search/bounds
  /// as "no filter" and rejects half-supplied bound pairs, so both bounds
  /// always travel together or not at all.
  private queryOptions(limit: number, offset: number): QueryRecordingsOptions {
    const opts: QueryRecordingsOptions = { limit, offset };
    const search = this.searchQuery.trim();
    if (search !== '') opts.search = search;
    if (this.selectedDate !== null) {
      // Half-open [start, end) local-midnight bounds — the same semantics
      // as the Trash restore-by-date window (localDayIntervalUtc).
      const { startIso, endIso } = dayIntervalUtcForKey(this.selectedDate);
      opts.startIso = startIso;
      opts.endIso = endIso;
    }
    return opts;
  }

  /// Identity of the CURRENT {date, search} filter. The visible list is
  /// tagged with the filter it was loaded under so a failed load for a NEW
  /// filter can never leave the OLD filter's rows on screen.
  private currentFilterKey(): string {
    return JSON.stringify([this.selectedDate, this.searchQuery.trim()]);
  }

  /// Load the first page under the CURRENT filters, replacing the list.
  /// Called on mount, after mutations that reorder, on filter changes,
  /// and by every background refresh path (sync, remote updates). The
  /// selected day and search always survive — the query is rebuilt from
  /// them, never from a stale snapshot.
  ///
  /// Alongside the page, refreshes the authoritative count and the date
  /// availability (both fire-and-forget; neither failure affects the
  /// list outcome).
  async load(limit = PAGE_SIZE, offset = 0): Promise<void> {
    void this.refreshActiveTotal();
    void this.refreshAvailability();
    if (this.currentFilterKey() !== this.listFilterKey) {
      // Rows from a previous filter must never show under the new one —
      // clear now so a FAILED load leaves an explicit retryable error,
      // not stale wrong-filter rows (nor a fake empty "success").
      this.list = [];
      this.hasMore = false;
      this.listFilterKey = this.currentFilterKey();
    }
    // Request token: overlapping load/search calls must not resolve out of
    // order — a stale response landing last would clobber the fresh list.
    const token = ++this.listRequestId;
    // A new first page supersedes any in-flight load-more: its page belongs
    // to the previous request generation, and its finally block must not
    // touch this generation's busy flags (cleared here, skipped there).
    this.loadingMore = false;
    this.loading = true;
    this.listError = null;
    this.loadMoreError = null;
    try {
      const items = await queryRecordings(this.queryOptions(limit, offset));
      if (token !== this.listRequestId) return;
      this.list = items;
      // RAW consumed offset — deliberately independent of the deduped
      // visible list: the backend positions pages in raw rows, and mixing
      // the two re-delivers or strands rows when duplicates appear.
      this.nextRawOffset = offset + items.length;
      this.hasMore = items.length >= limit;
    } catch (err) {
      console.error('Failed to load recordings:', err);
      if (token === this.listRequestId) {
        // Keep the same-filter rows in store state, but do NOT claim they
        // are current: the UI intentionally hides them behind the
        // retryable listError state until a reload succeeds. Retaining
        // them preserves state for that recovery instead of manufacturing
        // a fake empty success — it is not a freshness guarantee.
        this.listError = sanitizedErr(err);
      }
    } finally {
      if (token === this.listRequestId) this.loading = false;
    }
  }

  /// Fetch the next page and append it to the list. No-op while a
  /// first-page load is in flight (paging into an unknown list), while
  /// another load-more runs, or when the previous fetch said no more.
  async loadMore(): Promise<void> {
    if (this.loading || this.loadingMore || !this.hasMore) return;
    // Snapshot both the request generation and the raw offset: if a
    // load()/search() replaces the list while this page is in flight,
    // the stale page must NOT be appended onto the fresh results — and
    // an old offset must never be reused against a new filter.
    const tokenAtStart = this.listRequestId;
    const offsetAtStart = this.nextRawOffset;
    this.loadingMore = true;
    this.loadMoreError = null;
    try {
      const items = await queryRecordings(this.queryOptions(PAGE_SIZE, offsetAtStart));
      if (tokenAtStart !== this.listRequestId) return;
      // Dedup by id in case a new recording landed between pages and shifted
      // offsets — keeps the list stable without dropping anything. The RAW
      // accounting (offset advance, hasMore) ignores the dedup on purpose:
      // a full raw page that re-delivers listed rows still means more rows
      // exist beyond it.
      const existing = new Set(this.list.map((r) => r.id));
      const fresh = items.filter((r) => !existing.has(r.id));
      this.list = [...this.list, ...fresh];
      this.nextRawOffset = offsetAtStart + items.length;
      this.hasMore = items.length >= PAGE_SIZE;
    } catch (err) {
      console.error('Failed to load more recordings:', err);
      // Keep the already-valid rows; the error state offers retry.
      if (tokenAtStart === this.listRequestId) {
        this.loadMoreError = sanitizedErr(err);
      }
    } finally {
      if (tokenAtStart === this.listRequestId) this.loadingMore = false;
    }
  }

  /// Set/change the Active list's date filter (strict local 'YYYY-MM-DD'
  /// key, null = all dates) and reload the first page. The search filter
  /// is preserved; changing either filter resets paging and invalidates
  /// any in-flight first-page/load-more responses (they belong to the old
  /// filter). Selecting the current date again is a no-op.
  async setSelectedDate(key: string | null): Promise<void> {
    if (this.selectedDate === key) return;
    this.selectedDate = key;
    // A manual pick supersedes any earlier auto-clear notice.
    this.dateClearedNotice = null;
    await this.load();
  }

  /** Dismiss the auto-clear notice (its polite announcement already fired
   *  once on landing; the status region never repeats it). */
  dismissDateClearedNotice(): void {
    this.dateClearedNotice = null;
  }

  /// Set the Active list's search filter and reload the first page. The
  /// selected date is preserved; paging resets exactly like a date change.
  async search(query: string): Promise<void> {
    this.searchQuery = query;
    await this.load();
  }

  /// Refresh the date availability: every active recording's creation
  /// instant (timestamp-only payload, whole DB — independent of the
  /// search/date filters) grouped into local-day counts.
  ///
  /// Guarded by BOTH a request token (reordering) and the data generation
  /// (mutation boundary): a response landing after either moved is
  /// discarded whole, so stale data can neither clear the selected date
  /// nor repopulate days a mutation emptied. Coalescing reuses an
  /// in-flight fetch ONLY while its captured generation is still the
  /// CURRENT one — a fetch issued during a mutation window carries the
  /// pre-commit generation, and the mutation's completion fence
  /// (fenceMutationCompletion) bumps past it, forcing a genuine post-
  /// commit fetch. A FAILED fetch keeps the previous counts and NEVER
  /// touches the date filter.
  async refreshAvailability(): Promise<void> {
    // An in-flight fetch for the SAME data generation is still current —
    // reuse it instead of stacking duplicates (filter refreshes fan in).
    if (
      this.availabilityLoading &&
      this.availabilityFetchGeneration === this.dataGeneration
    ) {
      return;
    }
    const token = ++this.availabilityRequestId;
    const generation = this.dataGeneration;
    this.availabilityFetchGeneration = generation;
    this.availabilityLoading = true;
    try {
      const times = await listRecordingCreationTimes();
      if (token !== this.availabilityRequestId || generation !== this.dataGeneration) {
        return; // superseded by a newer request or a data mutation
      }
      const counts = groupCreationTimesByDay(times);
      this.availabilityCounts = counts;
      this.availabilityError = null;
      this.availabilityLoaded = true;
      this.maybeClearEmptiedSelectedDay(counts);
    } catch (err) {
      console.error('Failed to load recording availability:', err);
      if (token === this.availabilityRequestId && generation === this.dataGeneration) {
        this.availabilityError = sanitizedErr(err);
      }
    } finally {
      if (token === this.availabilityRequestId) {
        this.availabilityLoading = false;
        this.availabilityFetchGeneration = null;
      }
    }
  }

  /// When CURRENT (token- and generation-valid) availability proves the
  /// selected day has no active recordings left, clear ONLY the date —
  /// keep the search, keep the editor selection — announce politely, and
  /// re-run the query under the widened filter. No recursion: the reload
  /// re-refreshes availability, whose response now sees no selected date
  /// and takes this branch zero further times.
  private maybeClearEmptiedSelectedDay(counts: Map<string, number>): void {
    if (this.selectedDate === null) return;
    if (counts.has(this.selectedDate)) return;
    const gone = this.selectedDate;
    this.selectedDate = null;
    this.dateClearedNotice =
      `No active recordings remain on ${fullLocalDateFromKey(gone)}. Showing all dates.`;
    void this.load();
  }

  /// Monotonic request id guarding list writes (see load/loadMore).
  listRequestId = 0;
  /// Monotonic request id guarding selected-recording writes: two rapid
  /// `selectRecording` calls resolve last-write-wins without it, so a slow
  /// fetch for recording A can land after the user already switched to B
  /// and clobber the view (same discipline as load/search).
  selectRequestId = 0;
  /// RAW offset consumed by the backend so far (pages advance in raw rows,
  /// independent of the deduped visible list — see load/loadMore).
  nextRawOffset = 0;
  /// The filter ({date, search}) the visible list was loaded under.
  private listFilterKey: string | null = null;

  /** Fetch one recording and make it the selected one. Stale calls (a
   * newer selection superseded this one mid-flight) are discarded. */
  async select(id: string): Promise<void> {
    const token = ++this.selectRequestId;
    const recording = await getRecording(id);
    if (token !== this.selectRequestId) return;
    this.selectedRecording = recording;
  }

  /// Invalidate every in-flight Active query. Called twice per mutation:
  /// BEFORE the invoke (a pre-delete/pre-restore page can never land and
  /// resurrect old list state) and AGAIN at COMMIT
  /// (fenceMutationCompletion). Each call bumps the list token (stale
  /// pages never land), bumps the data generation (pre-bump availability
  /// dies), and clears the busy flags here — the stale requests' finally
  /// blocks skip them by token, so nothing but this path may reset them
  /// mid-flight.
  private invalidateActiveQueries(): void {
    this.listRequestId++;
    this.dataGeneration++;
    this.loading = false;
    this.loadingMore = false;
  }

  /// COMPLETION fence (RC-1): re-invalidate AFTER a mutation's await
  /// resolves, BEFORE surgery/post-commit refreshes. A read issued DURING
  /// the mutation window (search debounce, Active-tab return, calendar
  /// pick) snapshots PRE-commit state while holding the post-bump token
  /// and the still-current generation — both pre-mutation fences pass for
  /// it. Re-bumping at commit kills those reads, and the generation bump
  /// breaks refreshAvailability's coalescing so the post-commit refresh
  /// genuinely fetches current state instead of adopting the pre-commit
  /// in-flight response. Returns true when a list query was in flight
  /// (and got killed) — callers that patch the list surgically reload it
  /// so the visible list converges under whatever filter is current now.
  private fenceMutationCompletion(): boolean {
    const killedListQuery = this.loading || this.loadingMore;
    this.invalidateActiveQueries();
    return killedListQuery;
  }

  /** The most recently deleted summary, for undo. Cleared after restore or on next delete. */
  lastDeleted = $state<RecordingSummary | null>(null);

  async remove(id: string): Promise<void> {
    try {
      // Capture the item before removing so the Undo toast can restore it.
      const visible = this.list.find((r) => r.id === id);
      this.lastDeleted = visible ?? null;
      this.invalidateActiveQueries();
      await deleteRecording(id);
      // COMMIT fence (RC-1): kill any read that started during the
      // mutation window before the surgery/refreshes below run.
      const killedListQuery = this.fenceMutationCompletion();
      this.list = this.list.filter((r) => r.id !== id);
      // The removed row occupied a consumed raw position — shift the next
      // raw offset with it or the row that slides into its place is
      // skipped by the next page (a permanently lost row).
      if (visible) this.nextRawOffset = Math.max(0, this.nextRawOffset - 1);
      if (this.selectedRecording?.id === id) {
        this.selectedRecording = null;
      }
      // A load that started mid-mutation was killed by the fence — the
      // surgery above patched the OLD filter's list, so if that load was
      // replacing the list (e.g. a filter change) reload under the
      // CURRENT filter rather than leaving its cleared/old rows behind.
      if (killedListQuery) void this.load();
      // Fire-and-forget refreshes: a failed follow-up read must not turn
      // this successful mutation into a reported failure (the errors land
      // in their own states instead).
      void this.refreshActiveTotal();
      void this.refreshAvailability();
      this.refreshTrashedAfterMutation();
    } catch (err) {
      console.error('Failed to delete recording:', err);
      this.lastDeleted = null;
      throw err;
    }
  }

  async restore(id: string): Promise<void> {
    try {
      this.invalidateActiveQueries();
      await restoreRecording(id);
      // COMMIT fence (RC-1) — before the refreshes below.
      this.fenceMutationCompletion();
      // The reloaded first page (current filters) brings the restored row
      // back when it MATCHES them. The cached undo summary is never
      // injected into the list — a restored row outside the current
      // date/search filters must stay out until the filters allow it.
      if (this.lastDeleted?.id === id) {
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
   * (double-click on Undo / Restore buttons must be one operation). */
  restoring = $state<boolean>(false);

  async removeAll(): Promise<DeleteAllResult> {
    try {
      this.invalidateActiveQueries();
      const result = await deleteAllRecordings();
      // COMMIT fence (RC-1) — before the surgery/refreshes below. No
      // reload-on-killed-load is needed here: the surgery's empty list IS
      // the exact post-commit truth under every filter.
      this.fenceMutationCompletion();
      this.list = [];
      this.hasMore = false;
      this.nextRawOffset = 0;
      // The empty list is accurate FOR THE CURRENT filter — tag it so a
      // later same-filter refresh failure keeps showing this truth.
      this.listFilterKey = this.currentFilterKey();
      this.selectedRecording = null;
      this.activeTotal = 0;
      // Everything just moved into Trash — keep its badge AND any loaded
      // trash page honest too.
      this.refreshTrashedAfterMutation();
      // If a date filter was active, the availability refresh below proves
      // its day emptied and clears it with the polite notice (the generic
      // current-availability path — no special-casing here).
      void this.refreshAvailability();
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
      this.invalidateActiveQueries();
      const result = await restoreRecordings(ids);
      // COMMIT fence (RC-1) — before the refreshes below.
      this.fenceMutationCompletion();
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
  /// load()/loadTrashed() catch their own errors into their error states,
  /// so this never converts a successful mutation into a rejection.
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
      this.invalidateActiveQueries();
      const result = await restoreRecordings(ids);
      // COMMIT fence (RC-1) — before the refreshes below.
      this.fenceMutationCompletion();
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
      this.invalidateActiveQueries();
      const result = await restoreAllTrashed();
      // COMMIT fence (RC-1) — before the refreshes below.
      this.fenceMutationCompletion();
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
      this.invalidateActiveQueries();
      const result = await restoreRecordingsDeletedBetween(startIso, endIso);
      // COMMIT fence (RC-1) — before the refreshes below.
      this.fenceMutationCompletion();
      await this.refreshAfterMutation();
      return result.count;
    } finally {
      this.restoring = false;
    }
  }

  /// Hook for the Recordings tab's Active↔Trash switch (UI batch): the
  /// user RETURNING to Active must not see a page cached from before the
  /// restores they just did in Trash. Reloads the first page + count +
  /// availability under the CURRENT filters (which the switch preserves);
  /// the editor selection/draft is untouched.
  async onActiveViewReturn(): Promise<void> {
    await this.load();
  }

  /// A background sync round merged remote data: mark pre-merge
  /// availability stale (generation bump) and refresh it plus the trash
  /// badge — WITHOUT reloading the visible list. A 5-minute timer must
  /// not yank the user's scroll position; the list picks changes up on
  /// the next natural load()/event. (If the merged data emptied the
  /// selected day, the availability path clears it with the notice and
  /// reloads — that targeted reload is by design.)
  onBackgroundSyncMerged(): void {
    this.dataGeneration++;
    void this.refreshTrashedTotal();
    void this.refreshAvailability();
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
        // The sync mutated data (merges, tombstones, revivals): an
        // availability response issued before it completed is pre-merge
        // and must not survive.
        this.dataGeneration++;
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
      // The event means remote data changed: pre-event availability is
      // stale (generation bump) and the reload refreshes it alongside.
      this.dataGeneration++;
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
        // Sync merges tombstones/revivals — the badge AND date
        // availability move with them (mirrors syncNow's post-sync
        // refresh, minus the visible-list reload).
        recordings.onBackgroundSyncMerged();
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
    console.warn('Background sync stopped');
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
/// The date FILTER's day bounds (dayIntervalUtcForKey in
/// utils/recordingDates.ts) carry identical semantics — pinned equivalent
/// by test.
export function localDayIntervalUtc(day: Date): { startIso: string; endIso: string } {
  const start = new Date(day.getFullYear(), day.getMonth(), day.getDate());
  const end = new Date(day.getFullYear(), day.getMonth(), day.getDate() + 1);
  return { startIso: start.toISOString(), endIso: end.toISOString() };
}
