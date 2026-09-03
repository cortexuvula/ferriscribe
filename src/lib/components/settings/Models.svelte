<script lang="ts">
  import { onMount } from 'svelte';
  import { settings } from '../../stores/settings.svelte';
  import { listModels, setActiveProvider, type ModelInfo } from '../../api/chat';
  import { testLmStudioConnection, testOllamaConnection, testOmlxConnection } from '../../api/settings';
  import { isPairedWithServer } from '../../api/sharing';
  import { officeServedHint, providerStartHint } from '../../utils/providerHints';
  import { formatError } from '../../types/errors';
  import ProviderServerSection from './ProviderServerSection.svelte';
  import Callout from './Callout.svelte';

  let availableModels = $state<ModelInfo[]>([]);
  let modelsLoading = $state(false);
  /** Why the model list couldn't load (provider offline / empty list) —
   * shown with a start-the-server hint so the failure is actionable. */
  let modelsError = $state('');
  /** Paired clients route providers through the office server's proxies;
   * changes which hint we show when a provider is unreachable. */
  let isPaired = $state(false);
  const modelMemory = $state<Record<string, string>>({});
  /** Request token: rapid provider switches resolve out of order — only
   *  the most recent fetch may render its list (same discipline as the
   *  recordings store's load/search). */
  let modelFetchToken = 0;

  /** Which provider server sections are expanded. Only the ACTIVE
   *  provider's section auto-opens; the others stay collapsed (all three
   *  expanded made the pane a wall of mostly-irrelevant host/port fields).
   *  Users can still open any of them manually. */
  let openSections = $state({ lmstudio: false, ollama: false, omlx: false });
  $effect(() => {
    const provider = settings.state.ai_provider;
    openSections = { lmstudio: provider === 'lmstudio', ollama: provider === 'ollama', omlx: provider === 'omlx' };
  });

  async function fetchModelsForProvider(provider: string) {
    const token = ++modelFetchToken;
    modelsLoading = true;
    modelsError = '';
    try {
      const models = await listModels(provider);
      if (token !== modelFetchToken) return []; // superseded mid-flight
      availableModels = models;
      return models;
    } catch (e) {
      if (token !== modelFetchToken) return []; // superseded mid-flight
      console.error('Failed to fetch models:', e);
      availableModels = [];
      // Surface why: the backend error carries the provider name and the
      // endpoint URL it tried (e.g. "Ollama at http://… is offline").
      modelsError = formatError(e) || 'Could not fetch the model list.';
      return [];
    } finally {
      if (token === modelFetchToken) modelsLoading = false;
    }
  }

  onMount(async () => {
    if (settings.state.ai_provider && settings.state.ai_model) {
      modelMemory[settings.state.ai_provider] = settings.state.ai_model;
    }
    isPaired = await isPairedWithServer().catch(() => false);
    try {
      await fetchModelsForProvider(settings.state.ai_provider);
    } catch (e) {
      console.error('Settings init: fetchModelsForProvider failed:', e);
    }
  });

  async function handleAiProviderChange(e: Event) {
    const newProvider = (e.target as HTMLSelectElement).value;
    const oldProvider = settings.state.ai_provider;
    if (oldProvider && settings.state.ai_model) {
      modelMemory[oldProvider] = settings.state.ai_model;
    }
    await settings.updateField('ai_provider', newProvider);
    await setActiveProvider(newProvider);
    // Fence the whole post-fetch chain on the request token: fetch already
    // discards superseded results, but the WRITES below used to run anyway —
    // a slow stale chain resolving after a newer switch saw `models = []`
    // and cleared ai_model/ocr_model that the newer chain had just set,
    // breaking every subsequent generation while the dropdown looked fine.
    const token = ++modelFetchToken;
    const models = await fetchModelsForProvider(newProvider);
    if (token !== modelFetchToken) return; // superseded mid-flight
    const remembered = modelMemory[newProvider];
    if (remembered && models.some((m) => m.id === remembered)) {
      await settings.updateField('ai_model', remembered);
    } else if (models.length > 0) {
      await settings.updateField('ai_model', models[0].id);
    } else {
      // No models on the new provider: leaving the OLD provider's model id
      // in place would send a foreign model name to every generation
      // (404 on oMLX, wrong model on Ollama). Clear it; the offline hint
      // tells the user why the dropdown is empty.
      await settings.updateField('ai_model', '');
    }
    // Same staleness class for the OCR model: an id from the old
    // provider's list isn't offered by the new one and would silently 404
    // at generation time.
    if (settings.state.ocr_model && !models.some((m) => m.id === settings.state.ocr_model)) {
      await settings.updateField('ocr_model', null);
    }
  }

  async function handleAiModelChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value;
    await settings.updateField('ai_model', value);
    modelMemory[settings.state.ai_provider] = value;
  }

  /** Live drag value — the temperature persists on release, not per tick
   *  (every oninput tick would queue a full-config IPC save). */
  let temperatureDraft = $state(settings.state.temperature);

  async function handleTemperatureCommit() {
    await settings.updateField('temperature', temperatureDraft);
  }
