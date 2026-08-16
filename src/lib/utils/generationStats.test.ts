import { describe, it, expect } from 'vitest';
import { latestTokensPerSecond } from './generationStats';
import type { Recording } from '../types';

const stat = (tokens_per_second: number, generated_at: string) => ({
  provider: 'ollama',
  model: 'llama3',
  prompt_tokens: 10,
  completion_tokens: 100,
  duration_ms: 1000,
  tokens_per_second,
  generated_at,
});

describe('latestTokensPerSecond', () => {
  it('returns null for null metadata', () => {
    expect(latestTokensPerSecond(null)).toBeNull();
  });

  it('returns null when generation_stats is absent', () => {
    const metadata = { context: 'x' } as Recording['metadata'];
    expect(latestTokensPerSecond(metadata)).toBeNull();
  });

  it('picks the newest generated_at across doc types', () => {
    const metadata = {
      generation_stats: {
        soap: stat(10, '2026-08-16T10:00:00Z'),
        referral: stat(42.5, '2026-08-16T11:00:00Z'),
        letter: stat(20, '2026-08-16T09:00:00Z'),
      },
    } as Recording['metadata'];
    expect(latestTokensPerSecond(metadata)).toBe(42.5);
  });

  it('skips entries with unparseable generated_at', () => {
    const metadata = {
      generation_stats: {
        soap: stat(10, 'not-a-date'),
        referral: stat(30, '2026-08-16T11:00:00Z'),
      },
    } as Recording['metadata'];
    expect(latestTokensPerSecond(metadata)).toBe(30);
  });
});
