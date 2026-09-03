<script lang="ts">
  import { settings } from '../../stores/settings.svelte';
  import { toasts } from '../../stores/toasts.svelte';
  import { runMicrophoneProbe, type MicrophoneProbeResult } from '../../api/audio';
  import { formatError } from '../../types/errors';
  import type { AudioDevice } from '../../types';
  import Callout from './Callout.svelte';

  interface Props {
    audioDevices: AudioDevice[];
    devicesLoading: boolean;
  }

  const { audioDevices, devicesLoading }: Props = $props();

  async function handleInputDeviceChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value;
    probeResult = null; // stale probe for a different device
    await settings.updateField('input_device', value || null);
  }

  let probing = $state(false);
  let probeResult = $state<MicrophoneProbeResult | null>(null);

  async function handleTestMic() {
    probing = true;
    probeResult = null;
    try {
      probeResult = await runMicrophoneProbe(settings.state.input_device ?? null);
    } catch (e) {
      toasts.error(`Microphone test failed: ${formatError(e)}`);
    } finally {
      probing = false;
    }
  }

  function dbfs(rms: number): string {
    return rms > 0 ? `${(20 * Math.log10(rms)).toFixed(1)} dBFS` : 'digital silence';
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
      {#each audioDevices as device (device.name)}
        <option value={device.name}>
          {device.name}{device.is_default ? ' (Default)' : ''}
        </option>
      {/each}
    {/if}
  </select>
  <div class="probe-row">
    <button class="btn-test" onclick={handleTestMic} disabled={probing || devicesLoading}>
      {probing ? 'Testing…' : 'Test microphone'}
    </button>
    <span class="form-hint">
      Captures ~1 s from the selected device to confirm it is live before recording.
    </span>
  </div>
  {#if probeResult}
    {#if probeResult.is_silent}
      <Callout kind="danger">
        No signal detected ({dbfs(probeResult.rms)}). The microphone is delivering silence —
        check that it is connected and unmuted, and that FerriScribe has microphone permission
        in your OS privacy settings.
      </Callout>
    {:else}
      <Callout kind="success" role="status">
        Signal detected — peak {Math.round(probeResult.peak * 100)}%, level {dbfs(probeResult.rms)}
        over {probeResult.samples.toLocaleString()} samples.
      </Callout>
    {/if}
  {/if}
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

  .form-hint {
    font-size: 11px;
    color: var(--text-muted);
  }

  .probe-row {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 2px;
  }

  .btn-test {
    flex-shrink: 0;
    padding: 6px 14px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease, border-color 0.15s ease;
  }

  .btn-test:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .btn-test:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
