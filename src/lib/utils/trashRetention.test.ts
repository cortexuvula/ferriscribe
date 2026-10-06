import { describe, it, expect } from 'vitest';
import { trashRetentionLabel, TRASH_RETENTION_DAYS } from './trashRetention';

const DAY = 86_400_000;
/// A fixed "now" so the calendar-date rendering is deterministic.
const NOW = new Date('2026-10-06T12:00:00Z').getTime();

function deletedAgo(days: number): string {
  return new Date(NOW - days * DAY).toISOString();
}

describe('trashRetentionLabel', () => {
  it('shows neutral days-remaining well away from expiry', () => {
    const label = trashRetentionLabel(deletedAgo(1), NOW);
    expect(label.text).toMatch(/^29 days remaining · Permanent deletion after /);
    expect(label.nearExpiry).toBe(false);
  });

  it('pluralizes a single remaining day', () => {
    const label = trashRetentionLabel(deletedAgo(29), NOW);
    expect(label.text).toMatch(/^1 day remaining · Permanent deletion after /);
    expect(label.nearExpiry).toBe(true);
  });

  it('shows "Less than 1 day remaining" under a day, amber', () => {
    const label = trashRetentionLabel(new Date(NOW - 29.9 * DAY).toISOString(), NOW);
    expect(label.text).toMatch(/^Less than 1 day remaining · Permanent deletion after /);
    expect(label.nearExpiry).toBe(true);
  });

  it('shows "Pending permanent deletion" past 30 days — never a negative count', () => {
    const label = trashRetentionLabel(deletedAgo(TRASH_RETENTION_DAYS + 0.5), NOW);
    expect(label.text).toBe('Pending permanent deletion');
    expect(label.nearExpiry).toBe(true);
    expect(label.text).not.toMatch(/-\d+ days/);
  });

  it('switches to amber within 7 days of the purge boundary', () => {
    // 23 days ago → exactly 7 days remaining → still neutral.
    expect(trashRetentionLabel(deletedAgo(23), NOW).nearExpiry).toBe(false);
    // 23.5 days ago → 6.5 days remaining → amber.
    expect(trashRetentionLabel(deletedAgo(23.5), NOW).nearExpiry).toBe(true);
  });

  it('never promises an exact deletion time — only a calendar date', () => {
    const label = trashRetentionLabel(deletedAgo(1), NOW);
    const after = label.text.split('Permanent deletion after ')[1];
    expect(after).toBeTruthy();
    // No time-of-day component (the purge runs on a daily sweep).
    expect(after!).not.toMatch(/\d{1,2}:\d{2}/);
  });

  it('the window matches the backend sweeper literal', () => {
    expect(TRASH_RETENTION_DAYS).toBe(30);
  });
});
