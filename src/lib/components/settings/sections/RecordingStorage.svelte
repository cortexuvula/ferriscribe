<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  async function handleBrowseStoragePath() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: 'Select Recording Storage Folder',
    });
    if (selected) {
      await settings.updateField('storage_path', selected);
    }
  }

  async function handleResetStoragePath() {
    await settings.updateField('storage_path', null);
  }

</script>

<div class="form-group">
  <span class="form-label">Recording Storage Folder</span>
  <div class="storage-path-row">
    <span class="storage-path-display">
      {settings.state.storage_path || 'Default (application data)'}
    </span>
    <button class="btn-browse" onclick={handleBrowseStoragePath}>Browse</button>
    {#if settings.state.storage_path}
      <button class="btn-reset" onclick={handleResetStoragePath}>Reset</button>
    {/if}
  </div>
  <span class="form-hint">Choose where audio recordings are saved. New recordings will use this folder.</span>
</div>

<style>
  .storage-path-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .storage-path-display {
    flex: 1;
    font-size: 12px;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 6px 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .btn-browse,
  .btn-reset {
    flex-shrink: 0;
    padding: 6px 12px;
    font-size: 12px;
    font-weight: 500;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .btn-browse {
    background-color: var(--accent);
    color: var(--text-inverse);
  }

  .btn-browse:hover {
    background-color: var(--accent-hover);
  }

  .btn-reset {
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
  }

  .btn-reset:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }


  button { min-height: 44px; }
  .storage-path-display { overflow-wrap: anywhere; white-space: normal; min-width: 120px; }
</style>
