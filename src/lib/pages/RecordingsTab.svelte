<script lang="ts">
  import { onMount } from 'svelte';
  import { recordings, selectRecording } from '../stores/recordings.svelte';
  import { pipeline } from '../stores/pipeline.svelte';
  import { toasts } from '../stores/toasts.svelte';
  import SearchBar from '../components/SearchBar.svelte';
  import RecordingCard from '../components/RecordingCard.svelte';
  import ConfirmDialog from '../components/ConfirmDialog.svelte';

  let deleteTarget = $state<{ id: string; name: string } | null>(null);
  let showDeleteAll = $state(false);

  onMount(() => {
    recordings.load();
  });

  function requestDelete(id: string, name: string) {
    deleteTarget = { id, name };
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    try {
      await recordings.remove(deleteTarget.id);
    } catch (err) {
      console.error('Failed to delete recording:', err);
      toasts.error(`Failed to delete recording: ${err}`);
    } finally {
      deleteTarget = null;
    }
  }

  async function confirmDeleteAll() {
    try {
      await recordings.removeAll();
    } catch (err) {
      console.error('Failed to delete all recordings:', err);
      toasts.error(`Failed to delete all recordings: ${err}`);
    } finally {
      showDeleteAll = false;
    }
  }

  function retryTranscription(id: string) {
    pipeline.retry(id);
    toasts.success('Starting re-transcription…');
  }
</script>

<div class="recordings-tab">
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
      </div>

    {:else}
      <div class="list-toolbar">
        <span class="recording-count">{recordings.list.length} recording{recordings.list.length === 1 ? '' : 's'}</span>
        <button
          class="btn-delete-all"
          onclick={() => showDeleteAll = true}
        >
          Delete All
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

<ConfirmDialog
  open={deleteTarget !== null}
  title="Delete Recording"
  message={deleteTarget ? `This will permanently delete "${deleteTarget.name}" including its audio file, transcript, SOAP note, and all generated documents.` : ''}
  confirmLabel="Delete"
  onConfirm={confirmDelete}
  onCancel={() => deleteTarget = null}
/>

<ConfirmDialog
  open={showDeleteAll}
  title="Delete All Recordings"
  message={`This will permanently delete all ${recordings.list.length} recording${recordings.list.length === 1 ? '' : 's'}, including audio files, transcripts, SOAP notes, and all generated documents. This cannot be undone.`}
  confirmLabel="Delete All"
  onConfirm={confirmDeleteAll}
  onCancel={() => showDeleteAll = false}
/>

<style>
  .recordings-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .recordings-list {
    flex: 1;
    overflow-y: auto;
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

  .btn-delete-all {
    padding: 4px 10px;
    font-size: 12px;
    font-weight: 500;
    color: var(--danger, #ef4444);
    background-color: transparent;
    border: 1px solid var(--danger, #ef4444);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .btn-delete-all:hover {
    background-color: rgba(239, 68, 68, 0.1);
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
