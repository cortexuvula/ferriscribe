<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { formatError } from '../../../types/errors';

  type Props = {
    ondone?: () => void;
  };
  let { ondone }: Props = $props();

  let friendlyName = $state('Clinic Server');
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function start() {
    busy = true;
    error = null;
    try {
      await invoke('start_sharing', { friendlyName });
      ondone?.();
    } catch (e) {
      error = formatError(e);
    } finally {
      busy = false;
    }
  }
</script>

<section>
  <h3>Become office server</h3>
  <ol class="steps">
    <li>Friendly name (visible to clinicians on this machine):
      <input bind:value={friendlyName} />
    </li>
    <li>FerriScribe will configure persistent Ollama, download whisper.cpp,
        start an authenticated proxy, and advertise this server on the
        local network.</li>
    <li>If LM Studio is installed, open it and click "Start Server" in its
        Local Server tab. (We don't manage LM Studio's server toggle.)</li>
    <li>Your operating system may ask permission for FerriScribe to accept
        incoming connections. Click <b>Allow</b>.</li>
  </ol>
  <button class="btn btn-primary" disabled={busy} onclick={start}>
    {busy ? 'Setting up…' : 'Start sharing'}
  </button>
  {#if error}<div class="error">{error}</div>{/if}
</section>

<style>
  .error { color: #c00; margin-top: 0.5rem; }
  .steps { padding-left: 1.2rem; }
  .btn {
    border: 1px solid var(--border, #c8c8c8);
    background: var(--surface-1, #fff);
    color: inherit;
    padding: 0.5rem 1rem;
    border-radius: 0.375rem;
    font-weight: 500;
    margin-top: 0.5rem;
    transition: background 0.12s ease, border-color 0.12s ease;
  }
  .btn:hover:not(:disabled) {
    background: var(--surface-2, #f0f0f0);
    border-color: var(--border-strong, #a0a0a0);
  }
  .btn-primary {
    background: #2563eb;
    border-color: #2563eb;
    color: white;
  }
  .btn-primary:hover:not(:disabled) {
    background: #1d4ed8;
    border-color: #1d4ed8;
  }
</style>
