import { describe, it, expect } from 'vitest';
import {
  stripIcdPrefix,
  normalizedForms,
  validateIcdCode,
  extractIcdCodesValidated,
  extractIcdDescriptions,
  billingCodesLabel,
} from './icd';

describe('stripIcdPrefix', () => {
  it('strips "ICD-9 Code:" prefix', () => {
    expect(stripIcdPrefix('ICD-9 Code: 401.9')).toBe('401.9');
  });

  it('strips "ICD-9:" prefix', () => {
    expect(stripIcdPrefix('ICD-9: 847.2')).toBe('847.2');
  });

  it('strips "ICD-10 Code:" prefix', () => {
    expect(stripIcdPrefix('ICD-10 Code: Z00.00')).toBe('Z00.00');
  });

  it('strips surrounding brackets', () => {
    expect(stripIcdPrefix('[ICD-9: 250.0]')).toBe('250.0');
  });

  it('strips parentheses', () => {
    expect(stripIcdPrefix('(ICD-9: V70.0)')).toBe('V70.0');
  });

  it('handles lowercase', () => {
    expect(stripIcdPrefix('icd-9: 401.9')).toBe('401.9');
  });
});

describe('normalizedForms', () => {
  it('zero-pads numeric codes with decimal', () => {
    const forms = normalizedForms('1.0');
    expect(forms).toContain('001.0');
    expect(forms).toContain('1.0');
  });

  it('zero-pads integer codes', () => {
    const forms = normalizedForms('42');
    expect(forms).toContain('042');
    expect(forms).toContain('42');
  });

  it('preserves already-padded codes', () => {
    const forms = normalizedForms('001.0');
    expect(forms).toContain('001.0');
  });

  it('does not alter alpha-suffix MSP codes', () => {
    const forms = normalizedForms('01A');
    expect(forms).toEqual(['01A']);
  });

  it('does not alter V-codes', () => {
    const forms = normalizedForms('V70.0');
    expect(forms).toEqual(['V70.0']);
  });
});

describe('validateIcdCode', () => {
  const mspSet = new Set(['401.9', '847.2', '250.0', 'V70.0', '01A', '001.0']);

  it('returns true for a code on the list', () => {
    expect(validateIcdCode('ICD-9: 401.9', mspSet)).toBe(true);
  });

  it('returns false for a code not on the list', () => {
    expect(validateIcdCode('ICD-9: 999.99', mspSet)).toBe(false);
  });

  it('returns null when the set is not loaded', () => {
    expect(validateIcdCode('ICD-9: 401.9', null)).toBeNull();
  });

  it('accepts zero-padded form via normalization', () => {
    // Model emits "1.0", list has "001.0".
    expect(validateIcdCode('ICD-9: 1.0', mspSet)).toBe(true);
  });

  it('validates V-codes', () => {
    expect(validateIcdCode('ICD-9: V70.0', mspSet)).toBe(true);
  });

  it('validates alpha-suffix MSP codes', () => {
    expect(validateIcdCode('ICD-9: 01A', mspSet)).toBe(true);
  });

  it('returns null for stray prose with no digit (extraction noise)', () => {
    // BUG-4 regression: "ICD-9." prose matches the loose regex but has
    // no code body — must render neutral, not as a false billing warning.
    expect(validateIcdCode('ICD-9.', mspSet)).toBeNull();
    expect(validateIcdCode('ICD-9 codes.', mspSet)).toBeNull();
  });
});

