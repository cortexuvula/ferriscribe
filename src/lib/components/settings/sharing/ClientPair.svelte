<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount, onDestroy } from 'svelte';

  type Discovered = {
    instance_name: string;
    host: string;
    addresses: string[];
    ports: { ollama: number | null; whisper: number | null; lmstudio: number | null; pairing: number | null };
    version: string;
  };

  type PairPorts = { ollama: number; whisper: number; pairing: number; lmstudio: number | null };

  let discovered: Discovered[] = [];
  let scanning = false;
  let pasteUrl = '';
  let label = '';
  let busy = false;
  let error: string | null = null;
  let success = false;

  // Strip the mDNS suffix `._ferriscribe._tcp.local.` to recover the
  // human-readable friendly name the office-server admin set in the wizard.
  function friendlyName(d: Discovered): string {
    const m = d.instance_name.match(/^(.+?)\._ferriscribe\._tcp\.local\.?$/);
    if (m) return m[1];
    if (d.host) return d.host;
    return d.instance_name;
  }

  // mDNS browsers fire one ServiceResolved event per interface, so the
  // same logical office server appears N times with overlapping address
  // sets. Dedupe by instance_name (unique per service registration) and
  // merge addresses across events so the picker has every candidate.
  $: deduped = (() => {
    const seen = new Map<string, Discovered>();
    for (const d of discovered) {
      const existing = seen.get(d.instance_name);
      if (!existing) {
        seen.set(d.instance_name, { ...d, addresses: [...d.addresses] });
      } else {
        for (const a of d.addresses) {
          if (!existing.addresses.includes(a)) existing.addresses.push(a);
        }
      }
    }
    return Array.from(seen.values());
  })();

  // Pick the most useful address from a server's resolved set:
  //   1. RFC1918 IPv4 (192.168/10/172.16-31) — almost always the right answer
  //      on a clinic LAN
  //   2. Other IPv4 (e.g. 100.x Tailscale CGNAT, public-routable)
  //   3. IPv6 ULA (fc/fd) or globally-routable
  //   4. IPv6 link-local (fe80::, last resort — usually unreachable across hosts)
  function bestAddress(d: Discovered): string | null {
    if (d.addresses.length === 0) return null;
    const score = (a: string): number => {
      const isV6 = a.includes(':');
      if (!isV6) {
        if (/^(192\.168\.|10\.|172\.(1[6-9]|2[0-9]|3[01])\.)/.test(a)) return 0;
        return 1;
      }
      if (/^fe80:/i.test(a)) return 3;
      return 2;
    };
    return [...d.addresses].sort((a, b) => score(a) - score(b))[0];
  }

  async function rescan() {
    scanning = true;
    discovered = [];
    try {
      discovered = await invoke('discover_servers', { timeoutMs: 3000 });
    } finally {
      scanning = false;
    }
  }

  async function pairManual(
    lan: string | null,
    tailscale: string | null,
    ports: PairPorts,
    code: string,
  ) {
    busy = true;
    error = null;
    try {
      const tokenLabel = label || 'this laptop';
      await invoke('pair_with_server', {
        lan,
        tailscale,
        ports,
        code,
        label: tokenLabel,
      });
      success = true;
    } catch (e) {
      error = String(e);
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

    const ppRaw = u.searchParams.get('pp');
    const pp = ppRaw ? parseInt(ppRaw, 10) : NaN;
    if (!Number.isFinite(pp) || pp <= 0 || pp > 65535) {
      error = 'Pairing URL is missing a valid pairing port (pp).';
      return;
    }

    const op = parseInt(u.searchParams.get('op') ?? '', 10);
    const wp = parseInt(u.searchParams.get('wp') ?? '', 10);
    if (!Number.isFinite(op) || !Number.isFinite(wp)) {
      error = 'Pairing URL is missing required ports (op or wp).';
      return;
    }

    const lpRaw = u.searchParams.get('lp');
    const lp = lpRaw ? parseInt(lpRaw, 10) : null;

    if (!lan && !ts) { error = 'No reachable address in URL'; return; }
    pairManual(lan, ts, {
      ollama: op,
      whisper: wp,
      pairing: pp,
      lmstudio: lp !== null && Number.isFinite(lp) ? lp : null,
    }, code);
  }

  function pairDiscovered(d: Discovered) {
    const lan = bestAddress(d);
    const ports: PairPorts = {
      ollama: d.ports.ollama ?? 11435,
      whisper: d.ports.whisper ?? 8081,
      pairing: d.ports.pairing ?? 11436,
      lmstudio: d.ports.lmstudio ?? null,
    };
    const code = prompt('Enter the 6-digit code from the office server.') ?? '';
    if (!code) return;
    pairManual(lan, null, ports, code);
  }

  function onPairUrlEvent(e: Event) {
    const detail = (e as CustomEvent<string>).detail;
    if (typeof detail === 'string' && detail.startsWith('ferriscribe://pair?')) {
      pasteUrl = detail;
      pairFromUrl();
    }
  }

  onMount(() => {
    rescan();
    window.addEventListener('ferriscribe-pair-url', onPairUrlEvent);
  });

  onDestroy(() => {
    window.removeEventListener('ferriscribe-pair-url', onPairUrlEvent);
  });
</script>

<section>
  <h3>Connect to an office server</h3>
  {#if success}
    <div class="ok">Paired. The model pickers in Models settings now show
    the office server's installed models.</div>
  {:else}
    <div class="label-row">
      <label for="ferri-pair-label">This computer's label</label>
      <input
        id="ferri-pair-label"
        bind:value={label}
        placeholder="e.g. Dr. Smith's MacBook, Room 6"
      />
      <small class="hint">Shown in the Connected clients panel on the office server.</small>
    </div>

    <div class="discovery">
      <h4>Found on your network</h4>
      {#if scanning}<p class="hint">Scanning…</p>{/if}
      {#if !scanning && deduped.length === 0}
        <p class="hint">No servers found. Either no office server is running,
        or your Wi-Fi blocks discovery (UniFi / Meraki client isolation).
        Use the QR or code option below.</p>
      {/if}
      <ul class="servers">
        {#each deduped as d (d.instance_name)}
          <li>
            <div class="server-info">
              <strong class="server-name">{friendlyName(d)}</strong>
              {#if d.host && d.host !== friendlyName(d)}
                <span class="server-host">{d.host}</span>
              {/if}
            </div>
            <button class="btn btn-primary" onclick={() => pairDiscovered(d)}>Connect</button>
          </li>
        {/each}
      </ul>
      <button class="btn" onclick={rescan} disabled={scanning}>
        {scanning ? 'Scanning…' : 'Rescan'}
      </button>
    </div>

    <div class="paste">
      <h4>Or paste a pairing URL</h4>
      <input bind:value={pasteUrl} placeholder="ferriscribe://pair?..." />
      <button class="btn btn-primary" disabled={busy} onclick={pairFromUrl}>
        {busy ? 'Pairing…' : 'Pair'}
      </button>
    </div>

    {#if error}<div class="error">{error}</div>{/if}
  {/if}
</section>

<style>
  .ok { color: #16a34a; }
  .error { color: #c00; margin-top: 0.5rem; }
  .hint { color: var(--text-muted, #888); }

  .servers { list-style: none; padding: 0; margin: 0 0 0.5rem 0; }
  .servers li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.6rem 0;
    border-bottom: 1px solid var(--border, #ddd);
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
    border: 1px solid var(--border, #c8c8c8);
    border-radius: 0.375rem;
    background: var(--surface-1, transparent);
    color: inherit;
  }

  .paste { margin-top: 1.5rem; display: flex; flex-direction: column; gap: 0.5rem; }
  .paste input {
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border, #c8c8c8);
    border-radius: 0.375rem;
    background: var(--surface-1, transparent);
    color: inherit;
  }

  .btn {
    border: 1px solid var(--border, #c8c8c8);
    background: var(--surface-1, #fff);
    color: inherit;
    padding: 0.4rem 0.9rem;
    border-radius: 0.375rem;
    font-weight: 500;
    cursor: pointer;
    transition: background 0.12s ease, border-color 0.12s ease;
  }
  .btn:hover:not(:disabled) {
    background: var(--surface-2, #f0f0f0);
    border-color: var(--border-strong, #a0a0a0);
  }
  .btn-primary {
    background: #2563eb;
    border-color: #2563eb;
    color: white;
  }
  .btn-primary:hover:not(:disabled) {
    background: #1d4ed8;
    border-color: #1d4ed8;
  }
</style>
