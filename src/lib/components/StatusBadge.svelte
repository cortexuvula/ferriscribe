<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount, onDestroy } from 'svelte';

  type Mode = 'off' | 'server' | 'client';
  type Conn = 'lan' | 'tailscale' | 'reconnecting' | 'down' | 'local';

  let mode = $state<Mode>('off');
  let conn = $state<Conn>('down');
  let pollHandle: ReturnType<typeof setInterval> | null = null;

  async function poll() {
    try {
      const status = await invoke<{ enabled: boolean }>('sharing_status');
      if (status.enabled) {
        mode = 'server';
        conn = 'local';
        return;
      }
    } catch {
      // ignore — sharing simply not available
    }
    // Client-side connection state would be queried from a paired-connection
    // store. Without that wired up yet (Task 12 went Path B), default to 'off'.
    mode = 'off';
    conn = 'down';
  }

  onMount(() => {
    poll();
    pollHandle = setInterval(poll, 5000);
  });

  onDestroy(() => {
    if (pollHandle) clearInterval(pollHandle);
  });

  const color = $derived(
    conn === 'lan' || conn === 'local' ? '#0a0' :
    conn === 'tailscale' ? '#0a8' :
    conn === 'reconnecting' ? '#fa0' :
    '#c00'
  );

  const label = $derived(
    mode === 'off' ? '' :
    mode === 'server' ? 'Office server' :
    conn === 'lan' ? 'Connected (LAN)' :
    conn === 'tailscale' ? 'Connected (Tailscale)' :
    conn === 'reconnecting' ? 'Reconnecting…' :
    'Office server unreachable'
  );
</script>

{#if mode !== 'off'}
  <div class="badge">
    <span class="dot" style:background={color}></span>
    {label}
  </div>
{/if}

<style>
  .badge { display: inline-flex; align-items: center; gap: 0.4rem; font-size: 0.85rem; }
  .dot { width: 8px; height: 8px; border-radius: 50%; }
</style>
