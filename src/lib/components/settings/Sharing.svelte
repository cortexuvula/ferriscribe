<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { settings } from '../../stores/settings.svelte';
  import { syncConditionChips } from '../../api/conditions';
  import ServerWizard from './sharing/ServerWizard.svelte';
  import ServerStatus from './sharing/ServerStatus.svelte';
  import ClientPair from './sharing/ClientPair.svelte';

  type Mode = 'off' | 'server' | 'client';
  let mode: Mode = 'off';
  let sharingOn = false;
  let pairedTo: string | null = null;

  async function refresh() {
    try {
      const status = await invoke<{ enabled: boolean }>('sharing_status');
      sharingOn = !!status.enabled;
    } catch {
      sharingOn = false;
    }
    try {
      const paired = await invoke<{ label: string } | null>('paired_endpoint');
      pairedTo = paired?.label ?? null;
    } catch {
      pairedTo = null;
    }
    if (sharingOn) mode = 'server';
    else if (pairedTo) mode = 'client';
    else mode = 'off';
  }
  onMount(refresh);
</script>

<div class="sharing">
  <h2>Sharing across machines</h2>
  <p class="hint">
    Run FerriScribe's heavy AI on one office computer and connect from your
    laptop or other clinicians' machines.
  </p>

  <div class="modes">
    <label class:disabled={sharingOn}>
      <input type="radio" bind:group={mode} value="off" disabled={sharingOn} />
      Off
    </label>
    <label>
      <input type="radio" bind:group={mode} value="server" />
      This machine is the office server
    </label>
    <label class:disabled={sharingOn}>
      <input type="radio" bind:group={mode} value="client" disabled={sharingOn} />
      This machine connects to an office server
    </label>
  </div>

  {#if sharingOn}
    <p class="hint">
      Stop sharing first (in the panel below) before switching modes.
    </p>

    <label class="form-row" style="margin-top: 1rem;">
      <input
        type="checkbox"
        checked={settings.state.sync_condition_chips ?? false}
        onchange={async (e) => {
          const checked = (e.target as HTMLInputElement).checked;
          settings.updateField('sync_condition_chips', checked);
          if (checked) {
            try {
              await syncConditionChips();
            } catch (err) {
              console.error('Initial condition chip sync failed:', err);
            }
          }
        }}
      />
      <span>
        Sync known condition chips with the server
        <p class="hint">
          When enabled, your condition chip presets sync two-way between this
          machine and the server. Other clients' changes appear on reconnect.
          Off by default — each machine keeps its own list.
        </p>
      </span>
    </label>
  {/if}

  {#if mode === 'server' && !sharingOn}
    <ServerWizard ondone={refresh} />
  {:else if mode === 'server' && sharingOn}
    <ServerStatus onstopped={refresh} />
  {:else if mode === 'client'}
    <ClientPair />
  {/if}
</div>

<style>
  .sharing { display: flex; flex-direction: column; gap: 1rem; }
  .modes { display: flex; gap: 1rem; }
  .hint { color: var(--text-muted, #888); font-size: 0.8rem; margin: 4px 0 0 0; }
  label.disabled { opacity: 0.5; cursor: not-allowed; }
  .form-row { display: flex; gap: 10px; align-items: flex-start; }
</style>
