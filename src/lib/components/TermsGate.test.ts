/**
 * TermsGate / TermsOfServiceModal — unit-level tests.
 *
 * @testing-library/svelte is not installed and the vitest environment is
 * "node", so full component rendering isn't available. These tests cover:
 *   - the shared terms module (build-time raw import resolves and carries
 *     the real document, including the acceptance clause the gate enforces);
 *   - the gate-visibility derivation mirrored from App.svelte;
 *   - the acceptance timestamp the gate writes (valid ISO-8601).
 * If the component logic changes, the mirrors here must change to match.
 */
import { describe, it, expect } from 'vitest';
import { TERMS_OF_SERVICE_TEXT } from '../terms';

// Mirror of App.svelte's gate derivation.
function gateVisible(tosAcceptedAt: string | null): boolean {
  return tosAcceptedAt == null;
}

// Mirror of TermsGate.svelte's accept() payload.
function acceptanceTimestamp(now = new Date()): string {
  return now.toISOString();
}

describe('terms module (build-time raw import)', () => {
  it('imports the real TERMS_OF_SERVICE.md', () => {
    expect(TERMS_OF_SERVICE_TEXT).toContain('FERRISCRIBE TERMS OF SERVICE');
    expect(TERMS_OF_SERVICE_TEXT).toMatch(/Last updated:/);
  });

  it('contains the clauses the gate and About hint rely on', () => {
    // The plain-text document hard-wraps paragraphs; compare against a
    // whitespace-normalized copy so assertions match the prose, not the wrap.
    const flat = TERMS_OF_SERVICE_TEXT.replace(/\s+/g, ' ');
    // The gate's not-closable design rests on this clause.
    expect(flat).toContain('If you do not accept, do not use the software');
    // §13.2 justifies a single acceptance timestamp (no re-gate on amendment).
    expect(flat).toContain('continued use');
  });
});

describe('gate visibility (mirror of App.svelte)', () => {
  it('shows the gate until accepted, never after', () => {
    expect(gateVisible(null)).toBe(true);
    expect(gateVisible('2026-08-28T17:00:00.000Z')).toBe(false);
  });
});

describe('acceptance record (mirror of TermsGate accept())', () => {
  it('writes a valid ISO-8601 timestamp', () => {
    const ts = acceptanceTimestamp(new Date('2026-08-28T16:49:06.606Z'));
    expect(ts).toBe('2026-08-28T16:49:06.606Z');
    expect(Number.isNaN(Date.parse(ts))).toBe(false);
  });
});
