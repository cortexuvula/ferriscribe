<script lang="ts">
  import { onMount } from 'svelte';
  import { settings } from '../../stores/settings.svelte';
  import { listModels, setActiveProvider, reinitProviders, type ModelInfo } from '../../api/chat';
  import { testLmStudioConnection, testOllamaConnection, getApiKey } from '../../api/settings';
  import { formatError } from '../../types/errors';
  import { classifyEndpoint, isLocalOrAllowed } from '../../utils/endpointPolicy';

  const ollamaOk = $derived(isLocalOrAllowed(settings.state.ollama_host ?? '', settings.state.allow_public_endpoint));
  const ollamaKind = $derived(classifyEndpoint(settings.state.ollama_host ?? ''));
  const lmstudioOk = $derived(isLocalOrAllowed(settings.state.lmstudio_host ?? '', settings.state.allow_public_endpoint));
  const lmstudioKind = $derived(classifyEndpoint(settings.state.lmstudio_host ?? ''));

  let availableModels = $state<ModelInfo[]>([]);
  let modelsLoading = $state(false);
  let modelMemory = $state<Record<string, string>>({});

  let lmstudioTestStatus = $state<'idle' | 'testing' | 'success' | 'error'>('idle');
  let lmstudioTestMessage = $state('');
  let ollamaTestStatus = $state<'idle' | 'testing' | 'success' | 'error'>('idle');
  let ollamaTestMessage = $state('');

  async function fetchModelsForProvider(provider: string) {
    modelsLoading = true;
    try {
      availableModels = await listModels(provider);
    } catch (e) {
      console.error('Failed to fetch models:', e);
      availableModels = [];
    } finally {
      modelsLoading = false;
    }
  }

  onMount(async () => {
    if (settings.state.ai_provider && settings.state.ai_model) {
      modelMemory[settings.state.ai_provider] = settings.state.ai_model;
    }
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
    await fetchModelsForProvider(newProvider);
    const remembered = modelMemory[newProvider];
    if (remembered && availableModels.some((m) => m.id === remembered)) {
      await settings.updateField('ai_model', remembered);
    } else if (availableModels.length > 0) {
      await settings.updateField('ai_model', availableModels[0].id);
    }
  }

  async function handleAiModelChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value;
    await settings.updateField('ai_model', value);
    modelMemory[settings.state.ai_provider] = value;
  }

  async function handleTemperatureChange(e: Event) {
    const value = parseFloat((e.target as HTMLInputElement).value);
    await settings.updateField('temperature', value);
  }

  async function handleLmStudioHostChange(e: Event) {
    const value = (e.target as HTMLInputElement).value;
    await settings.updateField('lmstudio_host', value);
    lmstudioTestStatus = 'idle';
    lmstudioTestMessage = '';
    await reinitProviders();
  }

  async function handleLmStudioPortChange(e: Event) {
    const value = parseInt((e.target as HTMLInputElement).value, 10);
    if (value >= 1 && value <= 65535) {
      await settings.updateField('lmstudio_port', value);
      lmstudioTestStatus = 'idle';
      lmstudioTestMessage = '';
      await reinitProviders();
    }
  }

  async function handleTestLmStudioConnection() {
    lmstudioTestStatus = 'testing';
    lmstudioTestMessage = '';
    try {
      const host = settings.state.lmstudio_host || 'localhost';
      const port = settings.state.lmstudio_port || 1234;
      let apiKey: string | null = null;
      try {
        apiKey = await getApiKey('lmstudio_api_key');
      } catch {
        // Keychain unavailable — try without auth.
      }
      const msg = await testLmStudioConnection(host, port, apiKey);
      lmstudioTestStatus = 'success';
      lmstudioTestMessage = msg;
    } catch (err) {
      lmstudioTestStatus = 'error';
      lmstudioTestMessage = formatError(err) || 'Connection failed';
    }
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
          <option value="">No models available</option>
        {:else}
          {#each availableModels as model}
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
  </div>

  <div class="form-group">
    <label for="temperature" class="form-label">
      Temperature
      <span class="value-display">{settings.state.temperature.toFixed(1)}</span>
    </label>
    <input
      id="temperature"
      type="range"
      min="0"
      max="2"
      step="0.1"
      value={settings.state.temperature}
      oninput={handleTemperatureChange}
      class="range-input"
    />
    <div class="range-labels">
      <span>0 (Precise)</span>
      <span>2 (Creative)</span>
    </div>
  </div>

  <!-- LM Studio Server -->
  <div class="form-group-divider"></div>
  <h4 class="subsection-title">LM Studio Server</h4>
  <p class="subsection-hint">
    Configure the LM Studio server address. Use <code>localhost</code> if LM Studio runs on this machine, or enter a remote IP for a network server.
  </p>

  <div class="form-group">
    <label for="lmstudio-host" class="form-label">Host</label>
    <input
      id="lmstudio-host"
      type="text"
      value={settings.state.lmstudio_host}
      placeholder="localhost"
      onchange={handleLmStudioHostChange}
      class="text-input"
    />
    {#if !lmstudioOk}
      <div class="endpoint-warning" role="alert">
        ⚠ This is a public-internet address ({lmstudioKind}). PHI may leave your device.
        Enable <em>Allow public endpoints</em> in Advanced settings to use this anyway.
      </div>
    {/if}
  </div>

  <div class="form-group">
    <label for="lmstudio-port" class="form-label">Port</label>
    <input
      id="lmstudio-port"
      type="number"
      value={settings.state.lmstudio_port}
      placeholder="1234"
      min="1"
      max="65535"
      onchange={handleLmStudioPortChange}
      class="text-input port-input"
    />
  </div>

  <div class="form-group">
    <button
      class="btn-test-connection"
      onclick={handleTestLmStudioConnection}
      disabled={lmstudioTestStatus === 'testing'}
    >
      {#if lmstudioTestStatus === 'testing'}
        Testing…
      {:else}
        Test Connection
      {/if}
    </button>
    {#if lmstudioTestStatus === 'success'}
      <span class="test-result test-success">✓ {lmstudioTestMessage}</span>
    {:else if lmstudioTestStatus === 'error'}
      <span class="test-result test-error">✗ {lmstudioTestMessage}</span>
    {/if}
  </div>

  <!-- Ollama Server -->
  <div class="form-group-divider"></div>
  <h4 class="subsection-title">Ollama Server</h4>
  <p class="subsection-hint">
    Configure the Ollama server address. Use <code>localhost</code> if Ollama runs on this machine, or enter a remote IP / Tailscale hostname for a network server.
  </p>

  <div class="form-group">
    <label for="ollama-host" class="form-label">Host</label>
    <input
      id="ollama-host"
      type="text"
      value={settings.state.ollama_host ?? ''}
      placeholder="localhost"
      onchange={async (e) => {
        await settings.updateField('ollama_host', (e.target as HTMLInputElement).value);
        ollamaTestStatus = 'idle';
        ollamaTestMessage = '';
        await reinitProviders();
      }}
      class="text-input"
    />
    {#if !ollamaOk}
      <div class="endpoint-warning" role="alert">
        ⚠ This is a public-internet address ({ollamaKind}). PHI may leave your device.
        Enable <em>Allow public endpoints</em> in Advanced settings to use this anyway.
      </div>
    {/if}
  </div>

  <div class="form-group">
    <label for="ollama-port" class="form-label">Port</label>
    <input
      id="ollama-port"
      type="number"
      value={settings.state.ollama_port ?? 11434}
      placeholder="11434"
      min="1"
      max="65535"
      onchange={async (e) => {
        const value = parseInt((e.target as HTMLInputElement).value, 10);
        if (value >= 1 && value <= 65535) {
          await settings.updateField('ollama_port', value);
          ollamaTestStatus = 'idle';
          ollamaTestMessage = '';
          await reinitProviders();
        }
      }}
      class="text-input port-input"
    />
  </div>

  <div class="form-group">
    <button
      class="btn-test-connection"
      disabled={ollamaTestStatus === 'testing'}
      onclick={async () => {
        ollamaTestStatus = 'testing';
        ollamaTestMessage = '';
        try {
          let apiKey: string | null = null;
          try {
            apiKey = await getApiKey('ollama_api_key');
          } catch {
            // Keychain unavailable — try without auth.
          }
          const msg = await testOllamaConnection(
            settings.state.ollama_host || 'localhost',
            settings.state.ollama_port || 11434,
            apiKey,
          );
          ollamaTestStatus = 'success';
          ollamaTestMessage = msg;
        } catch (err) {
          ollamaTestStatus = 'error';
          ollamaTestMessage = formatError(err) || 'Connection failed';
        }
      }}
    >
      {#if ollamaTestStatus === 'testing'}
        Testing…
      {:else}
        Test Connection
      {/if}
    </button>
    {#if ollamaTestStatus === 'success'}
      <span class="test-result test-success">✓ {ollamaTestMessage}</span>
    {:else if ollamaTestStatus === 'error'}
      <span class="test-result test-error">✗ {ollamaTestMessage}</span>
    {/if}
  </div>
</section>

<style>
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

  .form-group-divider {
    border-top: 1px solid var(--border);
    margin: 20px 0 16px;
  }

  .subsection-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0 0 4px;
  }

  .subsection-hint {
    font-size: 12px;
    color: var(--text-muted);
    margin: 0 0 12px;
    line-height: 1.5;
  }

  .subsection-hint code {
    font-size: 11px;
    background-color: var(--bg-tertiary, #374151);
    padding: 1px 5px;
    border-radius: 3px;
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
