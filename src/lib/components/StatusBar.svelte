<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { audio } from '../stores/audio';
  import { settings } from '../stores/settings';
  import { formatDuration } from '../utils/format';

  type SharingStatus = {
    enabled: boolean;
    paired_clients: number;
  };
  type PairedConn = { label: string } | null;

  let sharing: SharingStatus | null = null;
  let paired: PairedConn = null;
  let pollHandle: ReturnType<typeof setInterval>;

  async function refresh() {
    try {
      sharing = await invoke<SharingStatus>('sharing_status');
    } catch { sharing = null; }
    try {
      paired = await invoke<PairedConn>('paired_endpoint');
    } catch { paired = null; }
  }

  onMount(() => {
    refresh();
    // 5s poll matches ServerStatus.svelte's panel poll, so the statusbar
    // and the Sharing settings panel stay roughly in sync without coupling.
    pollHandle = setInterval(refresh, 5000);
  });

  onDestroy(() => {
    if (pollHandle) clearInterval(pollHandle);
  });
</script>

<div class="statusbar">
  <div class="status-left">
    {#if $audio.state === 'recording'}
      <span class="status-indicator recording">● REC</span>
      <span class="status-timer">{formatDuration($audio.elapsed)}</span>
    {:else if $audio.state === 'paused'}
      <span class="status-indicator paused">⏸ PAUSED</span>
      <span class="status-timer">{formatDuration($audio.elapsed)}</span>
    {:else if $audio.state === 'stopped'}
      <span class="status-indicator stopped">■ Stopped</span>
    {:else}
      <span class="status-indicator ready">Ready</span>
    {/if}
  </div>

  <div class="status-right">
    {#if sharing?.enabled}
      <span class="sharing-badge server" title="This machine is acting as an office server. Other paired clients can reach Ollama / Whisper / LM Studio via this device.">
        <span class="dot" aria-hidden="true"></span>
        Office Server
        {#if sharing.paired_clients > 0}
          <span class="badge-count">· {sharing.paired_clients} client{sharing.paired_clients === 1 ? '' : 's'}</span>
        {/if}
      </span>
      <span class="status-sep">·</span>
    {:else if paired}
      <span class="sharing-badge client" title={`Paired with office server${paired.label ? ` as “${paired.label}”` : ''}.`}>
        <span class="dot" aria-hidden="true"></span>
        Paired
      </span>
      <span class="status-sep">·</span>
    {/if}
    <span class="status-provider">AI: {$settings.ai_provider}/{$settings.ai_model}</span>
    <span class="status-sep">·</span>
    <span class="status-provider">STT: {$settings.whisper_model}</span>
  </div>
</div>

<style>
  .statusbar {
    height: var(--statusbar-height);
    background-color: var(--bg-secondary);
    border-top: 1px solid var(--border);
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 12px;
    font-size: 11px;
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .status-left {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .status-right {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .status-indicator {
    font-weight: 500;
    letter-spacing: 0.02em;
  }

  .status-indicator.recording {
    color: var(--danger);
  }

  .status-indicator.paused {
    color: var(--warning);
  }

  .status-indicator.stopped {
    color: var(--text-secondary);
  }

  .status-indicator.ready {
    color: var(--text-muted);
  }

  .status-timer {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-secondary);
  }

  .status-provider {
    font-size: 11px;
  }

  .status-sep {
    color: var(--border);
  }

  .sharing-badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 1px 7px;
    border-radius: 999px;
    font-weight: 600;
    letter-spacing: 0.02em;
    cursor: default;
  }
  .sharing-badge.server {
    background: rgba(22, 163, 74, 0.15);
    color: #16a34a;
    border: 1px solid rgba(22, 163, 74, 0.35);
  }
  .sharing-badge.client {
    background: rgba(37, 99, 235, 0.15);
    color: #2563eb;
    border: 1px solid rgba(37, 99, 235, 0.35);
  }
  .sharing-badge .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: currentColor;
    box-shadow: 0 0 4px currentColor;
  }
  .sharing-badge .badge-count {
    font-weight: 400;
    opacity: 0.8;
  }
</style>
