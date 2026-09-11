// @vitest-environment jsdom
/** Settings-change freshness regression (@ui-consultant's blocker, 302b78c):
 * changing a digest-relevant setting (ai_model) must invalidate the painted
 * verdict AND issue a new get_generation_freshness comparison. The backend's
 * a5 test proves the digest comparison detects model changes; this test
 * proves the UI actually re-asks. Deferred responses pin the exact race:
 * at 302b78c the mutation fired no second request and left the pre-change
 * "Current" badge visible.
 */
import { cleanup, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import GenerateTab from './GenerateTab.svelte';
import { recordings } from '../stores/recordings.svelte';
import { settings } from '../stores/settings.svelte';
import type { Recording } from '../types';
import type { FreshnessReport } from '../api/generation';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/webview', () => ({ getCurrentWebview: () => ({ onDragDropEvent: vi.fn(async () => () => {}) }) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ writeText: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(async () => null) }));
vi.mock('../utils/notificationSound', () => ({ playSoapCompleteChime: vi.fn() }));

function fixture(id = 'synthetic-a'): Recording {
  return {
    id, filename: `${id}.wav`, transcript: 'Synthetic transcript', soap_note: 'Stored synthetic SOAP',
    referral: null, letter: null, peer_discussion: null, chat: null, patient_name: null,
    audio_path: '/synthetic/not-read.wav', duration_seconds: null, file_size_bytes: null,
    stt_provider: null, ai_provider: null, tags: [], status: { status: 'pending' }, created_at: '', metadata: null,
  };
}

const freshAll = (): FreshnessReport => ({
  soap: { status: 'fresh', reasons: [] },
  referral: { status: 'unknown', reasons: ['missing_provenance'] },
  letter: { status: 'unknown', reasons: ['missing_provenance'] },
  peer_discussion: { status: 'unknown', reasons: ['missing_provenance'] },
});

/** Deferred freshness responses: each call returns a promise the test
 * resolves manually, so exactly one request is in flight at a time and
 * response ordering is deterministic. */
let pending: { resolve: (report: FreshnessReport) => void }[] = [];

beforeEach(() => {
  vi.clearAllMocks();
  pending = [];
  const saved = fixture();
  recordings.selectedRecording = saved;
  recordings.loading = false;
  settings.loaded = true;
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === 'get_recording') return saved;
    if (command === 'get_generation_freshness') {
      let resolve!: (report: FreshnessReport) => void;
      const response = new Promise<FreshnessReport>((res) => (resolve = res));
      pending.push({ resolve });
      return response;
    }
    return [];
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('settings-change freshness invalidation', () => {
  it('changing ai_model drops the stale Current verdict and issues a second comparison (@ui-consultant regression sequence)', async () => {
    render(GenerateTab);

    // Request 1 fires (debounced) and stays pending until we resolve it.
    await waitFor(() => expect(pending).toHaveLength(1));

    // Resolve the first response as fresh → "Current" paints on the SOAP row.
    pending[0].resolve(freshAll());
    const row = screen.getByRole('article', { name: 'SOAP note' });
    await waitFor(() => expect(within(row).getByText('Current')).toBeTruthy());

    // Change the AI model — a digest-relevant setting. Mutate through the
    // same whole-snapshot replacement the store performs on every edit.
    settings.state = { ...settings.state, ai_model: 'synthetic-model-b' };

    // Wait past the 300ms freshness debounce.
    await new Promise((resolve) => setTimeout(resolve, 400));

    // TWO requests issued; the second is still pending (deferred).
    const freshnessCalls = vi.mocked(invoke).mock.calls.filter(([c]) => c === 'get_generation_freshness');
    expect(freshnessCalls).toHaveLength(2);
    expect(pending).toHaveLength(2);

    // The pre-change "Current" verdict must be GONE — the badge shows the
    // in-flight state, never a verdict computed against the old settings.
    expect(within(row).queryByText('Current')).toBeNull();
    expect(within(row).getByText('Checking freshness…')).toBeTruthy();
  });
});
