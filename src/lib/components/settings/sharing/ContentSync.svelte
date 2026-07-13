<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';
  import { syncContentNow, subscribeContentSync } from '../../../api/contentSync';

  type Props = {
    visible: boolean;
  };
  let { visible }: Props = $props();

  async function onChange(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    settings.updateField('sync_content', checked);
    if (checked) {
      try {
        await syncContentNow();
        // Start the long-lived SSE subscription now that sync is enabled.
        // Covers the case where the toggle is flipped on outside
        // RecordingsTab (which subscribes on its own mount) (Bug M5).
        await subscribeContentSync();
      } catch (err) {
        console.error('Initial content sync failed:', err);
      }
    }
  }
</script>

{#if visible}
  <label class="form-row" style="margin-top: 1rem;">
    <input
      type="checkbox"
      checked={settings.state.sync_content ?? false}
      onchange={onChange}
    />
    <span>
      Sync patient content via Tailscale
      <p class="hint">
        Syncs transcripts, SOAP notes, letters, and peer discussions between this
        machine and the server over your encrypted Tailscale connection. Audio
        files are archived on the server and fetched on demand.
      </p>
      <p class="hint" style="color: var(--color-warning, #e8a835);">
        Requires Tailscale on both this machine and the server.
      </p>
    </span>
  </label>
{/if}

<style>
  .form-row { display: flex; gap: 10px; align-items: flex-start; }
  .hint { color: var(--text-muted, #888); font-size: 0.8rem; margin: 4px 0 0 0; }
</style>
