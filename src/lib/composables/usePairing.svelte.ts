import { invoke } from '@tauri-apps/api/core';
import { suggestedClientLabel } from '../api/sharing';
import { settings } from '../stores/settings.svelte';
import { formatError } from '../types/errors';

/** Shape of the `discover_servers` / `discover_via_tailscale` return values. */
export type Discovered = {
  instance_name: string;
  host: string;
  /// Addresses learned via mDNS broadcast (LAN multicast).
  addresses: string[];
  /// Addresses learned via Tailscale peer enumeration. Optional for
  /// serde backward-compat with older clients that didn't emit this field.
  tailscale_addresses?: string[];
  ports: {
    ollama: number | null;
    whisper: number | null;
    lmstudio: number | null;
    omlx: number | null;
    pairing: number | null;
    vocab: number | null;
  };
  version: string;
};

export type PairPorts = {
  ollama: number;
  whisper: number;
  pairing: number;
  lmstudio: number | null;
  omlx: number | null;
  vocab: number | null;
};

export type PairedConnection = {
  lan: string | null;
  tailscale: string | null;
  ports: PairPorts;
  label: string;
};

/** Strip the mDNS suffix `._ferriscribe._tcp.local.` to recover the
 * human-readable friendly name the office-server admin set in the wizard. */
export function friendlyName(d: Discovered): string {
  const m = d.instance_name.match(/^(.+?)\._ferriscribe\._tcp\.local\.?$/);
  if (m) return m[1];
  return d.host || d.instance_name;
}

/**
 * Pick the most useful address from a set of resolved addresses:
 *   1. RFC1918 IPv4 (192.168/10/172.16-31) — almost always the right answer
 *      on a clinic LAN
 *   2. Other IPv4 (e.g. 100.x Tailscale CGNAT, public-routable)
 *   3. IPv6 ULA (fc/fd) or globally-routable
 *   4. IPv6 link-local (fe80::, last resort — usually unreachable across hosts)
 */
