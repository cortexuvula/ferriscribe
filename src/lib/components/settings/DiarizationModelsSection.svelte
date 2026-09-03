<script lang="ts">
  import ModelList from './ModelList.svelte';
  import type { DownloadableModel as WhisperModelInfo } from '../../api/models';

  interface Props {
    pyannoteModels: WhisperModelInfo[];
    downloadingModels: Set<string>;
    downloadProgress: Record<string, { downloaded: number; total: number }>;
    onDownload: (modelId: string) => Promise<void>;
    onDelete: (modelId: string) => Promise<void>;
    formatBytes: (bytes: number) => string;
  }

  const {
    pyannoteModels,
    downloadingModels,
    downloadProgress,
    onDownload,
    onDelete,
    formatBytes,
  }: Props = $props();
</script>

<p class="form-hint">Diarization runs on this machine regardless of STT mode — pyannote models below are required for speaker labels.</p>

<div class="form-group">
  <span class="form-label">Diarization Models (Speaker Identification)</span>
  <span class="form-hint">Both models are required for speaker diarization. Without them, transcripts will not have speaker labels.</span>
  <ModelList
    models={pyannoteModels}
    {downloadingModels}
    {downloadProgress}
    {onDownload}
    {onDelete}
    {formatBytes}
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
