<script lang="ts">
  import { onDestroy } from 'svelte';
  import { endpointHealth, type EndpointHealthState } from '../stores/endpointHealth.svelte';
  import { settings } from '../stores/settings.svelte';

  type Props = {
    onopenSettings: (target: 'models' | 'audio') => void;
  };
  let { onopenSettings }: Props = $props();

  let health = $state<EndpointHealthState>({
    ai: 'skipped', stt: 'skipped', lastCheckedAt: null, overall: 'hidden',
  });
  const unsub = endpointHealth.subscribe((s) => (health = s));
  onDestroy(unsub);

  function aiProviderLabel(): string {
    // Valid providers are 'ollama' and 'lmstudio'; binary fallback is intentional.
    return settings.state.ai_provider === 'ollama' ? 'Ollama' : 'LM Studio';
  }

  function lastCheckedDescription(): string {
    if (!health.lastCheckedAt) return 'not checked yet';
    const seconds = Math.floor((Date.now() - health.lastCheckedAt) / 1000);
    return `last checked ${seconds}s ago`;
  }

  function tooltipText(): string {
    const parts: string[] = [];
    if (health.ai === 'online') parts.push(`${aiProviderLabel()} online`);
    else if (health.ai === 'offline') parts.push(`${aiProviderLabel()} offline`);
    if (health.stt === 'online') parts.push('Whisper STT online');
    else if (health.stt === 'offline') parts.push('Whisper STT offline');
    if (parts.length === 0) return '';
    return `${parts.join(', ')} — ${lastCheckedDescription()}`;
  }

  function ariaLabelText(): string {
    switch (health.overall) {
      case 'online': return 'AI services online';
      case 'partial': return 'AI services partially offline';
      case 'offline': return 'AI services offline';
      default: return '';
    }
  }

  function onClick() {
    if (health.overall === 'online' || health.overall === 'hidden') return;
    // partial with AI offline only → models
    // partial with STT offline only → audio
    // offline (both) → models (AI is the more common case)
    if (health.overall === 'partial' && health.ai === 'online' && health.stt === 'offline') {
      onopenSettings('audio');
    } else {
      onopenSettings('models');
    }
  }

  function variantClass(): string {
    switch (health.overall) {
      case 'online': return 'ok';
      case 'partial': return 'warn';
      case 'offline': return 'error';
      default: return '';
    }
  }
</script>

{#if health.overall !== 'hidden'}
  <button
    type="button"
    class="endpoint-pill {variantClass()}"
    title={tooltipText()}
    aria-label={ariaLabelText()}
    onclick={onClick}
  >
    <span class="dot" aria-hidden="true"></span>
    AI
  </button>
{/if}

<style>
  .endpoint-pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 1px 7px;
    border-radius: 999px;
    font-weight: 600;
    letter-spacing: 0.02em;
    font-size: 11px;
    cursor: pointer;
    background: transparent;
  }
  .endpoint-pill.ok {
    background: color-mix(in srgb, var(--success) 15%, transparent);
    color: var(--success);
    border: 1px solid color-mix(in srgb, var(--success) 35%, transparent);
  }
  .endpoint-pill.warn {
    background: color-mix(in srgb, var(--warning) 15%, transparent);
    color: var(--warning);
    border: 1px solid color-mix(in srgb, var(--warning) 35%, transparent);
  }
  .endpoint-pill.error {
    background: color-mix(in srgb, var(--danger) 15%, transparent);
    color: var(--danger);
    border: 1px solid color-mix(in srgb, var(--danger) 35%, transparent);
  }
  .endpoint-pill .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: currentColor;
    box-shadow: 0 0 4px currentColor;
  }
  .endpoint-pill:hover {
    filter: brightness(1.05);
  }
  .endpoint-pill:focus-visible {
    outline: 2px solid var(--border-focus, #2563eb);
    outline-offset: 2px;
  }
</style>
