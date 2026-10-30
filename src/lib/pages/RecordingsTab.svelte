<script lang="ts">
  import { onMount } from 'svelte';
  import { recordings, selectRecording } from '../stores/recordings.svelte';
  import { pipeline } from '../stores/pipeline.svelte';
  import { settings } from '../stores/settings.svelte';
  import { toasts } from '../stores/toasts.svelte';
  import SearchBar from '../components/SearchBar.svelte';
  import RecordingCard from '../components/RecordingCard.svelte';
  import RecordingDateCalendar from '../components/RecordingDateCalendar.svelte';
  import ConfirmDialog from '../components/ConfirmDialog.svelte';
  import RecordingViewSwitch from '../components/RecordingViewSwitch.svelte';
  import TrashPanel from '../components/TrashPanel.svelte';
  import { sanitizedErr } from '../utils/sanitizedErr';
  import { fullLocalDateFromKey, localDateKey } from '../utils/recordingDates';

  let deleteTarget = $state<{ id: string; name: string } | null>(null);
  let showMoveAll = $state(false);
  const retentionDays = $derived(settings.trashRetentionDays);
  /// Active | Trash view. The switch is ALWAYS rendered — an empty Active
  /// list must never hide the route to Trash recovery.
  let view = $state<'active' | 'trash'>('active');

  /// The VISIBLE search text. Bound to the SearchBar (never to
  /// `recordings.searchQuery` directly — a direct binding would mutate the
  /// effective filter on every keystroke while the debounced request that
  /// actually invalidates prior queries is still pending). The store stays
  /// the source of truth; this mirror initializes from it on every remount
  /// and is rewritten by the programmatic clears below (which the SearchBar
  /// treats as an external change, cancelling any pending debounce).
  let searchText = $state(recordings.searchQuery);

  const searchActive = $derived(recordings.searchQuery.trim() !== '');
  const dateActive = $derived(recordings.selectedDate !== null);
  const filtersActive = $derived(searchActive || dateActive);
  const activeTotal = $derived(recordings.activeTotal ?? 0);

  /// Filtered count vs the authoritative total — the honest "what you see
  /// vs what exists" line while a date/search filter is applied.
  const countLabel = $derived.by(() => {
    const n = recordings.list.length;
    if (!filtersActive) return `${activeTotal} recording${activeTotal === 1 ? '' : 's'}`;
    return `${n}${recordings.hasMore ? '+' : ''} of ${activeTotal} recordings`;
  });

  /// The outside-date notice is shown ONLY when provable: the SELECTED
  /// recording's own created_at local day is known and differs from the
  /// selected date. Absence from a paginated/filtered list proves nothing.
  const selectionOutsideDate = $derived.by(() => {
    const day = recordings.selectedDate;
    const rec = recordings.selectedRecording;
    if (!day || !rec) return false;
    const created = rec.created_at;
    if (typeof created !== 'string') return false;
    try {
      return localDateKey(created) !== day;
    } catch {
      return false;
    }
  });

  onMount(() => {
    recordings.load();
    // Badge count only — the trash page loads on every Trash entry.
    void recordings.refreshTrashedTotal();
  });

  function switchView(next: 'active' | 'trash') {
    view = next;
    if (next === 'trash') {
      // ALWAYS reload page 1 on entry: rows move in and out of Trash from
      // the Active view, the Undo toast, and sync, and none of those keep
      // the cached page fresh — the old load-only-if-empty check is what
      // let a stale list through. One bounded page fetch per entry.
      void recordings.loadTrashed();
    } else {
      // Returning to Active: reload the first page under the CURRENT
      // filters (which survive the round trip) so restores done in Trash
      // are reflected; availability + count ride along with load().
      void recordings.onActiveViewReturn();
    }
  }

  /// Clear ONLY the search: the visible input empties (cancelling any
  /// pending debounce in the SearchBar) and the store re-queries with the
  /// date filter preserved.
  async function clearSearch() {
    searchText = '';
    await recordings.search('');
  }

  /// Clear ONLY the date; the search filter is preserved by the store.
  async function clearDate() {
    await recordings.setSelectedDate(null);
  }

  function requestDelete(id: string, name: string) {
    deleteTarget = { id, name };
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    // Capture the target into locals before any async work. The toast's
    // onAction closure captures these values by value, so the finally-block
    // nulling deleteTarget can no longer null them out from under the Undo
    // callback (Bug H8).
    const targetId = deleteTarget.id;
    // Null the dialog state BEFORE awaiting: the confirm buttons stay
    // rendered during the delete, and a second click would pass the
    // guard above and fire a duplicate destructive invoke.
    deleteTarget = null;
    try {
      await recordings.remove(targetId);
      toasts.add({
        message: 'Recording moved to Trash.',
        type: 'success',
        autoDismiss: true,
        actionLabel: 'Undo',
        onAction: async () => {
          try {
            await recordings.restore(targetId);
            toasts.success('Recording restored');
          } catch (err) {
            toasts.error(`Could not restore: ${sanitizedErr(err)}`);
          }
        },
      });
    } catch (err) {
      console.error('Failed to move recording to Trash:', err);
      toasts.error(`Failed to move recording to Trash: ${sanitizedErr(err)}`);
    }
  }

  function openMoveAllDialog() {
    // Refresh the authoritative count right before the dialog renders —
    // the list may be stale or filtered, and the dialog promises "all N".
    void recordings.refreshActiveTotal();
    showMoveAll = true;
  }

  async function confirmMoveAll() {
    // Clear BEFORE awaiting — same double-click shape as confirmDelete.
    showMoveAll = false;
    try {
      const result = await recordings.removeAll();
      toasts.add({
        message: `${result.count} moved to Trash. Restore them within ${retentionDays} days.`,
        type: 'success',
        autoDismiss: true,
        actionLabel: 'Undo',
        onAction: async () => {
          try {
            const restored = await recordings.undoMoveAll();
            const restoredNoun = restored === 1 ? 'recording' : 'recordings';
            toasts.success(`${restored} ${restoredNoun} restored to Active.`);
          } catch (err) {
            console.error('Failed to undo move-all:', err);
            toasts.error(`Could not restore: ${sanitizedErr(err)}`);
          }
        },
      });
    } catch (err) {
      console.error('Failed to move all recordings to Trash:', err);
      toasts.error(`Failed to move recordings to Trash: ${sanitizedErr(err)}`);
    }
  }

  function retryTranscription(id: string) {
    pipeline.retry(id);
    toasts.success('Starting re-transcription…');
  }
