// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import GenerateControls from './GenerateControls.svelte';
import type { Recording } from '../types';
import type { ComponentProps } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));
afterEach(cleanup);

const recording = {
  id: 'synthetic-recording', transcript: 'Synthetic transcript', soap_note: null,
  referral: null, letter: null, peer_discussion: null, metadata: null,
} as Recording;
function props(): ComponentProps<typeof GenerateControls> {
  return {
    recording,
    generationState: { generating: null, progressStatus: null, progress: null, error: null, lastFailedType: null },
    copyStatus: {}, selectedAudienceId: 'builtin-patient', letterType: '',
    audiences: [{ id: 'builtin-patient', name: 'Patient', system_prompt: '', user_template: null, is_builtin: true, created_at: '', updated_at: '' }],
    physicianName: '', specialty: '', discussionReason: '',
    onGenerate: vi.fn(), onCopy: vi.fn(), onSpeedRead: vi.fn(), onClearError: vi.fn(),
    onAudienceChange: vi.fn(), onLetterTypeChange: vi.fn(), onPhysicianNameChange: vi.fn(),
    onSpecialtyChange: vi.fn(), onDiscussionReasonChange: vi.fn(),
  };
}

describe('Generation Configure → Operate composition', () => {
  it('keeps completed documents together with type-specific preview and actions; never guesses freshness', async () => {
    const p = props();
    p.recording = { ...recording, soap_note: 'Synthetic SOAP', referral: 'Synthetic referral', letter: 'Synthetic letter', peer_discussion: 'Synthetic discussion' };
    render(GenerateControls, p);
    const outputs = screen.getByRole('region', { name: 'Recent output' });
    for (const [title, type, text] of [
      ['SOAP note', 'soap', 'Synthetic SOAP'], ['Referral letter', 'referral', 'Synthetic referral'],
      ['Letter', 'letter', 'Synthetic letter'], ['Peer discussion', 'peer_discussion', 'Synthetic discussion'],
    ]) {
      const row = within(outputs).getByRole('article', { name: title });
      expect(within(row).getByText('Freshness unavailable')).toBeTruthy();
      expect(within(row).queryByText('Current')).toBeNull();
      await fireEvent.click(within(row).getByText('Preview'));
      expect(within(row).getByText(text)).toBeTruthy();
      await fireEvent.click(within(row).getByRole('button', { name: 'Copy' }));
      expect(p.onCopy).toHaveBeenLastCalledWith(type);
      await fireEvent.click(within(row).getByRole('button', { name: 'Speed Read' }));
      expect(p.onSpeedRead).toHaveBeenLastCalledWith(type);
      await fireEvent.click(within(row).getByRole('button', { name: 'Regenerate' }));
      expect(p.onGenerate).toHaveBeenLastCalledWith(type);
    }
  });

  it('shows authoritative per-document stale status without contaminating other output rows', () => {
    const p = props();
    p.recording = { ...recording, soap_note: 'Synthetic SOAP', letter: 'Synthetic letter' };
    p.freshness = { soap: 'stale', letter: 'current' };
    render(GenerateControls, p);
    const soap = screen.getByRole('article', { name: 'SOAP note' });
    expect(within(soap).getByText('Stale')).toBeTruthy();
    expect(within(soap).getByText('Inputs changed. Regenerate before using this output.')).toBeTruthy();
    expect(within(soap).getByRole('button', { name: 'Regenerate' })).toBeTruthy();
    const letter = screen.getByRole('article', { name: 'Letter' });
    expect(within(letter).getByText('Current')).toBeTruthy();
    expect(within(letter).queryByText('Stale')).toBeNull();
  });

  it('shows failure even when a previous draft exists and retries the failed type', async () => {
    const p = props();
    p.recording = { ...recording, letter: 'Previous synthetic letter' };
    p.generationState.error = 'Synthetic provider failure';
    p.generationState.lastFailedType = 'letter';
    // Reproduce the real store contract: clearing an error clears its type.
    p.onClearError = vi.fn(() => { p.generationState.lastFailedType = null; });
    render(GenerateControls, p);
    expect(screen.getByRole('alert').textContent).toContain('Synthetic provider failure');
    expect(within(screen.getByRole('article', { name: 'Letter' })).getByText('Failed')).toBeTruthy();
    await fireEvent.click(within(screen.getByRole('alert')).getByRole('button', { name: 'Retry' }));
    expect(p.onGenerate).toHaveBeenCalledWith('letter');
  });

  it('keeps progress on the generating row and disables duplicate generation', () => {
    const p = props();
    p.generationState.generating = 'soap';
    p.generationState.progress = { tokens: 412, elapsed_ms: 20000, tokens_per_second: 20.6 };
    render(GenerateControls, p);
    const row = screen.getByRole('article', { name: 'SOAP note' });
    expect(within(row).getByRole('status').textContent).toContain('412');
    expect((screen.getByRole('button', { name: 'Generating SOAP note…' }) as HTMLButtonElement).disabled).toBe(true);
    expect((within(row).getByRole('button', { name: 'Generating…' }) as HTMLButtonElement).disabled).toBe(true);
  });

  it('anchors SOAP, defers document fields until selected, and separates empty output', async () => {
    const p = props();
    render(GenerateControls, p);
    expect(screen.getByRole('heading', { name: 'Create document' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Generate SOAP note' })).toBeTruthy();
    expect(screen.queryByLabelText('Audience')).toBeNull();
    expect(screen.queryByLabelText('Physician Name')).toBeNull();
    expect(screen.getByRole('heading', { name: 'Recent output' })).toBeTruthy();
    expect(screen.getByText('No documents generated yet.')).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Letter' }));
    expect(screen.getByLabelText('Audience')).toBeTruthy();
    expect(screen.getByLabelText('Purpose')).toBeTruthy();
    expect(screen.queryByLabelText('Physician Name')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Generate letter' }));
    expect(p.onGenerate).toHaveBeenCalledWith('letter');
    await fireEvent.click(screen.getByRole('button', { name: 'Peer discussion' }));
    expect(screen.getByLabelText('Physician Name')).toBeTruthy();
    expect(screen.getByLabelText('Specialty')).toBeTruthy();
    expect(screen.getByLabelText('Reason for Discussion')).toBeTruthy();
    expect(screen.queryByLabelText('Audience')).toBeNull();
  });
});
