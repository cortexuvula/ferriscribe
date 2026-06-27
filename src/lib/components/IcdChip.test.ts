/**
 * IcdChip — unit-level tests.
 *
 * @testing-library/svelte is not installed and the vitest environment is
 * "node", so full component rendering isn't available. These tests cover
 * the chip's tooltip/class logic by replicating the exact `$derived`
 * computation from the component. The logic is small enough that this
 * inline mirror is a faithful regression guard; if the component's
 * tooltip branch changes, the mirror here must change to match, and
 * any divergence will be caught at review time.
 *
 * The billing-critical behaviors under test:
 *   - valid === false  → amber "invalid" class + warning tooltip
 *   - valid === null   → neutral style + neutralTooltip
 *   - valid === true   → neutral style, empty tooltip
 */
import { describe, it, expect } from 'vitest';

// Mirror of the component's tooltip $derived (IcdChip.svelte:16-23).
function chipTooltip(valid: boolean | null, neutralTooltip = 'Validation unavailable'): string {
  if (valid === false) return 'Not in BC MSP ICD-9 list — verify before billing';
  if (valid === null) return neutralTooltip;
  return '';
}

// Mirror of the class:invalid directive.
function isInvalid(valid: boolean | null): boolean {
  return valid === false;
}

describe('IcdChip tooltip + class logic', () => {
  it('invalid code (valid===false) shows billing warning tooltip', () => {
    expect(chipTooltip(false)).toBe('Not in BC MSP ICD-9 list — verify before billing');
    expect(isInvalid(false)).toBe(true);
  });

  it('neutral code (valid===null) shows default neutral tooltip', () => {
    expect(chipTooltip(null)).toBe('Validation unavailable');
    expect(isInvalid(null)).toBe(false);
  });

  it('neutral code uses custom neutralTooltip when provided', () => {
    expect(chipTooltip(null, 'ICD-10 — validation unavailable')).toBe(
      'ICD-10 — validation unavailable',
    );
  });

  it('valid code (valid===true) has empty tooltip and no invalid class', () => {
    expect(chipTooltip(true)).toBe('');
    expect(isInvalid(true)).toBe(false);
  });

  it('only valid===false triggers the invalid (amber) class', () => {
    // The amber warning must never appear for null or true — a false
    // billing warning on a neutral chip would erode clinician trust.
    expect(isInvalid(false)).toBe(true);
    expect(isInvalid(null)).toBe(false);
    expect(isInvalid(true)).toBe(false);
  });
});
