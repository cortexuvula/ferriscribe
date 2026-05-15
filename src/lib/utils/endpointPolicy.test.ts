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

    // Tailscale
    ['100.64.0.0', 'Tailscale'],
    ['100.127.255.255', 'Tailscale'],
    ['100.128.0.0', 'Public'],

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
});
