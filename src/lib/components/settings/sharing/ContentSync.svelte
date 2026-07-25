<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { settings } from '../../../stores/settings.svelte';
  import { recordings, startBackgroundSync, stopBackgroundSync } from '../../../stores/recordings.svelte';
  import { syncContentNow, subscribeContentSync } from '../../../api/contentSync';
  import { toasts } from '../../../stores/toasts.svelte';

  type Props = {
    visible: boolean;
  };
  let { visible }: Props = $props();

  // Listen for sync-complete events so lastSyncedAt updates even when the
  // sync was triggered by startup or background timer (not the Sync Now
  // button in this component).
  let unlistenSyncComplete: (() => void) | null = null;

  onMount(async () => {
    try {
      unlistenSyncComplete = await listen('content-sync-complete', () => {
        recordings.lastSyncedAt = new Date();
      });
    } catch {
      // Non-fatal: event listener failure just means the timestamp won't update
    }
  });

  onDestroy(() => {
    unlistenSyncComplete?.();
  });

  async function onChange(e: Event) {
    const target = e.target as HTMLInputElement;
    const checked = target.checked;
    if (checked) {
      try {
        await syncContentNow();
        await subscribeContentSync();
        // Persist the toggle only after setup completed; the $effect below
        // will start background sync in response to the settings change.
        settings.updateField('sync_content', true);
        startBackgroundSync();
      } catch (err) {
        // Rollback: sync setup failed, don't persist the toggle.
        target.checked = false;
        console.error('Failed to enable content sync:', err);
        toasts.error('Could not enable content sync. Check your connection.');
      }
    } else {
      settings.updateField('sync_content', false);
      stopBackgroundSync();
    }
  }

  // Start/stop background sync based on settings state.
  $effect(() => {
    if (settings.state.sync_content) {
      startBackgroundSync();
    } else {
      stopBackgroundSync();
    }
  });

  async function handleSyncNow() {
    try {
      await syncContentNow();
      toasts.success('Content sync complete');
    } catch (err) {
      toasts.error(`Sync failed: ${err}`);
    }
  }

  function formatLastSynced(date: Date | null): string {
    if (!date) return 'never';
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMin = Math.floor(diffMs / 60000);
    if (diffMin < 1) return 'just now';
    if (diffMin < 60) return `${diffMin}m ago`;
    const diffHr = Math.floor(diffMin / 60);
    return `${diffHr}h ago`;
  }
</script>

{#if visible}
  <div class="content-sync-section">
    <label class="form-row">
      <input
        type="checkbox"
        checked={settings.state.sync_content ?? false}
        onchange={onChange}
      />
      <span>
        Sync patient content via Tailscale
        <p class="hint">
          Syncs transcripts, SOAP notes, letters, peer discussions, and audio
          between this machine and the server over your encrypted Tailscale
          connection. Background sync runs every 5 minutes.
        </p>
        <p class="hint" style="color: var(--color-warning, #e8a835);">
          Requires Tailscale on both this machine and the server.
        </p>
      </span>
    </label>

    {#if settings.state.sync_content}
      <div class="sync-controls">
        <button
          class="btn-sync-now"
          onclick={handleSyncNow}
          disabled={recordings.syncing}
        >
          {recordings.syncing ? 'Syncing…' : 'Sync Now'}
        </button>
        <span class="last-synced">
          Last synced: {formatLastSynced(recordings.lastSyncedAt)}
        </span>
      </div>
    {/if}
  </div>
{/if}

<style>
  .content-sync-section { margin-top: 1rem; }
  .form-row { display: flex; gap: 10px; align-items: flex-start; }
  .hint { color: var(--text-muted, #888); font-size: 0.8rem; margin: 4px 0 0 0; }

  .sync-controls {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 8px;
    margin-left: 24px;
  }

  .btn-sync-now {
    padding: 6px 16px;
    font-size: 0.85rem;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-hover, rgba(255,255,255,0.05));
    border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px);
    cursor: pointer;
    transition: background-color 0.15s ease, border-color 0.15s ease;
  }

  .btn-sync-now:hover:not(:disabled) {
    background-color: var(--bg-hover-strong, rgba(255,255,255,0.1));
    border-color: var(--accent, #3b82f6);
  }

  .btn-sync-now:disabled {
    opacity: 0.6;
    cursor: wait;
  }

  .last-synced {
    font-size: 0.75rem;
    color: var(--text-muted, #888);
  }
</style>
