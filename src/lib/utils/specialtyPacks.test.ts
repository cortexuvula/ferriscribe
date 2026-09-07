import { describe, it, expect } from 'vitest';

import type { SpecialtyPackInfo } from '../api/specialty';
import {
  CUSTOM_PROMPT_FIELD,
  activePack,
  activeSource,
  conflictingDocTypes,
  packProvides,
  sourceLabel,
} from './specialtyPacks';

function pack(overrides: Partial<SpecialtyPackInfo> = {}): SpecialtyPackInfo {
  return {
    id: 'psychiatry',
    name: 'Psychiatry',
    version: '1.0.0',
    description: 'Psychiatric-interview SOAP notes.',
    icon: null,
    source: 'bundled',
    provided_prompts: ['soap'],
    ...overrides,
  };
}

describe('activePack', () => {
  it('returns null for unset, empty, and whitespace-only selections', () => {
    expect(activePack([pack()], null)).toBeNull();
    expect(activePack([pack()], '')).toBeNull();
    expect(activePack([pack()], '   ')).toBeNull();
  });

  it('resolves the selected usable pack and ignores broken or unknown ones', () => {
    const good = pack();
    const broken = pack({ id: 'broken', error: 'manifest.json is missing' });
    expect(activePack([good, broken], 'psychiatry')).toBe(good);
    expect(activePack([good, broken], 'broken')).toBeNull();
    expect(activePack([good], 'no-such-pack')).toBeNull();
  });
});

describe('packProvides', () => {
  it('is true only for doc types the pack carries an artifact for', () => {
    const p = pack();
    expect(packProvides(p, 'soap')).toBe(true);
    expect(packProvides(p, 'referral')).toBe(false);
  });

  it('is false without a pack', () => {
    expect(packProvides(null, 'soap')).toBe(false);
  });
});

describe('activeSource — mirrors the Rust builders precedence', () => {
  it('custom free-text beats the pack, the pack beats the default', () => {
    const p = pack();
    expect(activeSource('soap', 'MY CUSTOM PROMPT', p)).toBe('custom');
    expect(activeSource('soap', null, p)).toBe('pack');
    expect(activeSource('soap', null, null)).toBe('default');
    // A pack that lacks the doc type falls through to the default.
    expect(activeSource('referral', null, p)).toBe('default');
    // An empty-string custom prompt is not a real override (Rust filters
    // empty custom prompts before the pack tier too).
    expect(activeSource('soap', '', p)).toBe('pack');
  });
});

describe('sourceLabel', () => {
  it('labels the three tiers', () => {
    expect(sourceLabel('custom', null)).toBe('custom');
    expect(sourceLabel('default', null)).toBe('default');
    expect(sourceLabel('pack', pack())).toBe('Psychiatry (+ safety block)');
    expect(sourceLabel('pack', null)).toBe('specialty pack (+ safety block)');
  });
});

describe('conflictingDocTypes', () => {
  it('flags doc types where a stored custom prompt silently overrides the pack', () => {
    const conflicts = conflictingDocTypes(
      {
        soap: 'custom soap text',
        referral: null,
        letter: '',
        synopsis: 'custom synopsis',
        peer_discussion: null,
      },
      pack({ provided_prompts: ['soap', 'synopsis', 'peer_discussion'] })
    );
    expect(conflicts).toEqual(['soap', 'synopsis']);
  });

  it('returns nothing without a pack or without custom prompts', () => {
    expect(conflictingDocTypes({ soap: 'x' }, null)).toEqual([]);
    expect(conflictingDocTypes({ soap: null }, pack())).toEqual([]);
  });
});

describe('CUSTOM_PROMPT_FIELD', () => {
  it('maps every doc type to its config key', () => {
    expect(CUSTOM_PROMPT_FIELD.soap).toBe('custom_soap_prompt');
    expect(CUSTOM_PROMPT_FIELD.referral).toBe('custom_referral_prompt');
    expect(CUSTOM_PROMPT_FIELD.letter).toBe('custom_letter_prompt');
    expect(CUSTOM_PROMPT_FIELD.synopsis).toBe('custom_synopsis_prompt');
    expect(CUSTOM_PROMPT_FIELD.peer_discussion).toBe('custom_peer_discussion_prompt');
  });
});
