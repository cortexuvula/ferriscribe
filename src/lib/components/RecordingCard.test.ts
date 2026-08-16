// @vitest-environment jsdom
/**
 * RecordingCard — component-level render tests for the tokens-per-second
 * meta-row display. The card has no backend/store dependencies, so no
 * module mocks are needed — only direct prop rendering.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/svelte';
import RecordingCard from './RecordingCard.svelte';
import type { RecordingSummary } from '../types';

function makeSummary(overrides: Partial<RecordingSummary> = {}): RecordingSummary {
  return {
    id: 'r1',
    filename: 'consult.wav',
    patient_name: null,
    status: { status: 'completed', completed_at: '2026-08-16T00:00:00Z' },
    duration_seconds: 61,
    created_at: '2026-08-16T00:00:00Z',
    tags: [],
    has_transcript: true,
    has_soap_note: true,
    has_referral: false,
    has_letter: false,
    has_peer_discussion: false,
    is_remote: false,
    tokens_per_second: null,
    ...overrides,
  };
}

afterEach(cleanup);

describe('RecordingCard — tokens per second', () => {
  it('renders tokens per second in the meta row when present', () => {
    render(RecordingCard, { recording: makeSummary({ tokens_per_second: 41.52 }) });
    expect(screen.getByText('41.5 tok/s')).toBeTruthy();
  });

  it('hides the tokens-per-second span when null', () => {
    render(RecordingCard, { recording: makeSummary() });
    expect(screen.queryByText(/tok\/s/)).toBeNull();
  });

  it('includes throughput in the aria-label when present', () => {
    render(RecordingCard, { recording: makeSummary({ tokens_per_second: 41.52 }) });
    const label = screen.getByRole('button').getAttribute('aria-label') ?? '';
    expect(label).toContain('41.5 tok/s');
  });
});
