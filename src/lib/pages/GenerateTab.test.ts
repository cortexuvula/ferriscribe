// @vitest-environment jsdom
/** Production components and stores, mocked only at Tauri / local OS boundaries.
 * These synthetic UI tests do NOT prove the missing backend freshness contract.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import GenerateTab from './GenerateTab.svelte';
import { recordings } from '../stores/recordings.svelte';
import { generation } from '../stores/generation.svelte';
import { rsvp } from '../stores/rsvp.svelte';
import { settings } from '../stores/settings.svelte';
import type { Recording } from '../types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/webview', () => ({ getCurrentWebview: () => ({ onDragDropEvent: vi.fn(async () => () => {}) }) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ writeText: vi.fn(async () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(async () => null) }));
vi.mock('../utils/notificationSound', () => ({ playSoapCompleteChime: vi.fn() }));

function fixture(id = 'synthetic-a'): Recording {
  return {
    id, filename: `${id}.wav`, transcript: 'Synthetic transcript', soap_note: null,
    referral: null, letter: null, peer_discussion: null, chat: null, patient_name: null,
    audio_path: '/synthetic/not-read.wav', duration_seconds: null, file_size_bytes: null,
    stt_provider: null, ai_provider: null, tags: [], status: { status: 'pending' }, created_at: '', metadata: null,
  };
}
let saved: Recording;
beforeEach(() => {
  vi.clearAllMocks();
  saved = fixture();
  recordings.selectedRecording = saved;
  recordings.loading = false;
  generation.finish(); generation.clearError();
  settings.state.soap_notification_sound = false;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === 'get_recording') return saved;
    if (command === 'get_generation_freshness') return { soap: { status: 'unknown', reasons: ['missing_provenance'] } };
    if (command === 'generate_soap') {
      saved = { ...saved, soap_note: 'Synthetic generated SOAP', metadata: { context: (args as { context: string }).context } };
      return saved.soap_note;
    }
    if (command.startsWith('generate_')) return 'Synthetic generated output';
    return [];
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

async function showNotes() {
  if (!screen.queryByLabelText('Notes')) await fireEvent.click(screen.getByRole('button', { name: /Additional Context/ }));
  return screen.getByLabelText('Notes');
}

describe('Generation workspace production page', () => {
  it('shows loading separately from empty, with navigation recovery', async () => {
    recordings.selectedRecording = null;
    recordings.loading = true;
    const navigate = vi.fn();
    render(GenerateTab, { onNavigateRecordings: navigate });
    expect(screen.getByRole('status').textContent).toContain('Loading recordings');
    expect(screen.queryByRole('button', { name: 'Go to Recordings' })).toBeNull();
    recordings.loading = false;
    await waitFor(() => expect(screen.getByRole('button', { name: 'Go to Recordings' })).toBeTruthy());
    await fireEvent.click(screen.getByRole('button', { name: 'Go to Recordings' }));
    expect(navigate).toHaveBeenCalledOnce();
  });

  it('starts optional context collapsed but expands saved active context and summarizes it when collapsed', async () => {
    const view = render(GenerateTab);
    expect(screen.queryByLabelText('Notes')).toBeNull();
    view.unmount();
    recordings.selectedRecording = { ...saved, metadata: { context: 'Synthetic notes', patient_context: { medications: ['Synthetic medication'], allergies: [], conditions: [] } } };
    render(GenerateTab);
    await waitFor(() => expect((screen.getByLabelText('Notes') as HTMLTextAreaElement).value).toBe('Synthetic notes'));
    await fireEvent.click(screen.getByRole('button', { name: /Additional Context/ }));
    expect(screen.queryByLabelText('Notes')).toBeNull();
    expect(screen.getByText('Included: medications, notes')).toBeTruthy();
  });

  it('sends partial freeform edits with structured context intact and preserves edits through output refresh', async () => {
    recordings.selectedRecording = { ...saved, metadata: { context: 'Synthetic note alpha', patient_context: { medications: ['Synthetic medication'], allergies: [], conditions: [] } } };
    render(GenerateTab);
    const notes = await showNotes();
    await fireEvent.input(notes, { target: { value: 'Synthetic note beta' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Generate SOAP note' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('generate_soap', {
      recordingId: 'synthetic-a', template: null,
      // F1/F2 contract: the freeform context carries ONLY notes (+ OCR);
      // structured lists travel via patientContext — Rust folds both into
      // the prompt and the digest.
      context: 'Synthetic note beta',
      patientContext: { patient_name: null, prior_soap_notes: [], medications: ['Synthetic medication'], allergies: [], conditions: [] },
    }));
    await waitFor(() => expect(screen.getByRole('article', { name: 'SOAP note' })).toBeTruthy());
    expect((screen.getByLabelText('Notes') as HTMLTextAreaElement).value).toBe('Synthetic note beta');
    await waitFor(() => expect(screen.getByText('Freshness unavailable')).toBeTruthy());
  });

  it('resets recording-specific context on switch and reloads saved context on return', async () => {
    const a = { ...saved, metadata: { context: 'Saved synthetic A' } };
    recordings.selectedRecording = a;
    render(GenerateTab);
    await fireEvent.input(await showNotes(), { target: { value: 'Unsaved synthetic A' } });
    recordings.selectedRecording = fixture('synthetic-b');
    await waitFor(() => expect(screen.queryByLabelText('Notes')).toBeNull());
    expect((await showNotes() as HTMLTextAreaElement).value).toBe('');
    recordings.selectedRecording = a;
    await waitFor(() => expect((screen.getByLabelText('Notes') as HTMLTextAreaElement).value).toBe('Saved synthetic A'));
  });

  it('validates the effective 50,000 character limit without invoking generation, and retries after trimming', async () => {
    render(GenerateTab);
    await fireEvent.input(await showNotes(), { target: { value: 'x'.repeat(50_001) } });
    await fireEvent.click(screen.getByRole('button', { name: 'Generate SOAP note' }));
    expect(screen.getByRole('alert').textContent).toContain('50,001');
    expect(vi.mocked(invoke).mock.calls.some(([cmd]) => cmd.startsWith('generate_'))).toBe(false);
    await fireEvent.input(screen.getByLabelText('Notes'), { target: { value: 'Short synthetic context' } });
    await fireEvent.click(within(screen.getByRole('alert')).getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('generate_soap', expect.objectContaining({ context: 'Short synthetic context' })));
  });

  it('copies and speed-reads peer discussion rather than falling through to letter', async () => {
    recordings.selectedRecording = { ...saved, letter: 'Synthetic letter text', peer_discussion: 'Synthetic discussion text' };
    const speed = vi.spyOn(rsvp, 'openGeneric');
    render(GenerateTab);
    const row = screen.getByRole('article', { name: 'Peer discussion' });
    await fireEvent.click(within(row).getByRole('button', { name: 'Copy' }));
    expect(writeText).toHaveBeenCalledWith('Synthetic discussion text');
    await fireEvent.click(within(row).getByRole('button', { name: 'Speed Read' }));
    expect(speed).toHaveBeenCalledWith('Synthetic discussion text', 'peer_discussion');
  });
});
