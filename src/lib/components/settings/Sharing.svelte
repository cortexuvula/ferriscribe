<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
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
  .hint { color: var(--text-muted, #888); }
  label.disabled { opacity: 0.5; cursor: not-allowed; }
</style>
