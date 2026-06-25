<script lang="ts">
  import { settings } from '../../stores/settings.svelte';
  import type { AudioDevice } from '../../types';

  interface Props {
    audioDevices: AudioDevice[];
    devicesLoading: boolean;
  }

  const { audioDevices, devicesLoading }: Props = $props();

  async function handleInputDeviceChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value;
    await settings.updateField('input_device', value || null);
  }
</script>

<div class="form-group">
  <label for="input-device" class="form-label">Input Device</label>
  <select
    id="input-device"
    value={settings.state.input_device ?? ''}
    onchange={handleInputDeviceChange}
    disabled={devicesLoading}
  >
    {#if devicesLoading}
      <option value="">Loading devices...</option>
    {:else}
      <option value="">System Default</option>
      {#each audioDevices as device}
        <option value={device.name}>
          {device.name}{device.is_default ? ' (Default)' : ''}
        </option>
      {/each}
    {/if}
  </select>
</div>

<style>
  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-label {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    gap: 8px;
  }
</style>
