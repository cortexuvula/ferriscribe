<script lang="ts">
  import { onMount } from 'svelte';
  import { settings } from '../../stores/settings.svelte';
  import { testSttRemoteConnection, setApiKey, getApiKey } from '../../api/settings';
  import { reinitProviders } from '../../api/chat';
  import { classifyEndpoint, isLocalOrAllowed } from '../../utils/endpointPolicy';
  import { useTestConnection } from '../../composables/useTestConnection.svelte';
  import Callout from './Callout.svelte';

  const sttOk = $derived(isLocalOrAllowed(settings.state.stt_remote_host ?? '', settings.state.allow_public_endpoint));
  const sttKind = $derived(classifyEndpoint(settings.state.stt_remote_host ?? ''));

  const test = useTestConnection();
  let sttRemoteApiKey = $state('');

  onMount(() => {
    getApiKey('stt_remote_api_key').then((key) => {
      if (key) sttRemoteApiKey = key;
    }).catch(() => { /* ignore — keychain miss is fine */ });
  });
</script>

<div class="form-group">
  <label for="stt-remote-host" class="form-label">Host</label>
  <input
    id="stt-remote-host"
    type="text"
    placeholder="computer-a.tailnet.ts.net"
    value={settings.state.stt_remote_host ?? ''}
    onchange={async (e) => {
      await settings.updateField('stt_remote_host', (e.target as HTMLInputElement).value);
      test.reset();
      await reinitProviders();
    }}
    class="text-input"
  />
  {#if !sttOk}
    <Callout kind="warning">
      ⚠ This is a public-internet address ({sttKind}). PHI may leave your device.
      Enable <em>Allow public endpoints</em> in Advanced settings to use this anyway.
    </Callout>
  {/if}
</div>
<div class="form-group">
  <label for="stt-remote-port" class="form-label">Port</label>
  <input
    id="stt-remote-port"
    type="number"
    value={settings.state.stt_remote_port ?? 8080}
    min="1"
    max="65535"
    onchange={async (e) => {
      const value = parseInt((e.target as HTMLInputElement).value, 10);
      if (value >= 1 && value <= 65535) {
        await settings.updateField('stt_remote_port', value);
        test.reset();
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
    value={settings.state.stt_remote_model ?? ''}
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
    onclick={() =>
      test.run(async () => {
        await setApiKey('stt_remote_api_key', sttRemoteApiKey);
        await reinitProviders();
        return 'Key saved.';
      })}
  >Save key</button>
  <span class="form-hint">Leave blank and click Save to clear.</span>
</div>
<div class="form-group">
  <button
    class="btn-test-connection"
    type="button"
    disabled={test.status === 'testing'}
    onclick={() =>
      test.run(() =>
        testSttRemoteConnection(
          settings.state.stt_remote_host || 'localhost',
          settings.state.stt_remote_port || 8080,
          sttRemoteApiKey || null,
        ),
      )}
  >{test.status === 'testing' ? 'Testing…' : 'Test Connection'}</button>
  {#if test.status === 'success'}
    <span class="test-result test-success">✓ {test.message}</span>
  {:else if test.status === 'error'}
    <span class="test-result test-error">✗ {test.message}</span>
  {/if}
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

  .form-hint code {
    font-size: 10px;
    background-color: var(--bg-tertiary, #374151);
    padding: 1px 4px;
    border-radius: 3px;
  }

  .port-input {
    max-width: 120px;
  }

  .btn-test-connection {
    align-self: flex-start;
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

  .text-input {
    padding: 8px 10px;
    font-size: 13px;
    background-color: var(--bg-input);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
</style>