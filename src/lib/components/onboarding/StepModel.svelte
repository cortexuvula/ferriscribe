<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { settings } from '../../stores/settings.svelte';
  import { listWhisperModels, downloadModel } from '../../api/models';
  import { reinitProviders } from '../../api/chat';
  import type { DownloadableModel } from '../../api/models';

  interface Props { onNext: () => void; onSkip: () => void; }
  const { onNext, onSkip }: Props = $props();

  let models = $state<DownloadableModel[]>([]);
  let selected = $state(settings.state.whisper_model);
  let downloadingModel = $state<string | null>(null);
  let downloadProgress = $state<Record<string, { downloaded: number; total: number }>>({});
  let progressUnlisten: (() => void) | undefined;
  // Race guard: the Skip button can unmount this step before the async
  // onMount's listen() resolves — a late resolution unregisters itself
  // instead of leaking (see settings/Audio.svelte for the full rationale).
  let disposed = false;

  onMount(async () => {
    try {
      models = await listWhisperModels();
      // Sync the selector to settings if the stored model is in the catalog.
      if (!models.find((m) => m.id === selected) && models.length > 0) {
        selected = models[0].id;
      }
    } catch (e) {
      console.error('Failed to list whisper models', e);
    }
    const un = await listen<{ model_id: string; downloaded_bytes: number; total_bytes: number }>(
      'model-download-progress',
      (event) => {
        downloadProgress = {
          ...downloadProgress,
          [event.payload.model_id]: {
            downloaded: event.payload.downloaded_bytes,
            total: event.payload.total_bytes,
          },
        };
      },
    );
    if (disposed) un();
    else progressUnlisten = un;
  });

  onDestroy(() => {
    disposed = true;
    progressUnlisten?.();
  });

  async function handleDownload(modelId: string) {
    downloadingModel = modelId;
    try {
      await downloadModel(modelId);
      models = await listWhisperModels(); // refresh `downloaded` flags
    } catch (e) {
      console.error('Download failed', e);
    } finally {
      downloadingModel = null;
    }
  }

  function pct(modelId: string): number | null {
    const p = downloadProgress[modelId];
    if (!p || p.total === 0) return null;
    return Math.round((p.downloaded / p.total) * 100);
  }

  async function saveAndNext() {
    await settings.updateField('whisper_model', selected);
    // The provider resolves the whisper model at build time — a download
    // mid-onboarding rebuilt it with the OLD selection, so rebuild again
    // now that the choice is persisted.
    try {
      await reinitProviders();
    } catch (err) {
      console.error('Failed to reinit providers after whisper model choice:', err);
    }
    onNext();
  }
</script>

<h2>Choose a transcription model</h2>
<p class="hint">Whisper transcribes your recordings on-device. The default (large-v3-turbo) is most accurate but largest (~1.6 GB). Pick a smaller model for faster setup, or download later from Settings.</p>

{#if models.length === 0}
  <p class="hint">Loading models…</p>
{:else}
  <div class="field">
    <label for="ob-model">Model</label>
    <select id="ob-model" bind:value={selected}>
      {#each models as m (m.id)}
        <option value={m.id}>{m.id} — {m.description}</option>
      {/each}
    </select>
  </div>

  {@const selectedModel = models.find((m) => m.id === selected)}
  {#if selectedModel}
    <div class="model-row">
      <div class="model-meta">
        <span class="model-size">{(selectedModel.size_bytes / 1024 / 1024).toFixed(0)} MB</span>
        {#if selectedModel.downloaded}
          <span class="model-ok">✓ Downloaded</span>
        {:else if downloadingModel === selectedModel.id}
          <span class="model-progress">Downloading… {pct(selectedModel.id) ?? 0}%</span>
        {:else}
          <button class="btn-download" onclick={() => handleDownload(selectedModel.id)}>
            Download
          </button>
        {/if}
      </div>
    </div>
  {/if}
{/if}

<div class="actions">
  <button class="btn-skip" onclick={onSkip}>Skip for now</button>
  <button class="btn-primary" onclick={saveAndNext} disabled={downloadingModel !== null}>
    {downloadingModel ? 'Downloading…' : 'Next →'}
  </button>
</div>

<style>
  h2 { font-size: 18px; font-weight: 600; margin: 0 0 4px; color: var(--text-primary); }
  .hint { font-size: 13px; color: var(--text-muted); margin: 0 0 16px; line-height: 1.5; }
  .field { display: flex; flex-direction: column; gap: 4px; margin-bottom: 12px; }
  label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; color: var(--text-secondary); }
  select {
    height: 32px; padding: 0 10px; font-size: 13px; color: var(--text-primary);
    background-color: var(--bg-primary, #1a1a1a); border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px);
  }
  select:focus { outline: none; border-color: var(--accent, #3b82f6); }
  .model-row { padding: 10px 0; }
  .model-meta { display: flex; align-items: center; gap: 12px; }
  .model-size { font-size: 12px; color: var(--text-muted); }
  .model-ok { font-size: 12px; color: var(--success, #22c55e); font-weight: 500; }
  .model-progress { font-size: 12px; color: var(--accent, #3b82f6); }
  .btn-download {
    padding: 4px 12px; font-size: 12px; font-weight: 500; color: var(--accent, #3b82f6);
    background-color: color-mix(in srgb, var(--accent, #3b82f6) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent, #3b82f6) 30%, transparent);
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-download:hover { background-color: color-mix(in srgb, var(--accent, #3b82f6) 20%, transparent); }
  .actions { display: flex; justify-content: space-between; align-items: center; margin-top: 16px; }
  .btn-skip { padding: 6px 10px; font-size: 12px; color: var(--text-muted); background: none; border: none; cursor: pointer; text-decoration: underline; }
  .btn-primary {
    padding: 8px 20px; font-size: 13px; font-weight: 600; color: white;
    background-color: var(--accent, #3b82f6); border: none;
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-primary:hover:not(:disabled) { background-color: var(--accent-hover, #2563eb); }
  .btn-primary:disabled { opacity: 0.6; cursor: not-allowed; }
</style>
