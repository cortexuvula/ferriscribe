import { describe, it, expect } from 'vitest';
import {
  stripIcdPrefix,
  normalizedForms,
  validateIcdCode,
  extractIcdCodesValidated,
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
});
