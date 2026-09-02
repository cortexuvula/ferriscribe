import { describe, expect, it } from 'vitest';
import { officeServedHint, providerLabel, providerStartHint } from './providerHints';

describe('providerHints', () => {
  it('labels known providers and passes unknown ids through', () => {
    expect(providerLabel('ollama')).toBe('Ollama');
    expect(providerLabel('lmstudio')).toBe('LM Studio');
    expect(providerLabel('omlx')).toBe('oMLX');
    expect(providerLabel('future-provider')).toBe('future-provider');
  });

  it('gives each known provider a concrete start instruction', () => {
    expect(providerStartHint('ollama')).toContain('ollama serve');
    expect(providerStartHint('lmstudio')).toContain('Start Server');
    expect(providerStartHint('omlx')).toContain('oMLX app');
  });

  it('falls back to a generic check for unknown providers', () => {
    expect(providerStartHint('future-provider')).toContain('running');
  });

  it('office hint names the provider and the office machine', () => {
    const hint = officeServedHint('ollama');
    expect(hint).toContain('Ollama');
    expect(hint).toContain('office server');
    expect(hint).toContain('Refresh');
  });
});
