<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { settings } from '../../stores/settings.svelte';

  interface Props { onDone: () => void; }
  let { onDone }: Props = $props();

  let storagePath = $state(settings.state.storage_path);

  const display = $derived(storagePath || 'Default (application data)');

  async function browse() {
    const selected = await open({ directory: true });
    if (typeof selected === 'string') {
      storagePath = selected;
    }
  }

  async function resetToDefault() {
    storagePath = null;
  }

  async function done() {
    await settings.updateField('storage_path', storagePath);
    await onDone();
  }
</script>

<h2>Where should recordings be saved?</h2>
<p class="hint">Audio files, transcripts, and notes are stored here. You can use the default location or choose a folder (e.g. a synced or external drive).</p>

<div class="folder-row">
  <code class="folder-path">{display}</code>
  <div class="folder-actions">
    <button class="btn-secondary" onclick={browse}>Choose folder…</button>
    {#if storagePath}
      <button class="btn-secondary" onclick={resetToDefault}>Reset to default</button>
    {/if}
  </div>
</div>

<button class="btn-done" onclick={done}>Done — start using FerriScribe</button>

<style>
  h2 { font-size: 18px; font-weight: 600; margin: 0 0 4px; color: var(--text-primary); }
  .hint { font-size: 13px; color: var(--text-muted); margin: 0 0 16px; line-height: 1.5; }
  .folder-row { display: flex; flex-direction: column; gap: 10px; margin-bottom: 20px; }
  .folder-path {
    font-size: 12px; font-family: ui-monospace, monospace; padding: 8px 10px;
    background-color: var(--bg-primary, #1a1a1a); border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px); color: var(--text-secondary);
    word-break: break-all;
  }
  .folder-actions { display: flex; gap: 8px; }
  .btn-secondary {
    padding: 6px 14px; font-size: 12px; font-weight: 500; color: var(--text-primary);
    background-color: transparent; border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-secondary:hover { border-color: var(--accent, #3b82f6); }
  .btn-done {
    width: 100%; padding: 12px; font-size: 14px; font-weight: 600; color: white;
    background-color: var(--accent, #3b82f6); border: none;
    border-radius: var(--radius-md, 8px); cursor: pointer;
    transition: background-color 0.15s ease;
  }
  .btn-done:hover { background-color: var(--accent-hover, #2563eb); }
</style>
