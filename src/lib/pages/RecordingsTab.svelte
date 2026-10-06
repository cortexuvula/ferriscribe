<script lang="ts">
  import { onMount } from 'svelte';
  import { recordings, selectRecording } from '../stores/recordings.svelte';
  import { pipeline } from '../stores/pipeline.svelte';
  import { toasts } from '../stores/toasts.svelte';
  import SearchBar from '../components/SearchBar.svelte';
  import RecordingCard from '../components/RecordingCard.svelte';
  import ConfirmDialog from '../components/ConfirmDialog.svelte';
  import RecordingViewSwitch from '../components/RecordingViewSwitch.svelte';
  import TrashPanel from '../components/TrashPanel.svelte';
  import { sanitizedErr } from '../utils/sanitizedErr';

  let deleteTarget = $state<{ id: string; name: string } | null>(null);
  let showMoveAll = $state(false);
  /// Active | Trash view. The switch is ALWAYS rendered — an empty Active
  /// list must never hide the route to Trash recovery.
  let view = $state<'active' | 'trash'>('active');

  onMount(() => {
    recordings.load();
    // Badge count only — the full trash page loads on first entry.
    void recordings.refreshTrashedTotal();
  });

  function switchView(next: 'active' | 'trash') {
    view = next;
    if (next === 'trash' && recordings.trashedList.length === 0) {
      // Fresh trash view (or emptied by a restore) — load page 1. An
      // error state also retries here; an EMPTY result will not reload
      // until re-entered, matching the Active view's load-once behavior.
      void recordings.loadTrashed();
    }
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
      const noun = result.count === 1 ? 'recording' : 'recordings';
      toasts.add({
        message: `${result.count} ${noun} moved to Trash. Available to restore for 30 days.`,
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
      <SearchBar
        placeholder="Search recordings…"
        onSearch={(q) => recordings.search(q)}
      />

      <div class="recordings-list">
        {#if recordings.loading}
          <div class="state-msg">
            <span>Loading recordings…</span>
          </div>

        {:else if recordings.list.length === 0}
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

        {:else}
          <div class="list-toolbar">
            <span class="recording-count">{recordings.list.length} recording{recordings.list.length === 1 ? '' : 's'}</span>
            <button
              class="btn-move-all"
              onclick={openMoveAllDialog}
            >
              Move all to Trash
            </button>
          </div>
          {#each recordings.list as rec (rec.id)}
            <RecordingCard
              recording={rec}
              selected={recordings.selectedRecording?.id === rec.id}
              onClick={() => selectRecording(rec.id)}
              onDelete={() => requestDelete(rec.id, rec.patient_name || rec.filename)}
              onRetry={() => retryTranscription(rec.id)}
            />
          {/each}
          {#if recordings.hasMore}
            <div class="load-more">
              <button
                class="btn-load-more"
                onclick={() => recordings.loadMore()}
                disabled={recordings.loadingMore}
              >
                {recordings.loadingMore ? 'Loading…' : 'Load more'}
              </button>
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
  message="You can restore this recording from Trash for 30 days. After that, it and its saved audio and documents are permanently deleted."
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
    message="This moves all active recordings to Trash, including recordings outside the current search. Their audio, transcripts, SOAP notes, and generated documents are kept for 30 days. You can restore them from Trash during that time. After 30 days, they are permanently deleted."
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
    height: 100%;
    padding: 40px 20px;
    text-align: center;
    color: var(--text-muted);
    gap: 6px;
  }

  .state-icon {
    font-size: 40px;
    margin-bottom: 8px;
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

  /* Restrained by design: Move-all-to-Trash is reversible for 30 days, so
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
