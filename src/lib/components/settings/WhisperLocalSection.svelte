<script lang="ts">
  import { settings } from '../../stores/settings';
  import type { ModelInfo as WhisperModelInfo } from '../../api/models';

  interface Props {
    whisperModels: WhisperModelInfo[];
    modelsRefreshing: boolean;
    downloadingModel: string | null;
    downloadProgress: Record<string, { downloaded: number; total: number }>;
    onModelChange: (modelId: string) => Promise<void>;
    onDownload: (modelId: string) => Promise<void>;
    onDelete: (modelId: string) => Promise<void>;
    formatBytes: (bytes: number) => string;
  }

  let {
    whisperModels,
    modelsRefreshing,
    downloadingModel,
    downloadProgress,
    onModelChange,
    onDownload,
    onDelete,
    formatBytes,
  }: Props = $props();
</script>

<div class="form-group">
  <label for="whisper-model" class="form-label">Whisper Model</label>
  <select
    id="whisper-model"
    value={$settings.whisper_model}
    onchange={(e) => onModelChange((e.target as HTMLSelectElement).value)}
    disabled={modelsRefreshing}
  >
    {#each whisperModels as model}
      <option value={model.id}>
        {model.id} ({formatBytes(model.size_bytes)}) {model.downloaded ? '' : '- not downloaded'}
      </option>
    {/each}
  </select>
  <span class="form-hint">Larger models are more accurate but use more memory and take longer.</span>
</div>

<div class="form-group">
  <span class="form-label">Model Management</span>
  <div class="model-list">
    {#each whisperModels as model}
      <div class="model-row">
        <div class="model-info">
          <span class="model-name">{model.id}</span>
          <span class="model-desc">{model.description}</span>
          <span class="model-size">{formatBytes(model.size_bytes)}</span>
        </div>
        <div class="model-actions">
          {#if model.downloaded}
            <span class="badge-downloaded">Downloaded</span>
            <button
              class="btn-delete-model"
              onclick={() => onDelete(model.id)}
              disabled={model.id === $settings.whisper_model}
              title={model.id === $settings.whisper_model ? 'Cannot delete the active model' : 'Delete to free disk space'}
            >
              Delete
            </button>
          {:else if downloadingModel === model.id}
            <span class="download-progress">
              {#if downloadProgress[model.id]}
                {Math.round((downloadProgress[model.id].downloaded / (downloadProgress[model.id].total || 1)) * 100)}%
              {:else}
                Starting...
              {/if}
            </span>
          {:else}
            <button
              class="btn-download-model"
              onclick={() => onDownload(model.id)}
              disabled={downloadingModel !== null}
            >
              Download
            </button>
          {/if}
        </div>
      </div>
    {/each}
  </div>
</div>

<style>
  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-label {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .form-hint {
    font-size: 11px;
    color: var(--text-muted);
  }

  .model-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .model-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    gap: 12px;
  }

  .model-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .model-name {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .model-desc {
    font-size: 11px;
    color: var(--text-muted);
  }

  .model-size {
    font-size: 11px;
    color: var(--text-muted);
  }

  .model-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .badge-downloaded {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--success);
    background-color: color-mix(in srgb, var(--success) 15%, transparent);
    border: 1px solid color-mix(in srgb, var(--success) 30%, transparent);
    border-radius: var(--radius-sm);
    padding: 1px 6px;
  }

  .download-progress {
    font-size: 12px;
    font-weight: 500;
    color: var(--accent);
  }

  .btn-download-model {
    padding: 4px 12px;
    font-size: 12px;
    font-weight: 500;
    background-color: var(--accent);
    color: var(--text-inverse);
    border-radius: var(--radius-sm);
    transition: background-color 0.15s ease;
  }

  .btn-download-model:hover:not(:disabled) {
    background-color: var(--accent-hover);
  }

  .btn-download-model:disabled {
    opacity: 0.5;
  }

  .btn-delete-model {
    padding: 4px 12px;
    font-size: 12px;
    font-weight: 500;
    color: var(--danger, #ef4444);
    background-color: transparent;
    border: 1px solid var(--danger, #ef4444);
    border-radius: var(--radius-sm);
    transition: background-color 0.15s ease;
  }

  .btn-delete-model:hover:not(:disabled) {
    background-color: rgba(239, 68, 68, 0.1);
  }

  .btn-delete-model:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }
</style>