describe('extractIcdCodesValidated', () => {
  const mspSet = new Set(['401.9', '847.2']);

  it('extracts and validates codes', () => {
    const text = 'ICD-9 Code: 847.2\nICD-9 Code: 999.99';
    const result = extractIcdCodesValidated(text, mspSet);
    expect(result).toHaveLength(2);
    expect(result[0].raw).toMatch(/847.2/);
    expect(result[0].valid).toBe(true);
    expect(result[1].raw).toMatch(/999.99/);
    expect(result[1].valid).toBe(false);
  });

  it('returns valid=null when set is null', () => {
    const text = 'ICD-9 Code: 847.2';
    const result = extractIcdCodesValidated(text, null);
    expect(result).toHaveLength(1);
    expect(result[0].valid).toBeNull();
  });

  it('does not validate ICD-10 codes (both mode)', () => {
    const text = 'ICD-10 Code: Z00.00';
    const result = extractIcdCodesValidated(text, mspSet);
    expect(result).toHaveLength(1);
    expect(result[0].valid).toBeNull();
  });

  it('does not validate ICD-9 codes in pure icd10 mode (wrong-set guard)', () => {
    // In icd10 mode, ICD-9 codes must NOT be validated against the MSP
    // ICD-9 set (avoids false "not in list" warnings on a code that may
    // be correct in an ICD-10 context).
    const text = 'ICD-9 Code: 401.9';
    const result = extractIcdCodesValidated(text, mspSet, 'icd10');
    expect(result).toHaveLength(1);
    expect(result[0].valid).toBeNull();
  });

  it('validates ICD-9 codes in both mode', () => {
    const text = 'ICD-9 Code: 847.2';
    const result = extractIcdCodesValidated(text, mspSet, 'both');
    expect(result[0].valid).toBe(true);
  });

  it('defaults to icd9 mode when no mode given', () => {
    const text = 'ICD-9 Code: 847.2';
    const result = extractIcdCodesValidated(text, mspSet);
    expect(result[0].valid).toBe(true);
  });

  // ---- Edge cases (F-coverage gap fill) ----

  it('stripIcdPrefix handles empty and whitespace-only', () => {
    expect(stripIcdPrefix('')).toBe('');
    expect(stripIcdPrefix('   ')).toBe('');
  });

  it('stripIcdPrefix leaves a bare code (no prefix) intact', () => {
    // No "ICD-N" prefix → the regex doesn't match → bare code returned.
    expect(stripIcdPrefix('401.9')).toBe('401.9');
    expect(stripIcdPrefix('V70.0')).toBe('V70.0');
  });

  it('normalizedForms does not produce duplicate for already-3-digit code', () => {
    // F3 dedup guard: "780" is already 3 digits → no duplicate form.
    expect(normalizedForms('780')).toEqual(['780']);
  });

  it('normalizedForms handles trailing-zero difference as distinct (documents behavior)', () => {
    // The MSP list uses specific trailing-zero forms (e.g. 250.00 exists,
    // 250.0 exists as a parent). normalizedForms does NOT reconcile
    // trailing zeros — 250.00 and 250.0 are treated as distinct. This
    // test documents that behavior so a future change is intentional.
    const forms00 = normalizedForms('250.00');
    const forms0 = normalizedForms('250.0');
    expect(forms00).not.toEqual(forms0);
  });

  it('validateIcdCode returns null for whitespace-only raw', () => {
    expect(validateIcdCode('   ', mspSet)).toBeNull();
  });

  it('validateIcdCode returns null for alpha-only body with no digit', () => {
    // Stray-prose guard: "ICD-9: ABC" has no digit in the code body.
    expect(validateIcdCode('ICD-9: ABC', mspSet)).toBeNull();
  });

  it('extractIcdCodesValidated returns [] for empty text', () => {
    expect(extractIcdCodesValidated('', mspSet)).toEqual([]);
    expect(extractIcdCodesValidated('no codes here', mspSet)).toEqual([]);
  });

  it('mixed ICD-9 and ICD-10 in both mode: only ICD-9 validates', () => {
    // In both mode, ICD-9 codes validate against the MSP set; ICD-10
    // codes render neutral. Both appear in the result array.
    const text = 'ICD-9 Code: 847.2\nICD-10 Code: Z00.00';
    const result = extractIcdCodesValidated(text, mspSet, 'both');
    expect(result).toHaveLength(2);
    const icd9 = result.find((r) => /ICD-9/.test(r.raw));
    const icd10 = result.find((r) => /ICD-10/.test(r.raw));
    expect(icd9?.valid).toBe(true);
    expect(icd10?.valid).toBeNull();
  });

  it('pure icd10 mode: ICD-10 codes render neutral, ICD-9 codes also neutral', () => {
    // Wrong-set guard: in icd10 mode neither validates (ICD-10 has no
    // bundled list; ICD-9 must not be checked against the ICD-9 set).
    const text = 'ICD-9 Code: 401.9\nICD-10 Code: I10';
    const result = extractIcdCodesValidated(text, mspSet, 'icd10');
    expect(result.every((r) => r.valid === null)).toBe(true);
  });
});

