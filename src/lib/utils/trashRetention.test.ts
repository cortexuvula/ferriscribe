import { describe, it, expect } from 'vitest';
import { trashRetentionLabel } from './trashRetention';

const DAY = 86_400_000;
const NOW = new Date('2026-10-06T12:00:00Z').getTime();

function deletedAgo(days: number): string {
  return new Date(NOW - days * DAY).toISOString();
}

describe('trashRetentionLabel', () => {
  it('shows neutral eligibility text well away from expiry', () => {
    const label = trashRetentionLabel(deletedAgo(1), 30, NOW);
    expect(label.text).toMatch(/^29 days until eligible for permanent deletion · /);
    expect(label.nearExpiry).toBe(false);
  });

  it('pluralizes a single remaining day', () => {
    const label = trashRetentionLabel(deletedAgo(29), 30, NOW);
    expect(label.text).toMatch(/^1 day until eligible for permanent deletion · /);
    expect(label.nearExpiry).toBe(true);
  });

  it('shows eligibility under a day with a supplementary near-expiry accent', () => {
    const label = trashRetentionLabel(deletedAgo(29.9), 30, NOW);
    expect(label.text).toMatch(/^Less than 1 day until eligible for permanent deletion · /);
    expect(label.nearExpiry).toBe(true);
  });

  it('shows Pending permanent deletion past the window — never a negative count', () => {
    const label = trashRetentionLabel(deletedAgo(30.5), 30, NOW);
    expect(label.text).toBe('Pending permanent deletion');
    expect(label.nearExpiry).toBe(true);
    expect(label.text).not.toMatch(/-\d+ days/);
  });

  it('names the eligibility date, not a scheduled deletion appointment', () => {
    const label = trashRetentionLabel(deletedAgo(1), 30, NOW);
    const date = label.text.split('until eligible for permanent deletion · ')[1];
    expect(date).toBe(new Date(NOW + 29 * DAY).toLocaleDateString(undefined, {
      month: 'short', day: 'numeric', year: 'numeric',
    }));
    expect(date).not.toMatch(/\d{1,2}:\d{2}/);
  });

  it.each([7, 14, 21, 30])('uses the effective %i-day backend window', (days) => {
    expect(trashRetentionLabel(deletedAgo(days - 1), days, NOW).text).toMatch(/^1 day until eligible for permanent deletion/);
    expect(trashRetentionLabel(deletedAgo(days + 1), days, NOW).text).toBe('Pending permanent deletion');
  });

  it.each([[7, 2], [14, 4], [21, 6], [30, 7]])(
    'scales the %i-day window accent to less than %i days remaining', (window, threshold) => {
      expect(trashRetentionLabel(deletedAgo(0), window, NOW).nearExpiry).toBe(false);
      expect(trashRetentionLabel(deletedAgo(window - threshold), window, NOW).nearExpiry).toBe(false);
      expect(trashRetentionLabel(deletedAgo(window - threshold + 0.01), window, NOW).nearExpiry).toBe(true);
      expect(trashRetentionLabel(deletedAgo(window), window, NOW).text).toBe('Pending permanent deletion');
    },
  );
});
