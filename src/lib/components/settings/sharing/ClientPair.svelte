<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount, onDestroy } from 'svelte';
  import { settings } from '../../../stores/settings.svelte';
  import { confirmDialog } from '../../../stores/confirm.svelte';
  import { formatError } from '../../../types/errors';
  import { usePairing, friendlyName, type PairedConnection } from '../../../composables/usePairing.svelte';

  // Shared pairing state machine (discovery, dedupe, URL parsing, pairing
  // call) — see usePairing.svelte.ts. On success, reload this panel's
  // paired-connection display.
  let pairedConn = $state<PairedConnection | null>(null);
  let unpairBusy = $state(false);

  async function loadPaired() {
    try {
      pairedConn = await invoke<PairedConnection | null>('paired_endpoint');
    } catch {
      pairedConn = null;
    }
  }

  const pairing = usePairing(loadPaired);

  async function unpair() {
    const ok = await confirmDialog({
      title: 'Unpair from office server?',
      message: 'Unpair from this office server? Content sync with the server will stop.',
      confirmLabel: 'Unpair',
      danger: true,
    });
    if (!ok) return;
    unpairBusy = true;
    pairing.error = null;
    try {
      await invoke('unpair');
      await settings.load();
      pairedConn = null;
      // Repopulate the discovery list so the connect form is immediately useful.
      pairing.rescan();
    } catch (e) {
      pairing.error = formatError(e);
    } finally {
      unpairBusy = false;
    }
  }

  function onPairUrlEvent(e: Event) {
    const detail = (e as CustomEvent<string>).detail;
    if (typeof detail === 'string' && detail.startsWith('ferriscribe://pair?')) {
      pairing.pasteUrl = detail;
      pairing.pairFromUrl();
    }
  }

  onMount(async () => {
    await loadPaired();
    if (!pairedConn) {
      // Pre-fill the label from the OS hostname so the office server
      // always sees a meaningful computer name.
      await pairing.prefillLabel();
      pairing.rescan();
    }
    window.addEventListener('ferriscribe-pair-url', onPairUrlEvent);
  });

  onDestroy(() => {
    window.removeEventListener('ferriscribe-pair-url', onPairUrlEvent);
  });
</script>

<section>
  <h3>Connect to an office server</h3>
  {#if pairedConn}
    <div class="paired-status">
      <div class="status-line">
        <span class="status-icon" aria-hidden="true">✓</span>
        <strong>Paired</strong>
      </div>
      <dl class="paired-details">
        <dt>Office server address</dt>
        <dd><code>{pairedConn.lan ?? pairedConn.tailscale ?? 'unknown'}</code></dd>
        <dt>This computer's label on the server</dt>
        <dd>{pairedConn.label}</dd>
      </dl>
      <p class="hint">
        Models from the office server are available under Settings &rarr; AI Models.
      </p>
      <button class="btn btn-danger" disabled={unpairBusy} onclick={unpair}>
        {unpairBusy ? 'Unpairing…' : 'Unpair'}
      </button>
      {#if pairing.error}<div class="error">{pairing.error}</div>{/if}
    </div>
  {:else}
    <div class="label-row">
      <label for="ferri-pair-label">This computer's label</label>
      <input
        id="ferri-pair-label"
        bind:value={pairing.label}
        placeholder="e.g. Dr. Smith's MacBook, Room 6"
      />
      <small class="hint">Shown in the Connected clients panel on the office server.</small>
    </div>

    <div class="discovery">
      <h4>Found on your network</h4>
      {#if pairing.scanning}<p class="hint">Scanning…</p>{/if}
      {#if !pairing.scanning && pairing.deduped.length === 0}
        <p class="hint">No servers found. Either no office server is running,
        or your Wi-Fi blocks discovery (UniFi / Meraki client isolation).
        Use the QR or code option below.</p>
      {/if}
      <ul class="servers">
        {#each pairing.deduped as d (d.instance_name)}
          <li>
            <div class="server-info">
              <strong class="server-name">{friendlyName(d)}</strong>
              {#if d.host && d.host !== friendlyName(d)}
                <span class="server-host">{d.host}</span>
              {/if}
            </div>
            <button class="btn btn-primary" onclick={() => pairing.pairDiscovered(d)}>Connect</button>
          </li>
        {/each}
      </ul>
      <button class="btn" onclick={() => pairing.rescan()} disabled={pairing.scanning}>
        {pairing.scanning ? 'Scanning…' : 'Rescan'}
      </button>
    </div>

    <div class="paste">
      <h4>Or paste a pairing URL</h4>
      <input
        bind:value={pairing.pasteUrl}
        placeholder="ferriscribe://pair?..."
        onkeydown={(e) => {
          if (e.key === 'Enter' && !pairing.busy) void pairing.pairFromUrl();
        }}
      />
      <button class="btn btn-primary" disabled={pairing.busy} onclick={() => pairing.pairFromUrl()}>
        {pairing.busy ? 'Pairing…' : 'Pair'}
      </button>
    </div>

    {#if pairing.error}<div class="error">{pairing.error}</div>{/if}
  {/if}
</section>

<style>
  .error { color: var(--danger); margin-top: 0.5rem; }
  .hint { color: var(--text-muted, #888); }

  .servers { list-style: none; padding: 0; margin: 0 0 0.5rem 0; }
  .servers li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.6rem 0;
    border-bottom: 1px solid var(--border);
  }
  .server-info { display: flex; flex-direction: column; gap: 0.15rem; }
  .server-name { font-size: 1rem; }
  .server-host {
    font-size: 0.85rem;
    color: var(--text-muted, #888);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }

  .label-row {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    margin-bottom: 1rem;
  }
  .label-row label { font-weight: 600; font-size: 0.95rem; }
  .label-row input {
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 0.375rem;
    background: var(--bg-input);
    color: var(--text-primary);
  }

  .paste { margin-top: 1.5rem; display: flex; flex-direction: column; gap: 0.5rem; }
  .paste input {
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 0.375rem;
    background: var(--bg-input);
    color: var(--text-primary);
  }

  .paired-status {
    padding: 1rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-card);
  }
  .status-line {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.75rem;
    font-size: 1rem;
  }
  .status-icon {
    width: 1.4rem;
    height: 1.4rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    background: var(--success);
    color: var(--text-inverse);
    font-weight: 700;
  }
  .paired-details {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.4rem 1rem;
    margin: 0 0 0.75rem 0;
    font-size: 0.92rem;
  }
  .paired-details dt { color: var(--text-muted, #888); }
  .paired-details dd { margin: 0; }
  .paired-details code {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }

  .btn {
    border: 1px solid var(--border);
    background: var(--bg-tertiary);
    color: var(--text-primary);
    padding: 0.4rem 0.9rem;
    border-radius: 0.375rem;
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.12s ease, border-color 0.12s ease;
  }
  .btn:hover:not(:disabled) {
    background: var(--bg-hover);
    border-color: var(--accent);
  }
  .btn-primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--text-inverse);
  }
  .btn-primary:hover:not(:disabled) {
    background: var(--accent-hover);
    border-color: var(--accent-hover);
  }
  .btn-danger {
    border-color: var(--danger);
    color: var(--danger);
  }
  .btn-danger:hover:not(:disabled) {
    background: var(--danger);
    color: var(--text-inverse);
  }
</style>
