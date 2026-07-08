<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';

  export type Mode = 'off' | 'server' | 'client';

  type Props = {
    onModeChange?: (mode: Mode) => void;
    onStatusChange?: (sharingOn: boolean, pairedTo: string | null) => void;
  };
  let { onModeChange, onStatusChange }: Props = $props();

  let mode = $state<Mode>('off');
  let sharingOn = $state(false);
  let pairedTo = $state<string | null>(null);

  // Re-derive `mode` from the live sharing/pairing status. Called on mount
  // and whenever a sub-component reports a state change (ServerWizard finishes
  // setup → sharing turns on; ServerStatus stops → sharing turns off). The
  // user's manual radio selection is honoured only while sharing is off;
  // once sharing is on the mode is locked to 'server' until stopped.
  async function refresh() {
    let nextOn: boolean;
    try {
      const status = await invoke<{ enabled: boolean }>('sharing_status');
      nextOn = !!status.enabled;
    } catch {
      nextOn = false;
    }
    let nextPaired: string | null;
    try {
      const paired = await invoke<{ label: string } | null>('paired_endpoint');
      nextPaired = paired?.label ?? null;
    } catch {
      nextPaired = null;
    }

    const prevOn = sharingOn;
    const prevPaired = pairedTo;
    const prevMode = mode;

    sharingOn = nextOn;
    pairedTo = nextPaired;
    if (nextOn) mode = 'server';
    else if (nextPaired) mode = 'client';
    else mode = 'off';

    if (mode !== prevMode) onModeChange?.(mode);
    if (nextOn !== prevOn || nextPaired !== prevPaired) {
      onStatusChange?.(nextOn, nextPaired);
    }
  }

  onMount(refresh);

  function selectMode(next: Mode) {
    if (sharingOn) return;
    mode = next;
    onModeChange?.(next);
  }

  // Exposed so the parent orchestrator can refresh after ServerWizard/ServerStatus
  // report a lifecycle change (sharing started / stopped) — those sub-components
  // live in the parent and their ondone/onstopped callbacks must re-derive the
  // mode/status that SharingModes owns.
  export { refresh };
</script>

<div class="modes">
  <label class:disabled={sharingOn}>
    <input
      type="radio"
      checked={mode === 'off'}
      disabled={sharingOn}
      onchange={() => selectMode('off')}
    />
    Off
  </label>
  <label>
    <input
      type="radio"
      checked={mode === 'server'}
      onchange={() => selectMode('server')}
    />
    This machine is the office server
  </label>
  <label class:disabled={sharingOn}>
    <input
      type="radio"
      checked={mode === 'client'}
      disabled={sharingOn}
      onchange={() => selectMode('client')}
    />
    This machine connects to an office server
  </label>
</div>

{#if sharingOn}
  <p class="hint">
    Stop sharing first (in the panel below) before switching modes.
  </p>
{/if}

<style>
  .modes { display: flex; gap: 1rem; }
  .hint { color: var(--text-muted, #888); font-size: 0.8rem; margin: 4px 0 0 0; }
  label.disabled { opacity: 0.5; cursor: not-allowed; }
</style>
