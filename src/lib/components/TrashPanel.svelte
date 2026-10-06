<script lang="ts">
  import { recordings } from '../stores/recordings.svelte';
  import { toasts } from '../stores/toasts.svelte';
  import SearchBar from './SearchBar.svelte';
  import TrashRecordingRow from './TrashRecordingRow.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';

  /// Trash view panel. Search state is LOCAL and separate from the Active
  /// view's — entering Trash always starts with an empty query. The search
  /// filters the loaded rows client-side (trashed rows are de-indexed from
  /// FTS, so there is no backend search); "N matches · M in Trash" keeps
  /// the authoritative total visible while filtered, and bulk actions
  /// always operate on ALL trashed records regardless of page or filter.

  let searchQuery = $state('');
  /// The visible input text, bound so "Clear search" empties BOTH the
  /// filter and the box (the debounced onSearch only flows one way).
  let searchInput = $state('');
  let showRestoreAll = $state(false);

  /// The ONE polite live region for trash-local results (D9): per-row
  /// restore completions land here — never ALSO as a toast (no double
  /// announce). Actionable failures use toasts (role=alert after D9).
  let statusMessage = $state('');

  const filtered = $derived.by(() => {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return recordings.trashedList;
    return recordings.trashedList.filter(
      (r) =>
        r.filename.toLowerCase().includes(q) ||
        (r.patient_name ?? '').toLowerCase().includes(q),
    );
  });

  function announce(msg: string) {
    statusMessage = msg;
  }

  async function retryLoad() {
    await recordings.loadTrashed();
  }

  function clearSearch() {
    searchQuery = '';
    searchInput = '';
  }

  /// Per-row restore with focus management (D9): the invoking row
  /// disappears when the refresh lands, so focus moves to the NEXT row's
  /// Restore button — or the Trash heading when it was the last row —
  /// never to document.body.
  async function restoreRow(id: string) {
    // Pick the fallback BEFORE the await: the row may leave `filtered`.
    const ids = filtered.map((r) => r.id);
    const nextId = ids[ids.indexOf(id) + 1];
    try {
      const count = await recordings.restoreTrashed([id]);
      announce(
        count > 0
          ? 'Recording restored to Active.'
          : 'Recording was no longer in Trash.',
      );
      requestAnimationFrame(() => {
        const next =
          (nextId !== undefined
            ? document.querySelector(`[data-restore-btn="${nextId}"]`)
            : null) ?? document.querySelector('[data-trash-heading]');
        (next as HTMLElement | null)?.focus();
      });
    } catch (err) {
      console.error('Failed to restore recording:', err);
      toasts.error(`Couldn't restore recording: ${err}`);
    }
  }

  /// "Restore all" — operates on EVERY trashed recording (the dialog says
  /// so), not the loaded page or the active search filter. Duplicate
  /// submissions are one restore: the dialog closes before the await and
  /// the store's in-flight guard rejects any racing second call.
  async function confirmRestoreAll() {
    showRestoreAll = false;
    try {
      const count = await recordings.restoreAllFromTrash();
      const noun = count === 1 ? 'recording' : 'recordings';
      announce(`${count} ${noun} restored to Active.`);
    } catch (err) {
      console.error('Failed to restore all recordings:', err);
      toasts.error(`Couldn't restore recordings: ${err}`);
    }
  }
</script>

