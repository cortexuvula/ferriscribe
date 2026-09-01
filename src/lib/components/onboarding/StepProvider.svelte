<script lang="ts">
  import { settings } from '../../stores/settings.svelte';
  import { testLmStudioConnection, testOllamaConnection, testOmlxConnection } from '../../api/settings';
  import { reinitProviders } from '../../api/chat';
  import { formatError } from '../../types/errors';

  interface Props { onNext: () => void; onSkip: () => void; }
  const { onNext, onSkip }: Props = $props();

  // Local copies seeded from settings so the user can edit before saving.
  let provider = $state(settings.state.ai_provider);
  let lmHost = $state(settings.state.lmstudio_host);
  let lmPort = $state(settings.state.lmstudio_port);
  let ollamaHost = $state(settings.state.ollama_host);
  let ollamaPort = $state(settings.state.ollama_port);
  let omlxHost = $state(settings.state.omlx_host);
  let omlxPort = $state(settings.state.omlx_port);
  let testStatus = $state<'idle' | 'testing' | 'success' | 'error'>('idle');
  let testError = $state<string | null>(null);

  async function testConnection() {
    testStatus = 'testing';
    testError = null;
    try {
      if (provider === 'lmstudio') {
        await testLmStudioConnection(lmHost, lmPort, undefined);
      } else if (provider === 'omlx') {
        await testOmlxConnection(omlxHost, omlxPort, undefined);
      } else {
        await testOllamaConnection(ollamaHost, ollamaPort, undefined);
      }
      testStatus = 'success';
    } catch (e) {
      testStatus = 'error';
      testError = formatError(e);
    }
  }

  async function saveAndNext() {
    await settings.updateField('ai_provider', provider);
    await settings.updateField('lmstudio_host', lmHost);
    await settings.updateField('lmstudio_port', lmPort);
    await settings.updateField('ollama_host', ollamaHost);
    await settings.updateField('ollama_port', ollamaPort);
    await settings.updateField('omlx_host', omlxHost);
    await settings.updateField('omlx_port', omlxPort);
    try { await reinitProviders(); } catch (e) { console.error('reinit failed', e); }
    onNext();
  }
</script>

<h2>Set up your AI provider</h2>
<p class="hint">FerriScribe needs a local AI server (Ollama, LM Studio, or oMLX) running to generate clinical notes. Pick the one you have installed and test the connection.</p>

<div class="field">
  <label for="ob-provider">Provider</label>
  <select id="ob-provider" bind:value={provider}>
    <option value="lmstudio">LM Studio</option>
    <option value="ollama">Ollama</option>
    <option value="omlx">oMLX</option>
  </select>
</div>

{#if provider === 'lmstudio'}
  <div class="row">
    <div class="field grow">
      <label for="ob-lm-host">Host</label>
      <input id="ob-lm-host" type="text" bind:value={lmHost} placeholder="localhost" />
    </div>
    <div class="field">
      <label for="ob-lm-port">Port</label>
      <input id="ob-lm-port" type="number" bind:value={lmPort} placeholder="1234" />
    </div>
  </div>
{:else if provider === 'omlx'}
  <div class="row">
    <div class="field grow">
      <label for="ob-omlx-host">Host</label>
      <input id="ob-omlx-host" type="text" bind:value={omlxHost} placeholder="localhost" />
    </div>
    <div class="field">
      <label for="ob-omlx-port">Port</label>
      <input id="ob-omlx-port" type="number" bind:value={omlxPort} placeholder="8000" />
    </div>
  </div>
{:else}
  <div class="row">
    <div class="field grow">
      <label for="ob-ollama-host">Host</label>
      <input id="ob-ollama-host" type="text" bind:value={ollamaHost} placeholder="localhost" />
    </div>
    <div class="field">
      <label for="ob-ollama-port">Port</label>
      <input id="ob-ollama-port" type="number" bind:value={ollamaPort} placeholder="11434" />
    </div>
  </div>
{/if}

<div class="test-row">
  <button class="btn-secondary" onclick={testConnection} disabled={testStatus === 'testing'}>
    {testStatus === 'testing' ? 'Testing…' : 'Test connection'}
  </button>
  {#if testStatus === 'success'}
    <span class="test-ok">✓ Connected</span>
  {:else if testStatus === 'error'}
    <span class="test-fail" title={testError ?? ''}>✗ Not reachable</span>
  {/if}
</div>
{#if testStatus === 'error' && testError}
  <p class="error-detail">{testError}</p>
{/if}

<div class="actions">
  <button class="btn-skip" onclick={onSkip}>Skip for now</button>
  <button class="btn-primary" onclick={saveAndNext}>Next →</button>
</div>

<style>
  h2 { font-size: 18px; font-weight: 600; margin: 0 0 4px; color: var(--text-primary); }
  .hint { font-size: 13px; color: var(--text-muted); margin: 0 0 16px; line-height: 1.5; }
  .field { display: flex; flex-direction: column; gap: 4px; margin-bottom: 12px; }
  .field.grow { flex: 1; }
  .row { display: flex; gap: 10px; }
  label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; color: var(--text-secondary); }
  input, select {
    height: 32px; padding: 0 10px; font-size: 13px; color: var(--text-primary);
    background-color: var(--bg-primary, #1a1a1a); border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px); box-sizing: border-box;
  }
  input:focus, select:focus { outline: none; border-color: var(--accent, #3b82f6); }
  .test-row { display: flex; align-items: center; gap: 10px; margin-bottom: 8px; }
  .test-ok { font-size: 13px; color: var(--success, #22c55e); font-weight: 500; }
  .test-fail { font-size: 13px; color: var(--danger, #ef4444); font-weight: 500; }
  .error-detail { font-size: 12px; color: var(--danger, #ef4444); margin: 0 0 8px; }
  .actions { display: flex; justify-content: space-between; align-items: center; margin-top: 16px; }
  .btn-secondary {
    padding: 8px 16px; font-size: 13px; font-weight: 500; color: var(--text-primary);
    background-color: transparent; border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-secondary:hover:not(:disabled) { border-color: var(--accent, #3b82f6); }
  .btn-secondary:disabled { opacity: 0.6; cursor: not-allowed; }
  .btn-primary {
    padding: 8px 20px; font-size: 13px; font-weight: 600; color: white;
    background-color: var(--accent, #3b82f6); border: none;
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-primary:hover { background-color: var(--accent-hover, #2563eb); }
  .btn-skip { padding: 6px 10px; font-size: 12px; color: var(--text-muted); background: none; border: none; cursor: pointer; text-decoration: underline; }
</style>
