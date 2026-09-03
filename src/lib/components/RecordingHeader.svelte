<script lang="ts">
  import { audio } from '../stores/audio.svelte';
  import { formatDuration } from '../utils/format';
  import { audioHealthAlert } from '../utils/audioHealth';
  import Waveform from './Waveform.svelte';
  import OfflineRecordBanner from './OfflineRecordBanner.svelte';

  interface Props {
    onStart?: () => void;
    onStop?: () => void;
    onNewRecording?: () => void;
    onopenSettings?: (target: 'models' | 'audio') => void;
  }
  const { onStart, onStop, onNewRecording, onopenSettings = () => {} }: Props = $props();

  function handleStart() {
    if (onStart) {
      onStart();
    } else {
      audio.startRecording();
    }
  }

  function handleStop() {
    if (onStop) {
      onStop();
    } else {
      audio.stop();
    }
  }

  function handleNew() {
    if (onNewRecording) {
      onNewRecording();
    } else {
      audio.reset();
    }
  }
</script>

<div class="recording-header">
  {#if audio.state.error}
    <div class="error-banner">
      <span class="error-text">{audio.state.error}</span>
      <button class="error-dismiss" onclick={() => audio.reset()}>Dismiss</button>
    </div>
  {/if}

  <OfflineRecordBanner {onopenSettings} />

  {#if audio.state.state === 'recording' || audio.state.state === 'paused'}
    {@const healthAlert = audioHealthAlert(audio.state.health)}
    {#if healthAlert}
      <div class="health-banner {healthAlert.level}" role="alert">
        <span class="health-text">{healthAlert.message}</span>
        <button class="health-settings" onclick={() => onopenSettings('audio')}>
          Audio settings
        </button>
      </div>
    {/if}
  {/if}

  <div class="controls-row">
    <div class="timer">
      {formatDuration(audio.state.elapsed)}
    </div>

    <div class="controls">
      {#if audio.state.state === 'idle'}
        <button class="btn btn-record" onclick={handleStart} title="Record (Space)">
          <span class="btn-icon">●</span> Record
        </button>
      {:else if audio.state.state === 'recording'}
        <button class="btn btn-pause" onclick={() => audio.pause()}>
          <span class="btn-icon">⏸</span> Pause
        </button>
        <button class="btn btn-stop" onclick={handleStop} title="Stop (Space)">
          <span class="btn-icon">■</span> Stop
        </button>
        <button class="btn btn-cancel" onclick={() => audio.cancel()}>
          <span class="btn-icon">✕</span> Cancel
        </button>
      {:else if audio.state.state === 'paused'}
        <button class="btn btn-resume" onclick={() => audio.resume()}>
          <span class="btn-icon">▶</span> Resume
        </button>
        <button class="btn btn-stop" onclick={handleStop} title="Stop (Space)">
          <span class="btn-icon">■</span> Stop
        </button>
        <button class="btn btn-cancel" onclick={() => audio.cancel()}>
          <span class="btn-icon">✕</span> Cancel
        </button>
      {:else if audio.state.state === 'stopped'}
        <button class="btn btn-new" onclick={handleNew} title="Record (Space)">
          <span class="btn-icon">+</span> New Recording
        </button>
      {/if}
    </div>
  </div>

  <div class="waveform-container">
    <Waveform />
  </div>
</div>

<style>
  .recording-header {
    background-color: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
    padding: 16px;
    flex-shrink: 0;
  }

  .error-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    margin-bottom: 12px;
    background-color: var(--danger-bg, rgba(239, 68, 68, 0.1));
    border: 1px solid var(--danger, #ef4444);
    border-radius: var(--radius-md);
    font-size: 13px;
    color: var(--danger, #ef4444);
  }

  .error-text {
    flex: 1;
  }

  .error-dismiss {
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    font-size: 12px;
    color: var(--danger, #ef4444);
    border: 1px solid var(--danger, #ef4444);
    background: transparent;
    cursor: pointer;
  }

  .error-dismiss:hover {
    background-color: var(--danger, #ef4444);
    color: white;
  }

  /* Capture-health watchdog banner (dead stream, silent mic, signal lost). */
  .health-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    margin-bottom: 12px;
    border-radius: var(--radius-md);
    font-size: 13px;
  }

  .health-banner.warning {
    background: color-mix(in srgb, var(--warning) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--warning) 35%, transparent);
    color: var(--text-primary);
  }

  .health-banner.danger {
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--danger) 35%, transparent);
    color: var(--text-primary);
  }

  .health-text {
    flex: 1;
  }

  .health-settings {
    flex-shrink: 0;
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    font-size: 12px;
    color: var(--text-secondary);
    border: 1px solid var(--border);
    background: transparent;
    cursor: pointer;
  }

  .health-settings:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .controls-row {
    display: flex;
    align-items: center;
    gap: 16px;
    margin-bottom: 12px;
  }

  .timer {
    font-family: var(--font-mono);
    font-size: 28px;
    font-weight: 600;
    color: var(--text-primary);
    min-width: 90px;
  }

  .controls {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  .btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: var(--radius-md);
    font-size: 13px;
    font-weight: 500;
    transition: opacity 0.15s ease, filter 0.15s ease;
  }

  .btn:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .btn-icon {
    font-size: 12px;
  }

  .btn-record {
    background-color: var(--danger);
    color: white;
  }

  .btn-pause {
    background-color: var(--warning);
    color: white;
  }

  .btn-stop {
    background-color: var(--bg-tertiary);
    color: var(--text-primary);
    border: 1px solid var(--border);
  }

  .btn-resume {
    background-color: var(--success);
    color: white;
  }

  .btn-cancel {
    background-color: transparent;
    color: var(--danger);
    border: 1px solid var(--danger);
  }

  .btn-cancel:hover:not(:disabled) {
    background-color: var(--danger);
    color: white;
    filter: none;
  }

  .btn-new {
    background-color: var(--accent);
    color: white;
  }

  .waveform-container {
    background-color: var(--bg-tertiary);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
</style>
