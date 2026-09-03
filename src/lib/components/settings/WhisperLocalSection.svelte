<script lang="ts">
  import { settings } from '../../stores/settings.svelte';
  import ModelList from './ModelList.svelte';
  import type { DownloadableModel as WhisperModelInfo } from '../../api/models';

  interface Props {
    whisperModels: WhisperModelInfo[];
    modelsRefreshing: boolean;
    downloadingModels: Set<string>;
    downloadProgress: Record<string, { downloaded: number; total: number }>;
    onModelChange: (modelId: string) => Promise<void>;
    onDownload: (modelId: string) => Promise<void>;
    onDelete: (modelId: string) => Promise<void>;
    formatBytes: (bytes: number) => string;
  }

  const {
    whisperModels,
    modelsRefreshing,
    downloadingModels,
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
    value={settings.state.whisper_model}
    onchange={(e) => onModelChange((e.target as HTMLSelectElement).value)}
    disabled={modelsRefreshing}
  >
    {#each whisperModels as model (model.id)}
      <option value={model.id}>
        {model.id} ({formatBytes(model.size_bytes)}) {model.downloaded ? '' : '- not downloaded'}
      </option>
    {/each}
  </select>
  <span class="form-hint">Larger models are more accurate but use more memory and take longer.</span>
</div>

<div class="form-group">
  <span class="form-label">Model Management</span>
  <ModelList
    models={whisperModels}
    {downloadingModels}
    {downloadProgress}
    {onDownload}
    {onDelete}
    {formatBytes}
    isDeleteDisabled={(m) => m.id === settings.state.whisper_model}
    deleteDisabledTitle="Cannot delete the active model"
  />
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
</style>
