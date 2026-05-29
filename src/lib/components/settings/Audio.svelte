<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { settings } from '../../stores/settings.svelte';
  import { listAudioDevices } from '../../api/audio';
  import { listWhisperModels, listPyannoteModels, downloadModel, deleteModel, type ModelInfo as WhisperModelInfo } from '../../api/models';
  import { reinitProviders } from '../../api/chat';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import type { AudioDevice } from '../../types';
  import AudioInputSection from './AudioInputSection.svelte';
  import WhisperLocalSection from './WhisperLocalSection.svelte';
  import SttRemoteSection from './SttRemoteSection.svelte';
  import DiarizationModelsSection from './DiarizationModelsSection.svelte';
  import { toasts } from '../../stores/toasts.svelte';

  let audioDevices = $state<AudioDevice[]>([]);
  let devicesLoading = $state(false);

  let whisperModels = $state<WhisperModelInfo[]>([]);
  let pyannoteModels = $state<WhisperModelInfo[]>([]);
  let modelsRefreshing = $state(false);
  let downloadingModel = $state<string | null>(null);
  let downloadProgress = $state<Record<string, { downloaded: number; total: number }>>({});
  let sttMode = $state<'local' | 'remote'>((settings.state.stt_mode as 'local' | 'remote') ?? 'local');
  let progressUnlisten: UnlistenFn | null = null;

  async function fetchAudioDevices() {
    devicesLoading = true;
    try {
      audioDevices = await listAudioDevices();
    } catch (e) {
      console.error('Failed to list audio devices:', e);
      audioDevices = [];
      toasts.error(`Failed to list audio devices: ${e}`);
    } finally {
      devicesLoading = false;
    }
  }

  async function fetchWhisperModels() {
    modelsRefreshing = true;
    try {
      whisperModels = await listWhisperModels();
    } catch (e) {
      console.error('Failed to list whisper models:', e);
    } finally {
      modelsRefreshing = false;
    }
  }

  async function fetchPyannoteModels() {
    try {
      pyannoteModels = await listPyannoteModels();
    } catch (e) {
      console.error('Failed to list pyannote models:', e);
    }
  }

  async function handleDownloadModel(modelId: string) {
    downloadingModel = modelId;
    try {
      await downloadModel(modelId);
      await Promise.all([fetchWhisperModels(), fetchPyannoteModels()]);
    } catch (e) {
      console.error(`Failed to download model ${modelId}:`, e);
    } finally {
      downloadingModel = null;
    }
  }

  async function handleDeleteModel(modelId: string) {
    try {
      await deleteModel(modelId);
      await Promise.all([fetchWhisperModels(), fetchPyannoteModels()]);
    } catch (e) {
      console.error(`Failed to delete model ${modelId}:`, e);
    }
  }

  function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1048576) return `${(bytes / 1024).toFixed(0)} KB`;
    if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(0)} MB`;
    return `${(bytes / 1073741824).toFixed(1)} GB`;
  }

  async function handleWhisperModelChange(modelId: string) {
    await settings.updateField('whisper_model', modelId);
  }

  onMount(async () => {
    const results = await Promise.allSettled([
      fetchAudioDevices(),
      fetchWhisperModels(),
      fetchPyannoteModels(),
    ]);
    const labels = ['fetchAudioDevices', 'fetchWhisperModels', 'fetchPyannoteModels'];
    for (const [i, r] of results.entries()) {
      if (r.status === 'rejected') {
        console.error(`Settings init: ${labels[i]} failed:`, r.reason);
      }
    }

    // Listen for model download progress events
    progressUnlisten = await listen<{ model_id: string; downloaded_bytes: number; total_bytes: number }>(
      'model-download-progress',
      (event) => {
        downloadProgress = {
          ...downloadProgress,
          [event.payload.model_id]: {
            downloaded: event.payload.downloaded_bytes,
            total: event.payload.total_bytes,
          },
        };
      }
    );
  });

  onDestroy(() => {
    progressUnlisten?.();
  });

  async function handleSampleRateChange(e: Event) {
    const value = parseInt((e.target as HTMLSelectElement).value, 10);
    await settings.updateField('sample_rate', value);
  }
</script>

<section class="settings-section">
  <h3 class="section-title">Audio / STT</h3>

  <AudioInputSection {audioDevices} {devicesLoading} />

  <fieldset class="form-group radio-fieldset">
    <legend class="form-label">STT Mode</legend>
    <div class="radio-row">
      <label class="radio-label">
        <input
          type="radio"
          bind:group={sttMode}
          value="local"
          onchange={async () => {
            await settings.updateField('stt_mode', sttMode);
            await reinitProviders();
          }}
        /> Local
      </label>
      <label class="radio-label">
        <input
          type="radio"
          bind:group={sttMode}
          value="remote"
          onchange={async () => {
            await settings.updateField('stt_mode', sttMode);
            await reinitProviders();
          }}
        /> Remote
      </label>
    </div>
  </fieldset>

  {#if sttMode === 'local'}
    <WhisperLocalSection
      {whisperModels}
      {modelsRefreshing}
      {downloadingModel}
      {downloadProgress}
      onModelChange={handleWhisperModelChange}
      onDownload={handleDownloadModel}
      onDelete={handleDeleteModel}
      {formatBytes}
    />
  {:else}
    <SttRemoteSection />
  {/if}

  <DiarizationModelsSection
    {pyannoteModels}
    {downloadingModel}
    {downloadProgress}
    onDownload={handleDownloadModel}
    onDelete={handleDeleteModel}
    {formatBytes}
  />

  <div class="form-group">
    <label for="max-speakers" class="form-label">
      Max speakers
      <span class="badge-value">{settings.state.max_speakers ?? 'Auto'}</span>
    </label>
    <input
      id="max-speakers"
      type="range"
      min={1}
      max={8}
      value={settings.state.max_speakers ?? 3}
      oninput={(e: Event) => {
        const value = parseInt((e.target as HTMLInputElement).value, 10);
        settings.updateField('max_speakers', value);
      }}
    />
    <span class="form-hint">Limits the number of speaker clusters. Set to the expected number of people in the conversation (typically 2–3).</span>
  </div>

  <div class="form-group">
    <label for="sample-rate" class="form-label">Sample Rate</label>
    <select
      id="sample-rate"
      value={settings.state.sample_rate}
      onchange={handleSampleRateChange}
    >
      <option value={16000}>16000 Hz</option>
      <option value={44100}>44100 Hz</option>
      <option value={48000}>48000 Hz</option>
    </select>
  </div>

  <div class="form-group">
    <label class="form-label checkbox-label">
      <input
        type="checkbox"
        checked={settings.state.auto_generate_soap}
        onchange={(e: Event) => {
          const checked = (e.target as HTMLInputElement).checked;
          settings.updateField('auto_generate_soap', checked);
        }}
      />
      <span>Auto-generate SOAP after recording</span>
    </label>
    <span class="form-hint">When enabled, transcription and SOAP generation start automatically after you stop recording.</span>
  </div>

</section>

<style>
  .settings-section {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .section-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--text-primary);
    padding-bottom: 8px;
    border-bottom: 1px solid var(--border-light);
    margin-bottom: 4px;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .radio-fieldset {
    border: 0;
    padding: 0;
    margin: 0 0 0.75rem 0;
  }
  .radio-fieldset legend {
    padding: 0;
    margin-bottom: 0.25rem;
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

  .badge-value {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    background-color: color-mix(in srgb, var(--accent) 15%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
    border-radius: var(--radius-sm);
    padding: 1px 6px;
    margin-left: 4px;
  }

  .checkbox-label {
    cursor: pointer;
    user-select: none;
  }

  .checkbox-label input[type='checkbox'] {
    width: auto;
    cursor: pointer;
  }

  .radio-row {
    display: flex;
    gap: 16px;
    align-items: center;
  }

  .radio-label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    user-select: none;
    font-size: 13px;
    color: var(--text-primary);
  }

  .radio-label input[type='radio'] {
    width: auto;
    cursor: pointer;
    margin: 0;
  }

</style>
