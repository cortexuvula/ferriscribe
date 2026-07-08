<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';
  import { syncConditionChips } from '../../../api/conditions';

  type Props = {
    visible: boolean;
  };
  let { visible }: Props = $props();

  async function onChange(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    settings.updateField('sync_condition_chips', checked);
    if (checked) {
      try {
        await syncConditionChips();
      } catch (err) {
        console.error('Initial condition chip sync failed:', err);
      }
    }
  }
</script>

{#if visible}
  <label class="form-row" style="margin-top: 1rem;">
    <input
      type="checkbox"
      checked={settings.state.sync_condition_chips ?? false}
      onchange={onChange}
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

<style>
  .form-row { display: flex; gap: 10px; align-items: flex-start; }
  .hint { color: var(--text-muted, #888); font-size: 0.8rem; margin: 4px 0 0 0; }
</style>