</script>

<section class="settings-section">
  <h3 class="section-title">AI Models</h3>

  <div class="form-group">
    <label for="ai-provider" class="form-label">AI Provider</label>
    <select
      id="ai-provider"
      value={settings.state.ai_provider}
      onchange={handleAiProviderChange}
    >
      <option value="lmstudio">LM Studio</option>
      <option value="ollama">Ollama</option>
      <option value="omlx">oMLX</option>
    </select>
  </div>

  <div class="form-group">
    <label for="ai-model" class="form-label">Model</label>
    <div class="model-select-row">
      <select
        id="ai-model"
        value={settings.state.ai_model}
        onchange={handleAiModelChange}
        disabled={modelsLoading}
      >
        {#if modelsLoading}
          <option value="">Loading models…</option>
        {:else if availableModels.length === 0}
          <option value="">{modelsError ? 'Could not load models' : 'No models available'}</option>
        {:else}
          {#each availableModels as model (model.id)}
            <option value={model.id}>{model.name}</option>
          {/each}
        {/if}
      </select>
      <button
        class="btn-refresh"
        onclick={() => fetchModelsForProvider(settings.state.ai_provider)}
        disabled={modelsLoading}
        title="Refresh model list"
      >
        {modelsLoading ? '…' : '↻'}
      </button>
    </div>
    {#if modelsError && !modelsLoading}
      <Callout kind="warning">
        <p class="model-list-error-message">{modelsError}</p>
        <p class="model-list-error-hint">
          {isPaired
            ? officeServedHint(settings.state.ai_provider)
            : providerStartHint(settings.state.ai_provider)}
        </p>
      </Callout>
    {/if}
  </div>

  <div class="form-group">
    <label for="ocr-model" class="form-label">OCR / Vision Model</label>
    <div class="model-select-row">
      <select
        id="ocr-model"
        value={settings.state.ocr_model ?? ''}
        onchange={async (e) => {
          const val = (e.currentTarget as HTMLSelectElement).value;
          await settings.updateField('ocr_model', val || null);
        }}
      >
        <option value="">(use generation model)</option>
        {#each availableModels as m (m.id)}
          <option value={m.id}>{m.name}</option>
        {/each}
      </select>
    </div>
    <p class="form-hint">
      Vision model for extracting text from dropped documents (e.g. glm-ocr).
      If not set, the generation model is used.
    </p>
  </div>

  <div class="form-group">
    <label for="temperature" class="form-label">
      Temperature
      <span class="value-display">{temperatureDraft.toFixed(1)}</span>
    </label>
    <input
      id="temperature"
      type="range"
      min="0"
      max="2"
      step="0.1"
      value={temperatureDraft}
      oninput={(e: Event) => {
        temperatureDraft = parseFloat((e.target as HTMLInputElement).value);
      }}
      onchange={handleTemperatureCommit}
      class="range-input"
    />
    <div class="range-labels">
      <span>0 (Precise)</span>
      <span>2 (Creative)</span>
    </div>
  </div>

  <!-- LM Studio Server -->
  <details class="provider-section" bind:open={openSections.lmstudio}>
    <summary>LM Studio Server</summary>
    <ProviderServerSection
      idPrefix="lmstudio"
      title="LM Studio Server"
      hideTitle
      hostField="lmstudio_host"
      portField="lmstudio_port"
      defaultPort={1234}
      apiKeySlot="lmstudio_api_key"
      testConnection={testLmStudioConnection}
      thinkingField="lmstudio_disable_thinking"
    >
    {#snippet hint()}
      Configure the LM Studio server address. Use <code>localhost</code> if LM Studio runs on this
      machine, or enter a remote IP for a network server.
    {/snippet}
      {#snippet thinkingHint()}
      Skips the minutes-long reasoning/"thinking" phase on models like Qwen3 before they write a
      SOAP note. LM Studio ignores API thinking parameters, so FerriScribe injects a pre-closed
      think block instead. For a fix that covers every app, edit the model's Prompt Template in LM
      Studio (Model Settings → Prompt Template, add
      <code>{'{%- set enable_thinking = false %}'}</code>).
      {/snippet}
  </ProviderServerSection>
  </details>

  <!-- Ollama Server -->
  <details class="provider-section" bind:open={openSections.ollama}>
    <summary>Ollama Server</summary>
    <ProviderServerSection
      idPrefix="ollama"
      title="Ollama Server"
      hideTitle
      hostField="ollama_host"
      portField="ollama_port"
      defaultPort={11434}
      apiKeySlot="ollama_api_key"
      testConnection={testOllamaConnection}
      thinkingField="ollama_disable_thinking"
    >
      {#snippet hint()}
      Configure the Ollama server address. Use <code>localhost</code> if Ollama runs on this
      machine, or enter a remote IP / Tailscale hostname for a network server.
      {/snippet}
      {#snippet thinkingHint()}
      Skips the minutes-long reasoning/"thinking" phase on models like Qwen3 before they write a
      SOAP note. Sends <code>reasoning_effort: "none"</code> to Ollama's OpenAI-compatible endpoint.
      {/snippet}
  </ProviderServerSection>
  </details>

  <!-- oMLX Server -->
  <details class="provider-section" bind:open={openSections.omlx}>
    <summary>oMLX Server</summary>
    <ProviderServerSection
      idPrefix="omlx"
      title="oMLX Server"
      hideTitle
      hostField="omlx_host"
      portField="omlx_port"
      defaultPort={8000}
      apiKeySlot="omlx_api_key"
      testConnection={testOmlxConnection}
      thinkingField="omlx_disable_thinking"
    >
      {#snippet hint()}
      Configure the oMLX server address (MLX inference for Apple Silicon). Use
      <code>localhost</code> if oMLX runs on this machine, or enter a remote IP for a network
      server.
      {/snippet}
      {#snippet thinkingHint()}
      Skips the minutes-long reasoning/"thinking" phase on models like Qwen3 before they write a
      SOAP note. oMLX ignores API thinking parameters, so FerriScribe injects a pre-closed think
      block instead.
      {/snippet}
  </ProviderServerSection>
  </details>
</section>

<style>
  .provider-section {
    border-top: 1px solid var(--border);
    margin-top: 20px;
  }

  .provider-section summary {
    cursor: pointer;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
    padding: 4px 0 8px;
    user-select: none;
  }

  .provider-section[open] summary {
    padding-bottom: 0;
  }

  .model-select-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .model-select-row select {
    flex: 1;
  }

  .btn-refresh {
    flex-shrink: 0;
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 16px;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease;
  }

  .btn-refresh:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-refresh:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .value-display {
    font-size: 12px;
    font-weight: 600;
    color: var(--accent);
    background-color: var(--accent-light);
    padding: 1px 6px;
    border-radius: var(--radius-sm);
  }

  .range-input {
    width: 100%;
    padding: 0;
    border: none;
    background: none;
    box-shadow: none;
    accent-color: var(--accent);
    cursor: pointer;
    height: 20px;
  }

  .range-input:focus {
    box-shadow: none;
    border-color: transparent;
  }

  .range-labels {
    display: flex;
    justify-content: space-between;
    font-size: 11px;
    color: var(--text-muted);
  }

  .model-list-error-message {
    margin: 0;
  }

  .model-list-error-hint {
    margin: 4px 0 0;
    font-weight: 600;
  }

  .form-hint {
    font-size: 11px;
    color: var(--text-muted);
    margin: 4px 0 0;
    line-height: 1.5;
  }
</style>
