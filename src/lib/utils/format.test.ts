import { describe, it, expect } from 'vitest';
import { formatDuration, formatTimestamp, formatDate } from './format';

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
