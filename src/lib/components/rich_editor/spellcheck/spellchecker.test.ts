// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';

// We can't easily mock the real dictionary in unit tests. Instead, verify
// that the wrapper behaves correctly when the underlying nspell is stubbed.
// Real-dictionary smoke-test is left to integration (running the app).

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'user_dict_list') return ['atenolol'];
    if (cmd === 'user_dict_add') return true;
    return null;
  }),
}));

vi.mock('dictionary-en/index.aff?url', () => ({ default: '/test.aff' }));
vi.mock('dictionary-en/index.dic?url', () => ({ default: '/test.dic' }));

vi.mock('nspell', () => ({
  default: () => ({
    correct: (w: string) => ['cat', 'dog', 'patient'].includes(w),
    suggest: (w: string) => (w === 'paitent' ? ['patient', 'patent'] : []),
  }),
}));

// Stub global fetch since the wrapper fetches the dict URLs.
beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => ({ text: async () => '' })),
  );
});

describe('Spellchecker wrapper', () => {
  it('returns true before load (degraded mode)', async () => {
    const { getSpellchecker } = await import('./spellchecker');
    const s = getSpellchecker();
    expect(s.ready).toBe(false);
    expect(s.check('xxxyz')).toBe(true);
  });

  it('flags unknown words and returns suggestions after load', async () => {
    const { getSpellchecker } = await import('./spellchecker');
    const s = getSpellchecker() as ReturnType<typeof getSpellchecker> & { load: () => Promise<void> };
    await s.load();
    expect(s.ready).toBe(true);
    expect(s.check('cat')).toBe(true);
    expect(s.check('paitent')).toBe(false);
    expect(s.suggest('paitent')).toEqual(['patient', 'patent']);
  });

  it('accepts words from the persisted user dictionary', async () => {
    const { getSpellchecker } = await import('./spellchecker');
    const s = getSpellchecker() as ReturnType<typeof getSpellchecker> & { load: () => Promise<void> };
    await s.load();
    expect(s.check('atenolol')).toBe(true); // present in mocked user_dict_list
  });

  it('addToUserDict persists and unflags the word', async () => {
    const { getSpellchecker } = await import('./spellchecker');
    const s = getSpellchecker() as ReturnType<typeof getSpellchecker> & { load: () => Promise<void> };
    await s.load();
    expect(s.check('lisinopril')).toBe(false);
    await s.addToUserDict('lisinopril');
    expect(s.check('lisinopril')).toBe(true);
  });

  it('ignoreInSession unflags the word for the session', async () => {
    const { getSpellchecker } = await import('./spellchecker');
    const s = getSpellchecker() as ReturnType<typeof getSpellchecker> & { load: () => Promise<void> };
    await s.load();
    expect(s.check('xxxyz')).toBe(false);
    s.ignoreInSession('xxxyz');
    expect(s.check('xxxyz')).toBe(true);
  });
});
