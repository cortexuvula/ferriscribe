import { describe, expect, it, vi } from 'vitest';

// Hoisted file-wide: only the getSpecialtyPackPrompt wrapper below invokes;
// the pure helpers under test never touch the Tauri bridge.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import type { SpecialtyPackInfo } from './specialty';
import {
  activePack,
  activeSource,
  conflictingDocTypes,
  packProvides,
  sourceLabel,
} from '../utils/specialtyPacks';

function pack(overrides: Partial<SpecialtyPackInfo> = {}): SpecialtyPackInfo {
  return {
    id: 'psychiatry',
    name: 'Psychiatry',
    version: '1.0.0',
    description: 'test',
    icon: null,
    source: 'bundled',
    provided_prompts: ['soap'],
    ...overrides,
  };
}

describe('activePack', () => {
  it('returns null when no specialty is selected', () => {
    expect(activePack([pack()], null)).toBeNull();
    expect(activePack([pack()], undefined)).toBeNull();
    expect(activePack([pack()], '')).toBeNull();
    expect(activePack([pack()], '   ')).toBeNull();
  });

  it('finds the selected usable pack', () => {
    const psychiatry = pack();
    expect(activePack([psychiatry], 'psychiatry')).toBe(psychiatry);
  });

  it('ignores broken packs and unknown ids', () => {
    const broken = pack({ id: 'ghost', error: 'manifest.json is missing' });
    expect(activePack([broken], 'ghost')).toBeNull();
    expect(activePack([], 'psychiatry')).toBeNull();
  });
});

describe('packProvides', () => {
  it('is true only for provided doc types', () => {
    const p = pack({ provided_prompts: ['soap', 'referral'] });
    expect(packProvides(p, 'soap')).toBe(true);
    expect(packProvides(p, 'referral')).toBe(true);
    expect(packProvides(p, 'synopsis')).toBe(false);
    expect(packProvides(null, 'soap')).toBe(false);
  });
});

describe('activeSource', () => {
  it('custom prompt wins over a pack', () => {
    const p = pack({ provided_prompts: ['soap'] });
    expect(activeSource('soap', 'my custom prompt', p)).toBe('custom');
    // Empty string counts as absent.
    expect(activeSource('soap', '', p)).toBe('pack');
    expect(activeSource('soap', null, p)).toBe('pack');
  });

  it('falls back to default for doc types the pack does not provide', () => {
    const p = pack({ provided_prompts: ['soap'] });
    expect(activeSource('synopsis', null, p)).toBe('default');
    expect(activeSource('soap', null, p)).toBe('pack');
    expect(activeSource('soap', null, null)).toBe('default');
  });
});

describe('sourceLabel', () => {
  it('labels the three tiers', () => {
    expect(sourceLabel('custom', null)).toBe('custom');
    expect(sourceLabel('pack', pack())).toBe('Psychiatry (+ safety block)');
    expect(sourceLabel('default', null)).toBe('default');
  });
});

describe('conflictingDocTypes', () => {
  it('flags doc types where a custom prompt silently overrides the pack', () => {
    const p = pack({ provided_prompts: ['soap', 'referral'] });
    const conflicts = conflictingDocTypes(
      { soap: 'custom soap', referral: null, synopsis: 'custom synopsis' },
      p,
    );
    // soap: custom + pack provides → conflict.
    // referral: no custom → no conflict.
    // synopsis: custom but pack does NOT provide synopsis → falls back to
    // default anyway, so there is nothing the pack could have served.
    expect(conflicts).toEqual(['soap']);
  });

  it('returns nothing without a pack or custom prompts', () => {
    const p = pack({ provided_prompts: ['soap'] });
    expect(conflictingDocTypes({}, p)).toEqual([]);
    expect(conflictingDocTypes({ soap: 'custom' }, null)).toEqual([]);
  });
});

describe('getSpecialtyPackPrompt', () => {
  it('invokes get_specialty_pack_prompt with camelCase docType and passes null through', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const invokeMock = vi.mocked(invoke);
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(null);
    const { getSpecialtyPackPrompt } = await import('./specialty');

    await getSpecialtyPackPrompt('peer_discussion');
    expect(invokeMock).toHaveBeenCalledWith('get_specialty_pack_prompt', {
      docType: 'peer_discussion',
    });

    invokeMock.mockResolvedValue('PACK BODY + safety block');
    await expect(getSpecialtyPackPrompt('soap')).resolves.toBe('PACK BODY + safety block');
  });
});

describe('listSpecialtyPacks', () => {
  it('invokes list_specialty_packs with no arguments', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    const invokeMock = vi.mocked(invoke);
    invokeMock.mockReset();
    invokeMock.mockResolvedValue([]);
    const { listSpecialtyPacks } = await import('./specialty');

    await listSpecialtyPacks();
    expect(invokeMock).toHaveBeenCalledWith('list_specialty_packs');
  });
});