<div class="trash-panel">
  <h3
    class="trash-heading"
    data-trash-heading
    tabindex="-1"
    id="trash-heading"
  >
    Trash
  </h3>
  <p class="trash-explain">
    Recordings stay in Trash for 30 days. After that, their audio, transcripts,
    SOAP notes, and generated documents are permanently deleted.
  </p>

  <div class="trash-toolbar">
    <span class="trash-count">
      {recordings.trashedTotal}
      {recordings.trashedTotal === 1 ? 'recording' : 'recordings'} in Trash
    </span>
    <button
      class="btn-restore-all"
      onclick={() => (showRestoreAll = true)}
      disabled={recordings.trashedTotal === 0}
    >
      Restore all…
    </button>
  </div>

  <SearchBar
    bind:value={searchInput}
    placeholder="Search Trash…"
    label="Search Trash"
    inputId="trash-search-input"
    onSearch={(q) => (searchQuery = q)}
  />

  {#if searchQuery.trim() !== ''}
    <!-- Static match count — deliberately NOT a live region (D9: one polite
         status region per view, reserved for previews/completions). -->
    <p class="match-line">
      {filtered.length} {filtered.length === 1 ? 'match' : 'matches'} ·
      {recordings.trashedTotal} in Trash
    </p>
  {/if}

  <div class="trash-status sr-only" role="status" aria-live="polite">
    {statusMessage}
  </div>

  <div class="trash-list">
    {#if recordings.trashedLoading}
      <div class="state-msg">
        <span>Loading Trash…</span>
      </div>

    {:else if recordings.trashedError !== null}
      <!-- A load failure must NEVER render as an empty trash. -->
      <div class="state-msg">
        <p>Couldn't load Trash. Try again.</p>
        <button class="btn-retry" onclick={retryLoad}>Retry</button>
      </div>

    {:else if recordings.trashedTotal === 0}
      <div class="state-msg">
        <div class="state-icon">🗑️</div>
        <p>Trash is empty.</p>
        <p class="hint">
          Recordings moved to Trash appear here for 30 days before permanent
          deletion.
        </p>
      </div>

    {:else if filtered.length === 0}
      <div class="state-msg">
        <p>No recordings in Trash match your search.</p>
        <button class="btn-retry" onclick={clearSearch}>Clear search</button>
      </div>

    {:else}
      {#each filtered as item (item.id)}
        <TrashRecordingRow
          item={item}
          onRestore={() => restoreRow(item.id)}
          restoreDisabled={recordings.restoring}
        />
      {/each}
      {#if recordings.trashedHasMore}
        <div class="load-more">
          <button
            class="btn-load-more"
            onclick={() => recordings.loadMoreTrashed()}
            disabled={recordings.trashedLoadingMore}
          >
            {recordings.trashedLoadingMore ? 'Loading…' : 'Load more'}
          </button>
        </div>
      {/if}
    {/if}
  </div>
</div>

{#if showRestoreAll && recordings.trashedTotal > 0}
  {@const restoreAllN = recordings.trashedTotal}
  {@const restoreAllNoun = restoreAllN === 1 ? 'recording' : 'recordings'}
  <ConfirmDialog
    open={true}
    title={`Restore all ${restoreAllN} ${restoreAllNoun} from Trash?`}
    message="They will return to Active with their saved audio and documents. This restores every recording in Trash, not just the ones currently listed."
    confirmLabel={`Restore all ${restoreAllN}`}
    danger={false}
    onConfirm={confirmRestoreAll}
    onCancel={() => (showRestoreAll = false)}
  />
{/if}

<style>
  .trash-panel {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .trash-heading {
    font-size: 15px;
    font-weight: 600;
    color: var(--text-primary);
    padding: 12px 12px 0;
  }

  .trash-heading:focus {
    outline: none;
  }

  .trash-explain {
    font-size: 12px;
    color: var(--text-muted);
    padding: 4px 12px 8px;
    margin: 0;
    line-height: 1.5;
  }

  .trash-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
  }

  .trash-count {
    font-size: 12px;
    color: var(--text-muted);
  }

  .btn-restore-all {
    padding: 4px 10px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .btn-restore-all:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-restore-all:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .match-line {
    font-size: 12px;
    color: var(--text-muted);
    padding: 6px 12px;
    margin: 0;
    border-bottom: 1px solid var(--border);
  }

  .trash-list {
    flex: 1;
    overflow-y: auto;
  }

  .state-msg {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 40px 20px;
    text-align: center;
    color: var(--text-muted);
  }

  .state-icon {
    font-size: 40px;
    margin-bottom: 8px;
  }

  .state-msg p {
    font-size: 14px;
    margin: 0;
  }

  .hint {
    font-size: 12px;
  }

  .btn-retry {
    margin-top: 6px;
    padding: 6px 16px;
    font-size: 13px;
    font-weight: 500;
    color: var(--accent);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .btn-retry:hover {
    background-color: var(--bg-hover);
    border-color: var(--accent);
  }

  .load-more {
    display: flex;
    justify-content: center;
    padding: 16px 12px;
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
