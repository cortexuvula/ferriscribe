<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { endpointHealth } from '../stores/endpointHealth.svelte';

  type Props = {
    onopenSettings: (target: 'models' | 'audio') => void;
  };
  const { onopenSettings }: Props = $props();

  // Read the store's reactive state directly — no subscribe()/local copy.
  let stop: () => void = () => {};
  onMount(() => { stop = endpointHealth.start(); });
  onDestroy(() => stop());

  function bannerText(): string {
    if (endpointHealth.state.overall === 'offline') {
      return "Office server offline — your recording will save locally, but transcription and SOAP will fail until it's back online.";
    }
    if (endpointHealth.state.overall === 'partial') {
      if (endpointHealth.state.ai === 'offline' && endpointHealth.state.stt === 'online') {
        return 'AI offline — your recording will save locally, but SOAP generation will fail.';
      }
      if (endpointHealth.state.stt === 'offline' && endpointHealth.state.ai === 'online') {
        return 'Whisper STT offline — your recording will save locally, but transcription will fail.';
      }
    }
    return '';
  }

  function onOpenSettingsClick() {
    // Same routing decision the pill makes:
    // partial with STT offline only → audio
    // everything else (partial with AI offline / offline both) → models
    if (endpointHealth.state.overall === 'partial' && endpointHealth.state.ai === 'online' && endpointHealth.state.stt === 'offline') {
      onopenSettings('audio');
    } else {
      onopenSettings('models');
    }
  }
</script>

{#if endpointHealth.state.overall === 'partial' || endpointHealth.state.overall === 'offline'}
  <div class="offline-banner" role="status" aria-live="polite">
    <span class="banner-text">{bannerText()}</span>
    <button type="button" class="banner-action" onclick={onOpenSettingsClick}>
      Open Settings
    </button>
  </div>
{/if}

<style>
  .offline-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    margin-bottom: 12px;
    background-color: color-mix(in srgb, var(--warning) 10%, transparent);
    border: 1px solid var(--warning);
    border-radius: var(--radius-md);
    font-size: 13px;
    color: var(--warning);
  }
  .banner-text {
    flex: 1;
  }
  .banner-action {
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    font-size: 12px;
    color: var(--warning);
    border: 1px solid var(--warning);
    background: transparent;
    cursor: pointer;
  }
  .banner-action:hover {
    background-color: var(--warning);
    color: white;
  }
  .banner-action:focus-visible {
    outline: 2px solid var(--border-focus, #2563eb);
    outline-offset: 2px;
  }
</style>
