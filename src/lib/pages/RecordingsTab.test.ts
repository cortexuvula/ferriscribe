// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import RecordingsTab from './RecordingsTab.svelte';
import type { RecordingSummary } from '../types';

const { store, settings, toasts } = vi.hoisted(() => ({
  store: {
    list: [] as RecordingSummary[], loading: false, hasMore: false,
    selectedRecording: null, activeTotal: 4, trashedTotal: 0,
    load: vi.fn(), refreshTrashedTotal: vi.fn(), refreshActiveTotal: vi.fn(),
    removeAll: vi.fn(), remove: vi.fn(), undoMoveAll: vi.fn(), restore: vi.fn(),
  },
  settings: { trashRetentionDays: 30 },
  toasts: { add: vi.fn(), success: vi.fn(), error: vi.fn() },
}));
vi.mock('../stores/recordings.svelte', () => ({ recordings: store, selectRecording: vi.fn() }));
vi.mock('../stores/settings.svelte', () => ({ settings }));
vi.mock('../stores/toasts.svelte', () => ({ toasts }));
vi.mock('../stores/pipeline.svelte', () => ({ pipeline: { retry: vi.fn() } }));

beforeEach(() => {
  vi.clearAllMocks();
  settings.trashRetentionDays = 30;
  store.list = [{
    id: 'synthetic-recording', filename: 'synthetic.wav', patient_name: null,
    status: { status: 'pending' }, duration_seconds: 60, created_at: '2026-10-01T12:00:00Z',
    tags: [], has_transcript: false, has_soap_note: false, has_referral: false,
    has_letter: false, has_peer_discussion: false, is_remote: false, tokens_per_second: null,
  }];
  store.activeTotal = 4;
  store.removeAll.mockResolvedValue({ count: 4, ids: ['synthetic-recording'] });
});
afterEach(cleanup);

describe('RecordingsTab — Trash policy copy', () => {
  it.each([7, 14, 21, 30])('uses the %i-day window in move-all confirmation and toast', async days => {
    settings.trashRetentionDays = days;
    render(RecordingsTab);
    await fireEvent.click(screen.getByRole('button', { name: 'Move all to Trash' }));
    const dialog = screen.getByRole('alertdialog');
    expect(dialog.textContent).toContain(`eligible for permanent deletion after ${days} days in Trash`);
    expect(dialog.textContent).toContain('including recordings outside the current search');
    expect(dialog.textContent).not.toContain(`After ${days} days, they are permanently deleted`);
    await fireEvent.click(screen.getByRole('button', { name: 'Move 4 recordings to Trash' }));
    await waitFor(() => expect(toasts.add).toHaveBeenCalledWith(expect.objectContaining({
      message: `4 moved to Trash. Restore them within ${days} days.`,
      actionLabel: 'Undo', type: 'success',
    })));
    expect(store.removeAll).toHaveBeenCalledTimes(1);
  });

  it('uses the canonical eligibility policy in the single-recording confirmation', async () => {
    settings.trashRetentionDays = 14;
    render(RecordingsTab);
    await fireEvent.click(screen.getByRole('button', { name: 'Delete synthetic.wav' }));
    expect(screen.getByRole('alertdialog').textContent).toContain('eligible for permanent deletion after 14 days in Trash');
  });
});
