<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { settings } from '../../stores/settings';
  import { listAudioDevices } from '../../api/audio';
  import { listWhisperModels, listPyannoteModels, downloadModel, deleteModel, type ModelInfo as WhisperModelInfo } from '../../api/models';
  import { testSttRemoteConnection, setApiKey, getApiKey } from '../../api/settings';
  import { reinitProviders } from '../../api/chat';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import type { AudioDevice } from '../../types';
  import AudioInputSection from './AudioInputSection.svelte';
  import WhisperLocalSection from './WhisperLocalSection.svelte';
  import { toasts } from '../../stores/toasts';
  import { formatError } from '../../types/errors';
  import { classifyEndpoint, isLocalOrAllowed } from '../../utils/endpointPolicy';

  const sttOk = $derived(isLocalOrAllowed($settings.stt_remote_host ?? '', $settings.allow_public_endpoint));
  const sttKind = $derived(classifyEndpoint($settings.stt_remote_host ?? ''));

  let audioDevices = $state<AudioDevice[]>([]);
  let devicesLoading = $state(false);

  let whisperModels = $state<WhisperModelInfo[]>([]);
  let pyannoteModels = $state<WhisperModelInfo[]>([]);
  let modelsRefreshing = $state(false);
  let downloadingModel = $state<string | null>(null);
  let downloadProgress = $state<Record<string, { downloaded: number; total: number }>>({});
  let sttMode = $state<'local' | 'remote'>(($settings.stt_mode as 'local' | 'remote') ?? 'local');
  let sttRemoteTestStatus = $state<'idle' | 'testing' | 'success' | 'error'>('idle');
  let sttRemoteTestMessage = $state('');
  let sttRemoteApiKey = $state('');
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

    // Load the persisted STT remote API key so the password field reflects it.
    getApiKey('stt_remote_api_key').then((key) => {
      if (key) sttRemoteApiKey = key;
    }).catch(() => { /* ignore — keychain miss is fine */ });

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
    <div class="form-group">
      <label for="stt-remote-host" class="form-label">Host</label>
      <input
        id="stt-remote-host"
        type="text"
        placeholder="computer-a.tailnet.ts.net"
        value={$settings.stt_remote_host ?? ''}
        onchange={async (e) => {
          await settings.updateField('stt_remote_host', (e.target as HTMLInputElement).value);
          sttRemoteTestStatus = 'idle';
          sttRemoteTestMessage = '';
          await reinitProviders();
        }}
        class="text-input"
      />
      {#if !sttOk}
        <div class="endpoint-warning" role="alert">
          ⚠ This is a public-internet address ({sttKind}). PHI may leave your device.
          Enable <em>Allow public endpoints</em> in Advanced settings to use this anyway.
        </div>
      {/if}
    </div>
    <div class="form-group">
      <label for="stt-remote-port" class="form-label">Port</label>
      <input
        id="stt-remote-port"
        type="number"
        value={$settings.stt_remote_port ?? 8080}
        min="1"
        max="65535"
        onchange={async (e) => {
          const value = parseInt((e.target as HTMLInputElement).value, 10);
          if (value >= 1 && value <= 65535) {
            await settings.updateField('stt_remote_port', value);
            sttRemoteTestStatus = 'idle';
            sttRemoteTestMessage = '';
            await reinitProviders();
          }
        }}
        class="text-input port-input"
      />
    </div>
    <div class="form-group">
      <label for="stt-remote-model" class="form-label">Model</label>
      <input
        id="stt-remote-model"
        type="text"
        value={$settings.stt_remote_model ?? ''}
        onchange={async (e) => {
          await settings.updateField('stt_remote_model', (e.target as HTMLInputElement).value);
          await reinitProviders();
        }}
        class="text-input"
      />
      <span class="form-hint">Model name as served by your Whisper server (e.g. <code>whisper-1</code>).</span>
    </div>
    <div class="form-group">
      <label for="stt-remote-key" class="form-label">API key (optional)</label>
      <input
        id="stt-remote-key"
        type="password"
        bind:value={sttRemoteApiKey}
        class="text-input"
      />
      <button
        class="btn-test-connection"
        type="button"
        onclick={async () => {
          try {
            await setApiKey('stt_remote_api_key', sttRemoteApiKey);
            sttRemoteTestMessage = 'Key saved.';
            sttRemoteTestStatus = 'success';
            await reinitProviders();
          } catch (err) {
            sttRemoteTestStatus = 'error';
            sttRemoteTestMessage = `Failed to save key: ${err}`;
          }
        }}
      >Save key</button>
      <span class="form-hint">Leave blank and click Save to clear.</span>
    </div>
    <div class="form-group">
      <button
        class="btn-test-connection"
        type="button"
        disabled={sttRemoteTestStatus === 'testing'}
        onclick={async () => {
          sttRemoteTestStatus = 'testing';
          sttRemoteTestMessage = '';
          try {
            const msg = await testSttRemoteConnection(
              $settings.stt_remote_host || 'localhost',
              $settings.stt_remote_port || 8080,
              sttRemoteApiKey || null,
            );
            sttRemoteTestStatus = 'success';
            sttRemoteTestMessage = msg;
          } catch (err) {
            sttRemoteTestStatus = 'error';
            sttRemoteTestMessage = formatError(err) || 'Connection failed';
          }
        }}
      >{sttRemoteTestStatus === 'testing' ? 'Testing…' : 'Test Connection'}</button>
      {#if sttRemoteTestStatus === 'success'}
        <span class="test-result test-success">✓ {sttRemoteTestMessage}</span>
      {:else if sttRemoteTestStatus === 'error'}
        <span class="test-result test-error">✗ {sttRemoteTestMessage}</span>
      {/if}
    </div>
  {/if}

  <p class="form-hint">Diarization runs on this machine regardless of STT mode — pyannote models below are required for speaker labels.</p>

  <div class="form-group">
    <span class="form-label">Diarization Models (Speaker Identification)</span>
    <span class="form-hint">Both models are required for speaker diarization. Without them, transcripts will not have speaker labels.</span>
    <div class="model-list">
      {#each pyannoteModels as model}
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
                onclick={() => handleDeleteModel(model.id)}
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
                onclick={() => handleDownloadModel(model.id)}
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

  <div class="form-group">
    <label for="sample-rate" class="form-label">Sample Rate</label>
    <select
      id="sample-rate"
      value={$settings.sample_rate}
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
        checked={$settings.auto_generate_soap}
        onchange={(e: Event) => {
          const checked = (e.target as HTMLInputElement).checked;
          settings.updateField('auto_generate_soap', checked);
        }}
      />
      <span>Auto-generate SOAP after recording</span>
    </label>
    <span class="form-hint">When enabled, transcription and SOAP generation start automatically after you stop recording.</span>
  </div>

  <div class="form-group">
    <label class="form-label checkbox-label">
      <input
        type="checkbox"
        checked={$settings.capture_for_training ?? false}
        onchange={(e: Event) => {
          const checked = (e.target as HTMLInputElement).checked;
          settings.updateField('capture_for_training', checked);
        }}
      />
      <span>Capture generations for training corpus</span>
    </label>
    <span class="form-hint">
      When enabled, the app records every SOAP generation and your edited final version into a
      local-device pool (encrypted whenever your database is encrypted). Useful for fine-tuning a
      model on your own dictation style later. Data stays on this device — nothing is sent anywhere.
    </span>
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

  .port-input {
    max-width: 120px;
  }

  .btn-test-connection {
    padding: 6px 14px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease, border-color 0.15s ease;
  }

  .btn-test-connection:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .btn-test-connection:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .test-result {
    font-size: 13px;
    margin-left: 10px;
  }

  .test-success {
    color: #22c55e;
  }

  .test-error {
    color: var(--danger, #ef4444);
  }

  .endpoint-warning {
    color: #b45309;
    background: #fef3c7;
    border: 1px solid #fbbf24;
    border-radius: 4px;
    padding: 6px 10px;
    margin-top: 4px;
    font-size: 0.85rem;
  }
</style>
