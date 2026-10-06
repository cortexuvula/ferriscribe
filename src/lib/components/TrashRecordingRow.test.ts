// @vitest-environment jsdom
/**
 * TrashRecordingRow — one trashed recording.
 *
 * Markup facts:
 *   - ONE display label: patient_name when present, else filename.
 *   - "Recorded <date/time> · <duration>" and "Moved to Trash <date/time>".
 *   - Retention line from trashRetentionLabel (days remaining / under a
 *     day / Pending permanent deletion), amber near expiry via the `now`
 *     test seam.
 *   - Restore is ALWAYS rendered and enabled (never hover-only) and
 *     carries data-restore-btn={id} for focus management in TrashPanel.
 *   - No transcript/SOAP content fields exist on the row's data type.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';
import TrashRecordingRow from './TrashRecordingRow.svelte';
import type { TrashedRecordingSummary } from '../types';

beforeEach(cleanup);

const NOW = new Date('2026-10-06T12:00:00Z').getTime();

function makeItem(overrides: Partial<TrashedRecordingSummary> = {}): TrashedRecordingSummary {
  return {
    id: 'row-1',
    filename: 'visit.wav',
    patient_name: null,
    duration_seconds: 95,
    created_at: '2026-10-01T09:00:00Z',
    deleted_at: new Date(NOW - 2 * 86_400_000).toISOString(), // 2 days ago
    ...overrides,
  };
}

describe('TrashRecordingRow', () => {
  it('shows the patient name when present — one label, not both', () => {
    render(TrashRecordingRow, {
      item: makeItem({ patient_name: 'Doe, Jane' }),
      onRestore: () => {},
      now: NOW,
    });
    expect(screen.getByText('Doe, Jane')).toBeTruthy();
    expect(screen.queryByText('visit.wav')).toBeNull();
  });

  it('falls back to the filename when no patient name', () => {
    render(TrashRecordingRow, {
      item: makeItem(),
      onRestore: () => {},
      now: NOW,
    });
    expect(screen.getByText('visit.wav')).toBeTruthy();
  });

  it('renders Recorded (date·duration) and Moved-to-Trash lines', () => {
    render(TrashRecordingRow, {
      item: makeItem(),
      onRestore: () => {},
      now: NOW,
    });
    expect(screen.getByText(/Recorded .+ · 01:35/)).toBeTruthy();
    expect(screen.getByText(/Moved to Trash /)).toBeTruthy();
  });

  it('renders the days-remaining retention line', () => {
    render(TrashRecordingRow, {
      item: makeItem(),
      onRestore: () => {},
      now: NOW,
    });
    expect(screen.getByText(/28 days remaining · Permanent deletion after /)).toBeTruthy();
  });

  it('shows Pending permanent deletion past the window, never a negative count', () => {
    render(TrashRecordingRow, {
      item: makeItem({ deleted_at: new Date(NOW - 31 * 86_400_000).toISOString() }),
      onRestore: () => {},
      now: NOW,
    });
    expect(screen.getByText('Pending permanent deletion')).toBeTruthy();
  });

  it('Restore is always visible and fires onRestore with no hover dependency', async () => {
    const onRestore = vi.fn();
    render(TrashRecordingRow, { item: makeItem(), onRestore, now: NOW });

    const btn = screen.getByRole('button', { name: 'Restore' }) as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
    expect(btn.getAttribute('data-restore-btn')).toBe('row-1');
    await fireEvent.click(btn);
    expect(onRestore).toHaveBeenCalledTimes(1);
  });

  it('Restore can be disabled during an in-flight restore (duplicate guard)', () => {
    render(TrashRecordingRow, {
      item: makeItem(),
      onRestore: () => {},
      restoreDisabled: true,
      now: NOW,
    });
    expect((screen.getByRole('button', { name: 'Restore' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });
});
