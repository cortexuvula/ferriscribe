// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { usePairing, friendlyName, bestFrom, type Discovered } from './usePairing.svelte';

// ── Mocks ───────────────────────────────────────────────────────────────────

const mockInvoke = vi.fn();
const mockSuggestedLabel = vi.fn();
const mockSettingsLoad = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...args),
}));

vi.mock('../api/sharing', () => ({
  suggestedClientLabel: () => mockSuggestedLabel(),
}));

vi.mock('../stores/settings.svelte', () => ({
  settings: { load: () => mockSettingsLoad() },
}));

// ── Fixtures ────────────────────────────────────────────────────────────────

function makeDiscovered(overrides: Partial<Discovered>): Discovered {
  return {
    instance_name: 'Clinic Server._ferriscribe._tcp.local.',
    host: 'clinics-host.local',
    addresses: [],
    tailscale_addresses: [],
    ports: { ollama: null, whisper: null, lmstudio: null, pairing: null, vocab: null },
    version: '1',
    ...overrides,
  };
}

// ── Pure helpers ────────────────────────────────────────────────────────────

describe('friendlyName', () => {
  it('strips the mDNS suffix', () => {
    expect(friendlyName(makeDiscovered({ instance_name: 'Room 6 Server._ferriscribe._tcp.local.' }))).toBe(
      'Room 6 Server',
    );
  });

  it('falls back to host, then instance name', () => {
    const d = makeDiscovered({ instance_name: 'raw-name', host: 'host.local' });
    expect(friendlyName(d)).toBe('host.local');
    expect(friendlyName(makeDiscovered({ instance_name: 'raw-name', host: '' }))).toBe('raw-name');
  });
});

describe('bestFrom', () => {
  it('prefers RFC1918 IPv4 over CGNAT, IPv6 ULA, and link-local', () => {
    expect(bestFrom([])).toBeNull();
    expect(bestFrom(['fe80::1'])).toBe('fe80::1');
    expect(
      bestFrom(['fe80::1', 'fd00::2', '100.64.0.5', '192.168.1.20', '10.0.0.4', '8.8.8.8']),
    ).toBe('192.168.1.20');
    // No RFC1918 → other IPv4 (Tailscale CGNAT / public) wins over any IPv6.
    expect(bestFrom(['fd00::2', '100.64.0.5'])).toBe('100.64.0.5');
  });
});

// ── State machine ───────────────────────────────────────────────────────────

describe('usePairing', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSuggestedLabel.mockResolvedValue('Test MacBook');
    mockSettingsLoad.mockResolvedValue(undefined);
    mockInvoke.mockReset();
  });

  it('rescan merges mDNS and Tailscale discoveries into one deduped entry', async () => {
    // mDNS fires per interface: same instance, different address sets.
    const iface1 = makeDiscovered({ addresses: ['192.168.1.50'] });
    const iface2 = makeDiscovered({ addresses: ['192.168.1.51', '192.168.1.50'] });
    const tsProbe = makeDiscovered({
      addresses: [],
      tailscale_addresses: ['clinic.tail161478.ts.net'],
    });
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'discover_servers') return Promise.resolve([iface1, iface2]);
      if (cmd === 'discover_via_tailscale') return Promise.resolve([tsProbe]);
      return Promise.reject(new Error(`unexpected invoke: ${cmd}`));
    });

    const pairing = usePairing();
    await pairing.rescan();

    expect(pairing.scanning).toBe(false);
    expect(pairing.deduped).toHaveLength(1);
    const merged = pairing.deduped[0]!;
    // LAN sets merged across interface events; tailnet set kept separate so
    // pairDiscovered routes it into the `tailscale` slot.
    expect(merged.addresses.sort()).toEqual(['192.168.1.50', '192.168.1.51']);
    expect(merged.tailscale_addresses).toEqual(['clinic.tail161478.ts.net']);
  });

  it('pairFromUrl rejects non-FerriScribe URLs without invoking', async () => {
    const pairing = usePairing();
    pairing.pasteUrl = 'https://evil.example/pair?code=1';
    pairing.pairFromUrl();

    expect(pairing.error).toContain('Not a FerriScribe pairing URL');
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('pairFromUrl pairs with parsed ports and fires onPaired', async () => {
    mockInvoke.mockResolvedValue(undefined);
    const onPaired = vi.fn();
    const pairing = usePairing(onPaired);

    pairing.pasteUrl = 'ferriscribe://pair?lan=192.168.1.9&ts=clinic.ts.net&code=123456&pp=11436&op=11435&wp=8081&vp=9000';
    pairing.pairFromUrl();
    // pairManual runs async — let the microtasks drain.
    await vi.waitFor(() => expect(pairing.busy).toBe(false));

    expect(mockInvoke).toHaveBeenCalledWith('pair_with_server', {
      lan: '192.168.1.9',
      tailscale: 'clinic.ts.net',
      ports: { ollama: 11435, whisper: 8081, pairing: 11436, lmstudio: null, vocab: 9000 },
      code: '123456',
      label: 'Test MacBook', // pre-filled from suggestedClientLabel
    });
    expect(mockSettingsLoad).toHaveBeenCalled();
    expect(onPaired).toHaveBeenCalled();
    expect(pairing.error).toBeNull();
  });

  it('pairDiscovered routes addresses into lan/tailscale slots with default ports', async () => {
    vi.stubGlobal('prompt', vi.fn(() => '654321'));
    mockInvoke.mockResolvedValue(undefined);
    const pairing = usePairing();

    pairing.pairDiscovered(
      makeDiscovered({
        addresses: ['192.168.1.60', 'fe80::1'],
        tailscale_addresses: ['server.tail161478.ts.net'],
        ports: { ollama: null, whisper: null, lmstudio: null, pairing: null, vocab: null },
      }),
    );
    await vi.waitFor(() => expect(pairing.busy).toBe(false));

    expect(mockInvoke).toHaveBeenCalledWith(
      'pair_with_server',
      expect.objectContaining({
        lan: '192.168.1.60',
        tailscale: 'server.tail161478.ts.net',
        ports: {
          ollama: 11435, // server advertised nothing — client defaults
          whisper: 8081,
          pairing: 11436,
          lmstudio: null,
          vocab: null,
        },
        code: '654321',
      }),
    );
    vi.unstubAllGlobals();
  });
});
