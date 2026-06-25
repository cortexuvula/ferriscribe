<script lang="ts">
  import { onDestroy } from 'svelte';
  import { endpointHealth, type EndpointHealthState } from '../stores/endpointHealth.svelte';

  type Props = {
    onopenSettings: (target: 'models' | 'audio') => void;
  };
  const { onopenSettings }: Props = $props();

  let health = $state<EndpointHealthState>({
    ai: 'skipped', stt: 'skipped', lastCheckedAt: null, overall: 'hidden',
  });
  const unsub = endpointHealth.subscribe((s) => (health = s));
  onDestroy(unsub);

  function bannerText(): string {
    if (health.overall === 'offline') {
      return "Office server offline — your recording will save locally, but transcription and SOAP will fail until it's back online.";
    }
    if (health.overall === 'partial') {
      if (health.ai === 'offline' && health.stt === 'online') {
        return 'AI offline — your recording will save locally, but SOAP generation will fail.';
      }
      if (health.stt === 'offline' && health.ai === 'online') {
        return 'Whisper STT offline — your recording will save locally, but transcription will fail.';
      }
    }
    return '';
  }

  function onOpenSettingsClick() {
    // Same routing decision the pill makes:
    // partial with STT offline only → audio
    // everything else (partial with AI offline / offline both) → models
    if (health.overall === 'partial' && health.ai === 'online' && health.stt === 'offline') {
      onopenSettings('audio');
    } else {
      onopenSettings('models');
    }
  }
</script>

{#if health.overall === 'partial' || health.overall === 'offline'}
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
