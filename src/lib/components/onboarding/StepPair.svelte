<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { suggestedClientLabel } from '../../api/sharing';
  import { settings } from '../../stores/settings.svelte';
  import { formatError } from '../../types/errors';

  interface Props { onNext: () => void; onSkip: () => void; }
  const { onNext, onSkip }: Props = $props();

  // Mirrors the Discovered / PairPorts types from ClientPair.svelte — the
  // shape of the sharing discovery commands' return values.
  type Discovered = {
    instance_name: string; host: string;
    addresses: string[]; tailscale_addresses?: string[];
    ports: { ollama: number | null; whisper: number | null; lmstudio: number | null; pairing: number | null; vocab: number | null };
    version: string;
  };
  type PairPorts = { ollama: number; whisper: number; pairing: number; lmstudio: number | null; vocab: number | null };

  // MUST be $state — rescan() reassigns it and the deduped $derived below
  // tracks it. A plain `let` here made the discovered-server list permanently
  // empty (the dedup effect only ran once at mount with the empty array).
  let discovered = $state<Discovered[]>([]);
  let scanning = $state(false);
  let pasteUrl = $state('');
  let label = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  function friendlyName(d: Discovered): string {
    const m = d.instance_name.match(/^(.+?)\._ferriscribe\._tcp\.local\.?$/);
    if (m) return m[1];
    return d.host || d.instance_name;
  }

  // Dedupe by instance_name (mDNS fires per-interface) and merge addresses,
  // recomputing whenever the raw discovered list changes. $derived (not
  // $effect) so there's no chance of a stale write or feedback loop.
  const deduped = $derived.by(() => {
    const seen = new Map<string, Discovered>();
    for (const d of discovered) {
      const ex = seen.get(d.instance_name);
      if (!ex) {
        seen.set(d.instance_name, { ...d, addresses: [...d.addresses], tailscale_addresses: [...(d.tailscale_addresses ?? [])] });
      } else {
        for (const a of d.addresses) if (!ex.addresses.includes(a)) ex.addresses.push(a);
        const tsList = (ex.tailscale_addresses ??= []);
        for (const a of d.tailscale_addresses ?? []) if (!tsList.includes(a)) tsList.push(a);
      }
    }
    return Array.from(seen.values());
  });

  function bestFrom(addresses: string[]): string | null {
    if (addresses.length === 0) return null;
    const score = (a: string): number => {
      const isV6 = a.includes(':');
      if (!isV6) {
        if (/^(192\.168\.|10\.|172\.(1[6-9]|2[0-9]|3[01])\.)/.test(a)) return 0;
        return 1;
      }
      if (/^fe80:/i.test(a)) return 3;
      return 2;
    };
    return [...addresses].sort((a, b) => score(a) - score(b))[0];
  }

  async function rescan() {
    scanning = true;
    discovered = [];
    try {
      const [lan, ts] = await Promise.all([
        invoke<Discovered[]>('discover_servers', { timeoutMs: 3000 }).catch(() => []),
        invoke<Discovered[]>('discover_via_tailscale', { timeoutMs: 3000 }).catch(() => []),
      ]);
      discovered = [...lan, ...ts];
    } finally {
      scanning = false;
    }
  }

  async function pairManual(lan: string | null, tailscale: string | null, ports: PairPorts, code: string) {
    busy = true;
    error = null;
    try {
      let tokenLabel = label.trim();
      if (!tokenLabel) {
        try { tokenLabel = (await suggestedClientLabel()).trim(); } catch { tokenLabel = ''; }
      }
      if (!tokenLabel) {
        error = 'Please enter a label for this computer.';
        busy = false;
        return;
      }
      await invoke('pair_with_server', { lan, tailscale, ports, code, label: tokenLabel });
      await settings.load();
      onNext(); // paired — advance
    } catch (e) {
      error = formatError(e);
    } finally {
      busy = false;
    }
  }

  function pairFromUrl() {
    if (!pasteUrl.startsWith('ferriscribe://pair?')) {
      error = 'Not a FerriScribe pairing URL.';
      return;
    }
    const u = new URL(pasteUrl.replace('ferriscribe://', 'http://x/'));
    const lan = u.searchParams.get('lan');
    const ts = u.searchParams.get('ts');
    const code = u.searchParams.get('code') ?? '';
    const pp = parseInt(u.searchParams.get('pp') ?? '', 10);
    const op = parseInt(u.searchParams.get('op') ?? '', 10);
    const wp = parseInt(u.searchParams.get('wp') ?? '', 10);
    if (!Number.isFinite(pp) || !Number.isFinite(op) || !Number.isFinite(wp)) {
      error = 'Pairing URL is missing required ports.';
      return;
    }
    const lp = parseInt(u.searchParams.get('lp') ?? '', 10);
    const vp = parseInt(u.searchParams.get('vp') ?? '', 10);
    if (!lan && !ts) { error = 'No reachable address in URL.'; return; }
    pairManual(lan, ts, {
      ollama: op, whisper: wp, pairing: pp,
      lmstudio: Number.isFinite(lp) ? lp : null, vocab: Number.isFinite(vp) ? vp : null,
    }, code);
  }

  function pairDiscovered(d: Discovered) {
    const lan = bestFrom(d.addresses);
    const tailscale = (d.tailscale_addresses ?? [])[0] ?? null;
    if (!lan && !tailscale) { error = 'No reachable address for this server.'; return; }
    const ports: PairPorts = {
      ollama: d.ports.ollama ?? 11435,
      whisper: d.ports.whisper ?? 8081,
      pairing: d.ports.pairing ?? 11436,
      lmstudio: d.ports.lmstudio ?? null,
      vocab: d.ports.vocab ?? null,
    };
    const code = prompt('Enter the 6-digit code from the office server.') ?? '';
    if (!code) return;
    pairManual(lan, tailscale, ports, code);
  }

  onMount(async () => {
    if (!label) {
      try { label = await suggestedClientLabel(); } catch { /* leave empty */ }
    }
    rescan();
  });
</script>

<h2>Connect to your office server</h2>
<p class="hint">An office server is another computer running FerriScribe that hosts the AI and transcription. Pick yours from the list, or paste a pairing URL from its QR code.</p>

<div class="field">
  <label for="ob-pair-label">This computer's label</label>
  <input id="ob-pair-label" type="text" bind:value={label} placeholder="e.g. Dr. Smith's MacBook" />
</div>

<div class="discovery">
  <h4>Found on your network</h4>
  {#if scanning}<p class="hint">Scanning…</p>{/if}
  {#if !scanning && deduped.length === 0}
    <p class="hint">No servers found. Make sure the office server is running and on the same network, or paste a pairing URL below.</p>
  {/if}
  <ul class="servers">
    {#each deduped as d (d.instance_name)}
      <li>
        <strong>{friendlyName(d)}</strong>
        <button class="btn-primary small" onclick={() => pairDiscovered(d)}>Connect</button>
      </li>
    {/each}
  </ul>
  <button class="btn-secondary" onclick={rescan} disabled={scanning}>
    {scanning ? 'Scanning…' : 'Rescan'}
  </button>
</div>

<div class="paste">
  <h4>Or paste a pairing URL</h4>
  <input bind:value={pasteUrl} placeholder="ferriscribe://pair?..." />
  <button class="btn-primary" disabled={busy} onclick={pairFromUrl}>
    {busy ? 'Pairing…' : 'Pair'}
  </button>
</div>

{#if error}<p class="error-detail">{error}</p>{/if}

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
