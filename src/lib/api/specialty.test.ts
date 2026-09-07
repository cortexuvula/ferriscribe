import { describe, expect, it } from 'vitest';

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