</script>

<div class="recordings-tab">
  <RecordingViewSwitch view={view} trashCount={recordings.trashedTotal} onChange={switchView} />

  {#if view === 'trash'}
    <div
      id="view-panel-trash"
      role="tabpanel"
      aria-labelledby="view-tab-trash"
      class="view-panel"
    >
      <TrashPanel />
    </div>
  {:else}
    <div
      id="view-panel-active"
      role="tabpanel"
      aria-labelledby="view-tab-active"
      class="view-panel"
    >
      <div class="filter-bar">
        <div class="search-wrap">
          <SearchBar
            bind:value={searchText}
            bare
            placeholder="Search recordings…"
            onSearch={(q) => void recordings.search(q)}
          />
        </div>
        <RecordingDateCalendar
          availabilityCounts={recordings.availabilityCounts}
          availabilityLoading={recordings.availabilityLoading}
          availabilityError={recordings.availabilityError}
          availabilityLoaded={recordings.availabilityLoaded}
          selectedDate={recordings.selectedDate}
          onSelectDate={(key) => void recordings.setSelectedDate(key)}
          onRefreshAvailability={() => void recordings.refreshAvailability()}
        />
      </div>

      {#if dateActive}
        <div class="active-filter-row">
          <span class="date-chip">
            <span class="chip-label">Recorded on {fullLocalDateFromKey(recordings.selectedDate ?? '')}</span>
            <button type="button" class="chip-clear" onclick={clearDate}>
              Clear recording date
            </button>
          </span>
        </div>
      {/if}

      {#if recordings.dateClearedNotice}
        <div class="active-filter-row notice-row">
          <p class="date-cleared-notice" role="status">{recordings.dateClearedNotice}</p>
          <button
            type="button"
            class="chip-clear"
            onclick={() => recordings.dismissDateClearedNotice()}
          >
            Dismiss
          </button>
        </div>
      {/if}

      {#if selectionOutsideDate}
        <p class="outside-date-note">Selected recording is outside the selected date.</p>
      {/if}

      <div class="recordings-list">
        {#if recordings.loading}
          <div class="state-msg">
            <span>Loading recordings…</span>
          </div>

        {:else if recordings.listError !== null}
          <div class="state-msg">
            <p>Couldn't load recordings.</p>
            <p class="hint">The list may be out of date. Nothing was changed.</p>
            <button type="button" class="btn-view-trash btn-retry" onclick={() => void recordings.load()}>
              Retry
            </button>
          </div>

        {:else}
          {#if activeTotal > 0}
            <div class="list-toolbar">
              <span class="recording-count">{countLabel}</span>
              <button
                class="btn-move-all"
                onclick={openMoveAllDialog}
              >
                Move all to Trash
              </button>
            </div>
          {/if}

          {#if recordings.list.length === 0}
            {#if searchActive && dateActive}
              <div class="state-msg">
                <p>No recordings on this date match your search.</p>
                <div class="state-actions">
                  <button type="button" class="btn-view-trash" onclick={clearSearch}>Clear search</button>
                  <button type="button" class="btn-view-trash" onclick={clearDate}>Show all dates</button>
                </div>
              </div>
            {:else if dateActive}
              <div class="state-msg">
                <p>No recordings on this date.</p>
                <div class="state-actions">
                  <button type="button" class="btn-view-trash" onclick={clearDate}>Show all dates</button>
                </div>
              </div>
            {:else if searchActive}
              <div class="state-msg">
                <p>No recordings match your search.</p>
                <div class="state-actions">
                  <button type="button" class="btn-view-trash" onclick={clearSearch}>Clear search</button>
                </div>
              </div>
            {:else}
              <div class="state-msg">
                <div class="state-icon">📋</div>
                <p>No recordings yet.</p>
                <p class="hint">Go to the <strong>Record</strong> tab to capture audio.</p>
                {#if recordings.trashedTotal > 0}
                  <button class="btn-view-trash" onclick={() => switchView('trash')}>
                    View Trash
                  </button>
                {/if}
              </div>
            {/if}
          {:else}
            {#each recordings.list as rec (rec.id)}
              <RecordingCard
                recording={rec}
                selected={recordings.selectedRecording?.id === rec.id}
                onClick={() => selectRecording(rec.id)}
                onDelete={() => requestDelete(rec.id, rec.patient_name || rec.filename)}
                onRetry={() => retryTranscription(rec.id)}
              />
            {/each}
          {/if}

          {#if recordings.hasMore || recordings.loadMoreError !== null}
            <div class="load-more">
              {#if recordings.loadMoreError !== null}
                <p class="load-more-error" role="alert">
                  Couldn't load more recordings.
                  <button
                    type="button"
                    class="btn-retry-inline"
                    onclick={() => void recordings.loadMore()}
                  >
                    Retry
                  </button>
                </p>
              {/if}
              {#if recordings.hasMore}
                <button
                  class="btn-load-more"
                  onclick={() => void recordings.loadMore()}
                  disabled={recordings.loadingMore}
                >
                  {recordings.loadingMore ? 'Loading…' : 'Load more'}
                </button>
              {/if}
            </div>
          {/if}
        {/if}
      </div>
    </div>
  {/if}
</div>

<ConfirmDialog
  open={deleteTarget !== null}
  title="Move recording to Trash?"
  message={`This recording is eligible for permanent deletion after ${retentionDays} days in Trash. You can restore it from Trash during that time. After that, cleanup permanently deletes its audio, transcript, SOAP notes and generated documents.`}
  confirmLabel="Move to Trash"
  danger={false}
  onConfirm={confirmDelete}
  onCancel={() => deleteTarget = null}
/>

{#if showMoveAll && recordings.activeTotal !== null}
  {@const moveAllN = recordings.activeTotal ?? 0}
  {@const moveAllNoun = moveAllN === 1 ? 'recording' : 'recordings'}
  <ConfirmDialog
    open={true}
    title={`Move all ${moveAllN} ${moveAllNoun} to Trash?`}
    message={`This moves all active recordings to Trash, including recordings outside the selected date and current search. Recordings are eligible for permanent deletion after ${retentionDays} days in Trash. You can restore them from Trash during that time. After that, cleanup permanently deletes their audio, transcripts, SOAP notes and generated documents.`}
    confirmLabel={`Move ${moveAllN} ${moveAllNoun} to Trash`}
    danger={false}
    onConfirm={confirmMoveAll}
    onCancel={() => showMoveAll = false}
  />
{/if}

<style>
  .recordings-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .view-panel {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .filter-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    background-color: var(--bg-secondary);
  }

  .search-wrap {
    flex: 1 1 240px;
    min-width: 200px;
    display: flex;
  }

  .active-filter-row {
    display: flex;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border-light);
    background-color: var(--bg-secondary);
  }

  .date-chip {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 3px 4px 3px 10px;
    font-size: 12px;
    color: var(--text-secondary);
    background-color: var(--accent-light);
    border: 1px solid var(--accent);
    border-radius: var(--radius-md);
  }

  .chip-clear {
    padding: 2px 8px;
    font-size: 11px;
    font-weight: 500;
    color: var(--accent);
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .chip-clear:hover {
    border-color: var(--accent);
    background-color: var(--bg-hover);
  }

  .notice-row {
    align-items: center;
    gap: 8px;
  }

  .date-cleared-notice {
    font-size: 12px;
    color: var(--text-secondary);
  }

  .outside-date-note {
    padding: 6px 12px;
    font-size: 12px;
    color: var(--text-muted);
    background-color: var(--bg-secondary);
    border-bottom: 1px solid var(--border-light);
  }

  .recordings-list {
    flex: 1;
    overflow-y: auto;
  }

  .btn-view-trash {
    margin-top: 8px;
    padding: 6px 16px;
    font-size: 13px;
    font-weight: 500;
    color: var(--accent);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .btn-view-trash:hover {
    background-color: var(--bg-hover);
    border-color: var(--accent);
  }

  .state-msg {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 40px 20px;
    min-height: 160px;
    text-align: center;
    color: var(--text-muted);
    gap: 6px;
  }

  .state-icon {
    font-size: 40px;
    margin-bottom: 8px;
  }

  .state-actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
    margin-top: 8px;
  }

  .state-actions .btn-view-trash {
    margin-top: 0;
  }

  .btn-retry {
    color: var(--accent);
  }

  p {
    font-size: 14px;
  }

  .hint {
    font-size: 12px;
  }

  strong {
    color: var(--text-secondary);
  }

  .list-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
  }

  .recording-count {
    font-size: 12px;
    color: var(--text-muted);
  }

  /* Restrained by design: Move-all-to-Trash has a configurable recovery window, so
   * it must not carry the irreversible-danger treatment (red) the old
   * hard-delete button had. */
  .btn-move-all {
    padding: 4px 10px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .btn-move-all:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .load-more {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 16px 12px;
  }

  .load-more-error {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--danger);
  }

  .btn-retry-inline {
    padding: 2px 12px;
    font-size: 12px;
    font-weight: 500;
    color: var(--accent);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .btn-retry-inline:hover {
    background-color: var(--bg-hover);
    border-color: var(--accent);
  }

  .btn-load-more {
    padding: 8px 24px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
    transition: background-color 0.15s ease, border-color 0.15s ease, color 0.15s ease;
  }

  .btn-load-more:hover:not(:disabled) {
    background-color: var(--bg-hover);
    border-color: var(--accent);
    color: var(--text-primary);
  }

  .btn-load-more:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
