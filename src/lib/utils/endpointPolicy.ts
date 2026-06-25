/**
 * TS mirror of Rust's `endpoint_policy` classifier. The Rust side is the
 * source of truth (Settings save and provider construction enforce
 * authoritatively). This helper exists only to render an inline warning in
 * the Settings UI before the user clicks Save.
 *
 * Keep this file in sync with `crates/core/src/endpoint_policy.rs`.
 */

export type EndpointKind =
  | 'Loopback'
  | 'LanRfc1918'
  | 'LinkLocal'
  | 'Tailscale'
  | 'Ula'
  | 'Mdns'
  | 'Public'
  | 'Unknown';

const LOCAL_TLD_SUFFIXES = ['.local', '.lan', '.home.arpa', '.internal'];

function stripBrackets(host: string): string {
  return host.startsWith('[') && host.endsWith(']')
    ? host.slice(1, -1)
    : host;
}

function isIpv4(host: string): { a: number; b: number; c: number; d: number } | null {
  const parts = host.split('.');
  if (parts.length !== 4) return null;
  const nums = parts.map((p) => /^\d+$/.test(p) ? Number(p) : NaN);
  if (nums.some((n) => !Number.isFinite(n) || n < 0 || n > 255)) return null;
  return { a: nums[0], b: nums[1], c: nums[2], d: nums[3] };
}

function classifyIpv4(p: { a: number; b: number; c: number; d: number }): EndpointKind {
  if (p.a === 127) return 'Loopback';
  if (p.a === 169 && p.b === 254) return 'LinkLocal';
  if (p.a === 10) return 'LanRfc1918';
  if (p.a === 172 && p.b >= 16 && p.b <= 31) return 'LanRfc1918';
  if (p.a === 192 && p.b === 168) return 'LanRfc1918';
  if (p.a === 100 && p.b >= 64 && p.b <= 127) return 'Tailscale';
  return 'Public';
}

function classifyIpv6(host: string): EndpointKind | null {
  // Must contain a colon and parse roughly as an IPv6 address.
  // Browser/Node has no built-in IPv6 parser, so we use a regex test that
  // accepts the syntactically valid forms we care about.
  if (!/^[0-9a-fA-F:]+$/.test(host) || !host.includes(':')) return null;

  // Loopback
  if (host === '::1') return 'Loopback';

  // Read the first hex group, accounting for "::" leading.
  // We just care about the high bits of the first 16-bit segment.
  const firstSeg = host.split(':').find((s) => s.length > 0);
  if (!firstSeg) return null;
  const seg0 = parseInt(firstSeg, 16);
  if (!Number.isFinite(seg0)) return null;

  // fe80::/10 → segment & 0xffc0 === 0xfe80
  if ((seg0 & 0xffc0) === 0xfe80) return 'LinkLocal';
  // fc00::/7 → segment & 0xfe00 === 0xfc00
  if ((seg0 & 0xfe00) === 0xfc00) return 'Ula';
  return 'Public';
}

export function classifyEndpoint(host: string): EndpointKind {
  const trimmed = stripBrackets(host);

  // IPv4
  const v4 = isIpv4(trimmed);
  if (v4) return classifyIpv4(v4);

  // IPv6
  const v6 = classifyIpv6(trimmed);
  if (v6) return v6;

  // Hostname
  const lower = trimmed.toLowerCase();
  // Defensive: normalize away any trailing dot(s) so fully-qualified domain
  // names like "foo.ts.net." still match the suffix checks below. Mirrors the
  // Rust source of truth in crates/core/src/endpoint_policy.rs.
  const normalized = lower.replace(/\.+$/, '');
  if (normalized === 'localhost') return 'Loopback';
  // Tailscale MagicDNS: <machine>.<tailnet>.ts.net. Match the FQDN suffix
  // ".ts.net" (with leading dot) so we don't false-positive on things like
  // "fakets.net". This is a static, DNS-free trust signal.
  // MUST stay in sync with the Rust classifier — see validate_accepts_tailscale_magicdns_without_allow_public.
  if (normalized.endsWith('.ts.net')) return 'Tailscale';
  for (const suf of LOCAL_TLD_SUFFIXES) {
    if (normalized.endsWith(suf)) return 'Mdns';
  }
  return 'Unknown';
}

/**
 * Returns true if the host is acceptable given `allowPublic`. An empty
 * host is treated as acceptable (no value yet).
 */
export function isLocalOrAllowed(host: string, allowPublic: boolean): boolean {
  if (host === '') return true;
  const kind = classifyEndpoint(host);
  if (kind === 'Public' || kind === 'Unknown') return allowPublic;
  return true;
}
