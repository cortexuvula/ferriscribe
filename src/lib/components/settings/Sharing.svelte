<script lang="ts">
  import SharingModes, { type Mode } from './sharing/SharingModes.svelte';
  import ConditionChipSync from './sharing/ConditionChipSync.svelte';
  import ServerWizard from './sharing/ServerWizard.svelte';
  import ServerStatus from './sharing/ServerStatus.svelte';
  import ClientPair from './sharing/ClientPair.svelte';

  // SharingModes owns the canonical mode/sharingOn/pairedTo state and its
  // refresh() re-derivation; the orchestrator mirrors the values it needs to
  // route ServerWizard/ServerStatus/ClientPair and to gate ConditionChipSync.
  let mode = $state<Mode>('off');
  let sharingOn = $state(false);
  let pairedTo = $state<string | null>(null);
  let sharingModes: SharingModes;
</script>

<div class="sharing">
  <h2>Sharing across machines</h2>
  <p class="hint">
    Run FerriScribe's heavy AI on one office computer and connect from your
    laptop or other clinicians' machines.
  </p>

  <SharingModes
    bind:this={sharingModes}
    onModeChange={(m) => (mode = m)}
    onStatusChange={(on, paired) => {
      sharingOn = on;
      pairedTo = paired;
    }}
  />

  <ConditionChipSync visible={sharingOn || !!pairedTo} />

  {#if mode === 'server' && !sharingOn}
    <ServerWizard ondone={() => sharingModes.refresh()} />
  {:else if mode === 'server' && sharingOn}
    <ServerStatus onstopped={() => sharingModes.refresh()} />
  {:else if mode === 'client'}
    <ClientPair />
  {/if}
</div>

<style>
  .sharing { display: flex; flex-direction: column; gap: 1rem; }
  .hint { color: var(--text-muted, #888); font-size: 0.8rem; margin: 4px 0 0 0; }
</style>
