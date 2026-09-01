import { describe, it, expect } from 'vitest';
import { latestTokensPerSecond, generationProgressText } from './generationStats';
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

  it('breaks timestamp ties by doc-type order (later wins)', () => {
    const metadata = {
      generation_stats: {
        soap: stat(10, '2026-08-16T11:00:00Z'),
        peer_discussion: stat(25, '2026-08-16T11:00:00Z'),
      },
    } as Recording['metadata'];
    expect(latestTokensPerSecond(metadata)).toBe(25);
  });
});

describe('generationProgressText', () => {
  it('formats tokens and tok/s for an in-flight generation', () => {
    expect(generationProgressText({ tokens: 412, elapsed_ms: 20000, tokens_per_second: 20.6 }))
      .toBe('Generating… 412 tokens · 20.6 tok/s');
  });

  it('drops decimals at >=100 tok/s', () => {
    expect(generationProgressText({ tokens: 1500, elapsed_ms: 12000, tokens_per_second: 125 }))
      .toBe('Generating… 1500 tokens · 125 tok/s');
  });

  it('handles zero-token/zero-rate startup stats', () => {
    expect(generationProgressText({ tokens: 0, elapsed_ms: 0, tokens_per_second: 0 }))
      .toBe('Generating… 0 tokens · 0.0 tok/s');
  });

  it('omits the rate when it is not finite', () => {
    expect(generationProgressText({ tokens: 5, elapsed_ms: 1000, tokens_per_second: Number.NaN }))
      .toBe('Generating… 5 tokens · ');
  });
});
