import { describe, it, expect } from 'vitest';
import {
  formatDuration,
  formatMinsSecs,
  formatTimestamp,
  formatDate,
  formatTokensPerSecond,
} from './format';

describe('formatDuration', () => {
  it('formats null as placeholder', () => {
    expect(formatDuration(null)).toBe('--:--');
  });

  it('formats zero as 00:00', () => {
    expect(formatDuration(0)).toBe('00:00');
  });

  it('formats seconds under a minute', () => {
    expect(formatDuration(7)).toBe('00:07');
  });

  it('formats exact minutes', () => {
    expect(formatDuration(120)).toBe('02:00');
  });

  it('formats minutes + seconds', () => {
    expect(formatDuration(125)).toBe('02:05');
  });

  it('truncates fractional seconds', () => {
    expect(formatDuration(90.9)).toBe('01:30');
  });

  it('handles large values without overflow', () => {
    expect(formatDuration(3600)).toBe('60:00');
  });
});

describe('formatMinsSecs', () => {
  it('formats whole values as M:SS with unpadded minutes', () => {
    expect(formatMinsSecs(0)).toBe('0:00');
    expect(formatMinsSecs(59)).toBe('0:59');
    expect(formatMinsSecs(192)).toBe('3:12');
  });

  it('rounds the total once so minute and second parts agree', () => {
    // Independent rounding previously produced "1:59" here.
    expect(formatMinsSecs(59.4)).toBe('0:59');
    expect(formatMinsSecs(59.6)).toBe('1:00');
    expect(formatMinsSecs(119.6)).toBe('2:00');
  });
});

describe('formatTimestamp', () => {
  it('produces a non-empty string for a valid ISO date', () => {
    const out = formatTimestamp('2026-06-25T14:30:00Z');
    expect(out.length).toBeGreaterThan(0);
  });

  it('produces a string containing digits (locale-dependent but always has numbers)', () => {
    const out = formatTimestamp('2026-06-25T14:30:00Z');
    expect(/\d/.test(out)).toBe(true);
  });
});

describe('formatDate', () => {
  it('produces a non-empty string for a valid ISO date', () => {
    const out = formatDate('2026-06-25T14:30:00Z');
    expect(out.length).toBeGreaterThan(0);
  });

  it('includes the year', () => {
    const out = formatDate('2026-06-25T14:30:00Z');
    expect(out).toContain('2026');
  });
});

describe('formatTokensPerSecond', () => {
  it('formats null/undefined as empty string', () => {
    expect(formatTokensPerSecond(null)).toBe('');
    expect(formatTokensPerSecond(undefined)).toBe('');
  });

  it('formats small values with one decimal', () => {
    expect(formatTokensPerSecond(41.52)).toBe('41.5 tok/s');
    expect(formatTokensPerSecond(7.25)).toBe('7.3 tok/s');
  });

  it('formats values of 100 or more with no decimals', () => {
    expect(formatTokensPerSecond(99.9)).toBe('99.9 tok/s');
    expect(formatTokensPerSecond(100)).toBe('100 tok/s');
    expect(formatTokensPerSecond(1234.56)).toBe('1235 tok/s');
  });

  it('rejects non-finite and negative values', () => {
    expect(formatTokensPerSecond(Number.NaN)).toBe('');
    expect(formatTokensPerSecond(Number.POSITIVE_INFINITY)).toBe('');
    expect(formatTokensPerSecond(-5)).toBe('');
  });
});