export function bestFrom(addresses: string[]): string | null {
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

/**
 * mDNS browsers fire one ServiceResolved event per interface, so the same
 * logical office server appears N times with overlapping address sets.
 * Dedupe by instance_name (unique per service registration) and merge
 * addresses across events so the picker has every candidate.
 *
 * The same logical office server can also surface via both mDNS and the
 * Tailscale peer probe — when that happens we merge the LAN addresses
 * (`addresses`) and the tailnet addresses (`tailscale_addresses`) into a
 * single entry, keeping each channel's set separate so the slot routing in
 * `pairDiscovered` is correct.
 */
function dedupeDiscovered(discovered: Discovered[]): Discovered[] {
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- transient dedup map, not reactive state
  const seen = new Map<string, Discovered>();
  for (const d of discovered) {
    const ex = seen.get(d.instance_name);
    if (!ex) {
      seen.set(d.instance_name, {
        ...d,
        addresses: [...d.addresses],
        tailscale_addresses: [...(d.tailscale_addresses ?? [])],
      });
    } else {
      for (const a of d.addresses) if (!ex.addresses.includes(a)) ex.addresses.push(a);
      const tsList = (ex.tailscale_addresses ??= []);
      for (const a of d.tailscale_addresses ?? []) if (!tsList.includes(a)) tsList.push(a);
    }
  }
  return Array.from(seen.values());
}

/**
 * Shared client-side pairing state machine for the settings panel
 * (ClientPair.svelte) and the onboarding wizard (StepPair.svelte), which
 * previously carried ~150 duplicated lines of this logic. Svelte 5 runes in
 * a `.svelte.ts` module keep the reactivity; callers reach reactive values
 * through the returned getters/setters.
 *
 * `onPaired` runs after a successful pair (the settings panel reloads its
 * paired-connection display; the wizard advances to the next step).
 */
export function usePairing(onPaired?: () => void | Promise<void>) {
  // `discovered` MUST be $state — rescan() reassigns it and the deduped
  // $derived tracks it. A plain `let` made the discovered-server list
  // permanently empty (the dedup only ran once at mount).
  let discovered = $state<Discovered[]>([]);
  let scanning = $state(false);
  let pasteUrl = $state('');
  let label = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  const deduped = $derived.by(() => dedupeDiscovered(discovered));

  /** Pre-fill the label from the OS hostname so the office server always
   * sees a meaningful computer name. Failure is non-fatal — the user can
   * still type a label manually. */
  async function prefillLabel() {
    if (label) return;
    try {
      label = await suggestedClientLabel();
    } catch {
      // ignore — leave the input empty for the user to fill
    }
  }

  async function rescan() {
    scanning = true;
    discovered = [];
    try {
      // Run mDNS (LAN broadcast) and Tailscale-peer probes in parallel.
      // mDNS sees servers on the same physical network; Tailscale probing
      // sees servers reachable via the tailnet overlay. Either is valid;
      // the dedupe step (by instance_name) merges them into one entry per
      // logical office server when both paths succeed.
      const [lan, ts] = await Promise.all([
        invoke<Discovered[]>('discover_servers', { timeoutMs: 3000 }).catch(() => []),
        invoke<Discovered[]>('discover_via_tailscale', { timeoutMs: 3000 }).catch(() => []),
      ]);
      discovered = [...lan, ...ts];
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
      let tokenLabel = label.trim();
      if (!tokenLabel) {
        try {
          tokenLabel = (await suggestedClientLabel()).trim();
        } catch {
          tokenLabel = '';
        }
      }
      if (!tokenLabel) {
        error = 'Please enter a label for this computer.';
        busy = false;
        return;
      }
      await invoke('pair_with_server', {
        lan,
        tailscale,
        ports,
        code,
        label: tokenLabel,
      });
      await settings.load();
      await onPaired?.();
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
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- transient parse of the paste box, not reactive state
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
    const portOrError = (v: number, label: string): string | null =>
      !Number.isFinite(v) || v <= 0 || v > 65535 ? label : null;
    const opErr = portOrError(op, 'op');
    const wpErr = portOrError(wp, 'wp');
    if (opErr || wpErr) {
      error = `Pairing URL has an out-of-range port (${opErr ?? wpErr}).`;
      return;
    }

    // Optional ports: present-but-invalid is an error (a truncated or
    // hand-mangled URL), absent stays null. NaN/0/negative/>65535 all
    // reject — ports outside the u16 range can never be dialed.
    const optionalPort = (raw: string | null): number | null | 'invalid' => {
      if (raw === null) return null;
      const v = parseInt(raw, 10);
      return Number.isFinite(v) && v > 0 && v <= 65535 ? v : ('invalid' as const);
    };
    const lp = optionalPort(u.searchParams.get('lp'));
    const mp = optionalPort(u.searchParams.get('mp'));
    const vp = optionalPort(u.searchParams.get('vp'));
    if (lp === 'invalid' || mp === 'invalid' || vp === 'invalid') {
      error = 'Pairing URL has an out-of-range optional port (lp/mp/vp).';
      return;
    }

    // `lan=` (empty value) is not an address — treat as absent so it can't
    // reach the pair handshake as an empty-string host.
    const lanEmpty = lan === '';
    if ((!lan || lanEmpty) && !ts) {
      error = 'No reachable address in URL';
      return;
    }
    pairManual(lanEmpty ? null : lan, ts, {
      ollama: op,
      whisper: wp,
      pairing: pp,
      lmstudio: lp,
      omlx: mp,
      vocab: vp,
    }, code);
  }

  function pairDiscovered(d: Discovered) {
    // Route discovery-channel addresses into their semantically correct
    // RemoteEndpoint slots: mDNS-learned addresses go to `lan`,
    // Tailscale-learned addresses (e.g. MagicDNS hostnames like
    // `mac.tail161478.ts.net`) go to `tailscale`. `pair_with_server`
    // already handles all three combinations (lan-only, ts-only, both).
    const lan = bestFrom(d.addresses);
    const tailscale = (d.tailscale_addresses ?? [])[0] ?? null;
    if (!lan && !tailscale) {
      error = 'No reachable address for this server.';
      return;
    }
    const ports: PairPorts = {
      ollama: d.ports.ollama ?? 11435,
      whisper: d.ports.whisper ?? 8081,
      pairing: d.ports.pairing ?? 11436,
      lmstudio: d.ports.lmstudio ?? null,
      omlx: d.ports.omlx ?? null,
      vocab: d.ports.vocab ?? null,
    };
    const code = prompt('Enter the 6-digit code from the office server.') ?? '';
    if (!code) return;
    pairManual(lan, tailscale, ports, code);
  }

  return {
    get discovered() {
      return discovered;
    },
    get deduped() {
      return deduped;
    },
    get scanning() {
      return scanning;
    },
    get busy() {
      return busy;
    },
    get error() {
      return error;
    },
    set error(v: string | null) {
      error = v;
    },
    get label() {
      return label;
    },
    set label(v: string) {
      label = v;
    },
    get pasteUrl() {
      return pasteUrl;
    },
    set pasteUrl(v: string) {
      pasteUrl = v;
    },
    prefillLabel,
    rescan,
    pairManual,
    pairFromUrl,
    pairDiscovered,
  };
}
