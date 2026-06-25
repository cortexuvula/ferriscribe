import { describe, it, expect } from 'vitest';
import { classifyEndpoint, isLocalOrAllowed, type EndpointKind } from './endpointPolicy';

describe('classifyEndpoint', () => {
  const cases: Array<[string, EndpointKind]> = [
    // Loopback
    ['localhost', 'Loopback'],
    ['LOCALHOST', 'Loopback'],
    ['127.0.0.1', 'Loopback'],
    ['::1', 'Loopback'],

    // RFC1918
    ['10.0.0.0', 'LanRfc1918'],
    ['10.255.255.255', 'LanRfc1918'],
    ['172.16.0.0', 'LanRfc1918'],
    ['172.31.255.255', 'LanRfc1918'],
    ['192.168.1.42', 'LanRfc1918'],

    // Out of RFC1918
    ['9.255.255.255', 'Public'],
    ['172.32.0.0', 'Public'],
    ['192.169.0.0', 'Public'],

    // Link-local
    ['169.254.0.1', 'LinkLocal'],
    ['fe80::1', 'LinkLocal'],

    // Tailscale CGNAT
    ['100.64.0.0', 'Tailscale'],
    ['100.127.255.255', 'Tailscale'],
    ['100.128.0.0', 'Public'],

    // Tailscale MagicDNS (*.ts.net) — regression: these were classifying as
    // 'Unknown' because the TS mirror didn't have the .ts.net suffix check,
    // causing false "public address" warnings for remote Tailscale clients.
    ['mac.tail161478.ts.net', 'Tailscale'],
    ['server.ts.net', 'Tailscale'],
    ['MAC.TAILNET.TS.NET', 'Tailscale'],
    ['mac.tail161478.ts.net.', 'Tailscale'], // trailing dot FQDN
    // Must NOT false-match these:
    ['ts.net', 'Unknown'], // bare apex, no leading dot
    ['fakets.net', 'Unknown'], // ends with "ts.net" not ".ts.net"
    ['notreally.ts.net.example.com', 'Unknown'], // .ts.net is mid-string

    // ULA
    ['fd00::1', 'Ula'],
    ['fc00::1', 'Ula'],

    // mDNS / non-routable TLDs
    ['clinic.local', 'Mdns'],
    ['box.lan', 'Mdns'],
    ['server.internal', 'Mdns'],
    ['host.home.arpa', 'Mdns'],
    ['CLINIC.LOCAL', 'Mdns'],

    // Public / Unknown
    ['8.8.8.8', 'Public'],
    ['api.openai.com', 'Unknown'],
    ['clinic.example.com', 'Unknown'],
  ];

  for (const [host, expected] of cases) {
    it(`classifies "${host}" as ${expected}`, () => {
      expect(classifyEndpoint(host)).toBe(expected);
    });
  }
});

describe('isLocalOrAllowed', () => {
  it('accepts local kinds regardless of allow_public', () => {
    for (const host of ['localhost', '192.168.1.42', '100.64.0.1', 'clinic.local']) {
      expect(isLocalOrAllowed(host, false)).toBe(true);
      expect(isLocalOrAllowed(host, true)).toBe(true);
    }
  });

  it('rejects public/unknown unless allow_public', () => {
    for (const host of ['api.openai.com', '8.8.8.8']) {
      expect(isLocalOrAllowed(host, false)).toBe(false);
      expect(isLocalOrAllowed(host, true)).toBe(true);
    }
  });

  it('accepts empty host (skipped)', () => {
    expect(isLocalOrAllowed('', false)).toBe(true);
  });

  // Regression: remote clients pairing via Tailscale MagicDNS were seeing
  // a false "public address" warning because *.ts.net classified as Unknown.
  // Tailscale is a trusted local-network kind — must be allowed without
  // allowPublic, both for CGNAT IPs and MagicDNS hostnames.
  it('accepts Tailscale MagicDNS without allow_public', () => {
    expect(isLocalOrAllowed('mac.tail161478.ts.net', false)).toBe(true);
    expect(isLocalOrAllowed('server.ts.net', false)).toBe(true);
  });

  it('accepts Tailscale CGNAT without allow_public', () => {
    expect(isLocalOrAllowed('100.64.0.1', false)).toBe(true);
  });
});