describe('extractIcdDescriptions', () => {
  it('captures the per-line "code — description" form the SOAP prompt emits', () => {
    const text = 'Assessment: back pain\n\nICD-9 Code: 847.2 — Sprain of lumbar\nICD-9 Code: 724.5 — Lumbago';
    const map = extractIcdDescriptions(text);
    expect(map.get('847.2')).toBe('Sprain of lumbar');
    expect(map.get('724.5')).toBe('Lumbago');
  });

  it('accepts a plain hyphen as the code/description separator', () => {
    const map = extractIcdDescriptions('ICD-9 - 847.2 - Sprain of lumbar');
    expect(map.get('847.2')).toBe('Sprain of lumbar');
  });

  it('accepts the colon-less "ICD9 847.2 — desc" variant', () => {
    const map = extractIcdDescriptions('ICD9 847.2 — Sprain of lumbar');
    expect(map.get('847.2')).toBe('Sprain of lumbar');
  });

  it('skips description-less code lines (inline mentions)', () => {
    // Inline "(ICD-9: 250.0)" and bare "ICD-9 Code: 401.9" lines carry no
    // description — they must not seed the map with junk.
    const map = extractIcdDescriptions('(ICD-9: 250.0)\nICD-9 Code: 401.9');
    expect(map.size).toBe(0);
  });

  it('keeps the first description when a code line repeats', () => {
    const map = extractIcdDescriptions(
      'ICD-9 Code: 847.2 — Sprain of lumbar\nICD-9 Code: 847.2 — Duplicate',
    );
    expect(map.get('847.2')).toBe('Sprain of lumbar');
    expect(map.size).toBe(1);
  });

  it('does not treat ordinary dashed prose lines as code descriptions', () => {
    const map = extractIcdDescriptions('1. Ibuprofen 400 mg — take with food');
    expect(map.size).toBe(0);
  });

  it('returns an empty map for empty text', () => {
    expect(extractIcdDescriptions('').size).toBe(0);
  });
});

describe('extractIcdCodesValidated — bare code + explaining title', () => {
  const mspSet = new Set(['847.2', '001.0']);
  const mspDescs = new Map([
    ['847.2', 'LUMBAR'],
    ['001.0', 'CHOLERA DUE TO VIBRIO CHOLERAE'],
    ['V70.0', 'ROUTINE GENERAL MEDICAL EXAMINATION'],
  ]);

  it('exposes the bare (prefix-stripped) code for chip display', () => {
    const result = extractIcdCodesValidated('ICD-9 Code: 847.2', mspSet);
    expect(result[0].bare).toBe('847.2');
    expect(result[0].raw).toBe('ICD-9 Code: 847.2');
  });

  it('uses the note description when the note carries one', () => {
    const text = 'ICD-9 Code: 847.2 — Sprain of lumbar';
    const result = extractIcdCodesValidated(text, mspSet, 'icd9', mspDescs);
    expect(result[0].description).toBe('Sprain of lumbar');
  });

  it('falls back to the official MSP description, softened to title case', () => {
    // No " — description" in the note; the MSP map is ALL-CAPS.
    const result = extractIcdCodesValidated('(ICD-9: 847.2)', mspSet, 'icd9', mspDescs);
    expect(result[0].description).toBe('Lumbar');
  });

  it('note description wins over the MSP description', () => {
    const text = 'ICD-9 Code: 847.2 — Sprain of lumbar';
    const result = extractIcdCodesValidated(text, mspSet, 'icd9', mspDescs);
    expect(result[0].description).toBe('Sprain of lumbar');
  });

  it('MSP fallback matches the zero-padded form of a trimmed code', () => {
    // Note emits "1.0", the MSP map keys "001.0" — description must still
    // resolve (normalizedForms lookup).
    const result = extractIcdCodesValidated('ICD-9 Code: 1.0', mspSet, 'icd9', mspDescs);
    expect(result[0].description).toBe('Cholera Due To Vibrio Cholerae');
  });

  it('description is null when neither source has the code', () => {
    const result = extractIcdCodesValidated('ICD-9 Code: 999.99', mspSet, 'icd9', mspDescs);
    expect(result[0].description).toBeNull();
  });

  it('description is null for a no-description ICD-10 code (no bundled list)', () => {
    const result = extractIcdCodesValidated('ICD-10 Code: Z00.00', mspSet, 'both', mspDescs);
    expect(result[0].description).toBeNull();
  });

  it('works without an MSP map (note descriptions only)', () => {
    const result = extractIcdCodesValidated('ICD-9 Code: 847.2 — Sprain of lumbar', mspSet);
    expect(result[0].description).toBe('Sprain of lumbar');
    const bare = extractIcdCodesValidated('(ICD-9: 847.2)', mspSet);
    expect(bare[0].description).toBeNull();
  });

  it('title-case softening keeps alphanumerics like B12 intact', () => {
    const descs = new Map([['266.2', 'OTHER B-COMPLEX DEFICIENCIES']]);
    const result = extractIcdCodesValidated('ICD-9 Code: 266.2', null, 'icd9', descs);
    expect(result[0].description).toBe('Other B-Complex Deficiencies');
  });
});

describe('billingCodesLabel', () => {
  it('labels the list for each ICD mode', () => {
    expect(billingCodesLabel('icd9')).toBe('Billing codes (ICD-9)');
    expect(billingCodesLabel('icd10')).toBe('Billing codes (ICD-10)');
    expect(billingCodesLabel('both')).toBe('Billing codes (ICD-9/ICD-10)');
  });

  it('defaults to the BC MSP ICD-9 label', () => {
    expect(billingCodesLabel()).toBe('Billing codes (ICD-9)');
  });
});
