// @vitest-environment jsdom
/**
 * IcdCodeList — component-level render tests.
 *
 * The default vitest environment is "node" (see vitest.config.ts), which
 * has no DOM; the file-level pragma above switches this suite to jsdom so
 * the full component (chip child included) renders via
 * @testing-library/svelte.
 *
 * Markup facts these tests rely on (kept in sync with the component):
 *   - The container is `<div class="icd-list" role="list">` with the
 *     label as its `aria-label` and a heading span above the rows.
 *   - Each code renders a `.icd-row[role=listitem]` containing an
 *     `.icd-code` chip with the BARE code (no "ICD-9 Code:" prefix) and,
 *     when a title resolved, an `.icd-desc` span with that title.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render, screen, within, cleanup } from '@testing-library/svelte';
import IcdCodeList from './IcdCodeList.svelte';
import type { ValidatedIcdCode } from '../icd';

afterEach(cleanup);

function code(overrides: Partial<ValidatedIcdCode>): ValidatedIcdCode {
  return { raw: 'ICD-9 Code: 847.2', bare: '847.2', valid: true, description: null, ...overrides };
}

describe('IcdCodeList', () => {
  it('renders the heading label and one row per code', () => {
    render(IcdCodeList, {
      label: 'Billing codes (ICD-9)',
      codes: [code({}), code({ raw: 'ICD-9 Code: 724.5', bare: '724.5' })],
    });
    expect(screen.getByText('Billing codes (ICD-9)')).toBeTruthy();
    const rows = screen.getAllByRole('listitem');
    expect(rows).toHaveLength(2);
    expect(screen.getByRole('list').getAttribute('aria-label')).toBe('Billing codes (ICD-9)');
  });

  it('shows the bare code on the chip and the title beside it', () => {
    render(IcdCodeList, {
      label: 'Billing codes (ICD-9)',
      codes: [code({ description: 'Sprain of lumbar' })],
    });
    const row = screen.getByRole('listitem');
    expect(within(row).getByText('847.2')).toBeTruthy();
    expect(within(row).getByText('Sprain of lumbar')).toBeTruthy();
    // The old chip row rendered the raw extracted string — the bare code
    // must replace it, not accompany it.
    expect(within(row).queryByText(/ICD-9 Code/)).toBeNull();
  });

  it('renders the chip alone when no title resolved', () => {
    render(IcdCodeList, {
      label: 'Billing codes (ICD-9)',
      codes: [code({ description: null })],
    });
    expect(screen.getByText('847.2')).toBeTruthy();
    expect(screen.queryByText('Sprain of lumbar')).toBeNull();
  });

  it('keeps the amber invalid styling on off-list codes (via IcdChip)', () => {
    render(IcdCodeList, {
      label: 'Billing codes (ICD-9)',
      codes: [code({ valid: false, description: 'Not on the MSP list' })],
    });
    const chip = screen.getByText('847.2');
    expect(chip.className).toContain('invalid');
    expect(chip.getAttribute('title')).toBe('Not in BC MSP ICD-9 list — verify before billing');
  });

  it('renders nothing per-row beyond chips for an empty title set', () => {
    render(IcdCodeList, { label: 'Billing codes (ICD-9)', codes: [] });
    expect(screen.getByRole('list')).toBeTruthy();
    expect(screen.queryByRole('listitem')).toBeNull();
  });
});
