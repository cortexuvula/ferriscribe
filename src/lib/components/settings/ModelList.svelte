<script lang="ts">
  /**
   * Shared downloadable-model list for the Audio / STT pane — one row per
   * model with description, size, a "Downloaded" badge or Download button
   * (live percentage while in flight), and a Delete button. Extracted from
   * the two ~identical copies in WhisperLocalSection /
   * DiarizationModelsSection; the only per-list difference (whether the
   * active model's Delete is blocked) is injected via isDeleteDisabled.
   */
  import type { DownloadableModel } from '../../api/models';

  interface Props {
    models: DownloadableModel[];
    downloadingModels: Set<string>;
    downloadProgress: Record<string, { downloaded: number; total: number }>;
    onDownload: (modelId: string) => void;
    onDelete: (modelId: string) => void;
    formatBytes: (bytes: number) => string;
    /** Return true to disable a model's Delete button (e.g. the active whisper model). */
    isDeleteDisabled?: (model: DownloadableModel) => boolean;
    /** Tooltip shown when Delete is disabled. */
    deleteDisabledTitle?: string;
    /** Tooltip shown when Delete is enabled. */
    deleteTitle?: string;
  }

  let {
    models,
    downloadingModels,
    downloadProgress,
    onDownload,
    onDelete,
    formatBytes,
    isDeleteDisabled,
    deleteDisabledTitle,
    deleteTitle = 'Delete to free disk space',
  }: Props = $props();
</script>

<div class="model-list">
  {#each models as model (model.id)}
    <div class="model-row">
      <div class="model-info">
        <span class="model-name">{model.id}</span>
        <span class="model-desc">{model.description}</span>
        <span class="model-size">{formatBytes(model.size_bytes)}</span>
      </div>
      <div class="model-actions">
        {#if model.downloaded}
          <span class="badge-downloaded">Downloaded</span>
          {@const deleteBlocked = isDeleteDisabled?.(model) ?? false}
          <button
            class="btn-delete-model"
            onclick={() => onDelete(model.id)}
            disabled={deleteBlocked}
            title={deleteBlocked ? (deleteDisabledTitle ?? deleteTitle) : deleteTitle}
          >
            Delete
          </button>
        {:else if downloadingModels.has(model.id)}
          <div class="download-progress">
            {#if downloadProgress[model.id]}
              {@const pct = Math.round(
                (downloadProgress[model.id].downloaded / (downloadProgress[model.id].total || 1)) * 100,
              )}
              <div
                class="progress-track"
                role="progressbar"
                aria-label={`Downloading ${model.id}`}
                aria-valuenow={pct}
                aria-valuemin={0}
                aria-valuemax={100}
              >
                <div class="progress-fill" style={`width: ${pct}%`}></div>
              </div>
              <span class="progress-pct">{pct}%</span>
            {:else}
              <span class="progress-pct">Starting...</span>
            {/if}
          </div>
        {:else}
          <button
            class="btn-download-model"
            onclick={() => onDownload(model.id)}
            disabled={downloadingModels.size > 0}
          >
            Download
          </button>
        {/if}
      </div>
    </div>
  {/each}
</div>

<style>
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
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 120px;
  }

  .progress-track {
    flex: 1;
    height: 6px;
    border-radius: 3px;
    background-color: var(--bg-hover);
    border: 1px solid var(--border);
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background-color: var(--accent);
    transition: width 0.2s ease;
  }

  .progress-pct {
    font-size: 12px;
    font-weight: 500;
    color: var(--accent);
    min-width: 3ch;
    text-align: right;
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
