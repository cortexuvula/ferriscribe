<script lang="ts">
  import { onMount } from 'svelte';
  import { usePairing, friendlyName } from '../../composables/usePairing.svelte';

  interface Props { onNext: () => void; onSkip: () => void; }
  const { onNext, onSkip }: Props = $props();

  // Shared pairing state machine (discovery, dedupe, URL parsing, pairing
  // call) — see usePairing.svelte.ts. Advances the wizard on success.
  const pairing = usePairing(() => onNext());

  onMount(async () => {
    await pairing.prefillLabel();
    pairing.rescan();
  });
</script>

<h2>Connect to your office server</h2>
<p class="hint">An office server is another computer running FerriScribe that hosts the AI and transcription. Pick yours from the list, or paste a pairing URL from its QR code.</p>

<div class="field">
  <label for="ob-pair-label">This computer's label</label>
  <input id="ob-pair-label" type="text" bind:value={pairing.label} placeholder="e.g. Dr. Smith's MacBook" />
</div>

<div class="discovery">
  <h4>Found on your network</h4>
  {#if pairing.scanning}<p class="hint">Scanning…</p>{/if}
  {#if !pairing.scanning && pairing.deduped.length === 0}
    <p class="hint">No servers found. Make sure the office server is running and on the same network, or paste a pairing URL below.</p>
  {/if}
  <ul class="servers">
    {#each pairing.deduped as d (d.instance_name)}
      <li>
        <strong>{friendlyName(d)}</strong>
        <button class="btn-primary small" onclick={() => pairing.pairDiscovered(d)}>Connect</button>
      </li>
    {/each}
  </ul>
  <button class="btn-secondary" onclick={() => pairing.rescan()} disabled={pairing.scanning}>
    {pairing.scanning ? 'Scanning…' : 'Rescan'}
  </button>
</div>

<div class="paste">
  <h4>Or paste a pairing URL</h4>
  <input bind:value={pairing.pasteUrl} placeholder="ferriscribe://pair?..." />
  <button class="btn-primary" disabled={pairing.busy} onclick={() => pairing.pairFromUrl()}>
    {pairing.busy ? 'Pairing…' : 'Pair'}
  </button>
</div>

{#if pairing.error}<p class="error-detail">{pairing.error}</p>{/if}

<div class="actions">
  <button class="btn-skip" onclick={onSkip}>Skip — I'll set this up later</button>
</div>

<style>
  h2 { font-size: 18px; font-weight: 600; margin: 0 0 4px; color: var(--text-primary); }
  h4 { font-size: 13px; font-weight: 600; margin: 16px 0 8px; color: var(--text-secondary); }
  .hint { font-size: 13px; color: var(--text-muted); margin: 0 0 12px; line-height: 1.5; }
  .field { display: flex; flex-direction: column; gap: 4px; margin-bottom: 8px; }
  label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; color: var(--text-secondary); }
  input {
    height: 32px; padding: 0 10px; font-size: 13px; color: var(--text-primary);
    background-color: var(--bg-primary, #1a1a1a); border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px); box-sizing: border-box;
  }
  input:focus { outline: none; border-color: var(--accent, #3b82f6); }
  .servers { list-style: none; padding: 0; margin: 0 0 8px; }
  .servers li {
    display: flex; align-items: center; justify-content: space-between; gap: 12px;
    padding: 8px 0; border-bottom: 1px solid var(--border, #333); font-size: 13px;
  }
  .btn-secondary {
    padding: 6px 14px; font-size: 12px; font-weight: 500; color: var(--text-primary);
    background-color: transparent; border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-secondary:hover:not(:disabled) { border-color: var(--accent, #3b82f6); }
  .btn-secondary:disabled { opacity: 0.6; cursor: not-allowed; }
  .btn-primary {
    padding: 6px 16px; font-size: 12px; font-weight: 600; color: white;
    background-color: var(--accent, #3b82f6); border: none;
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-primary.small { padding: 4px 12px; }
  .btn-primary:hover:not(:disabled) { background-color: var(--accent-hover, #2563eb); }
  .btn-primary:disabled { opacity: 0.6; cursor: not-allowed; }
  .paste { display: flex; flex-direction: column; gap: 6px; }
  .error-detail { font-size: 12px; color: var(--danger, #ef4444); margin: 8px 0; }
  .actions { display: flex; justify-content: center; margin-top: 16px; }
  .btn-skip { padding: 6px 10px; font-size: 12px; color: var(--text-muted); background: none; border: none; cursor: pointer; text-decoration: underline; }
</style>
