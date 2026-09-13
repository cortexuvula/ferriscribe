// @vitest-environment jsdom
// Component-level coverage of the metadata-only copy gate. The DECISION
// table itself is unit-tested in src/lib/utils/copyLogic.test.ts — these
// tests prove the component actually calls that module (draft routing,
// caveat prepend, hard block with clipboard untouched, visible remedy) and
// that the ui-consultant bypass (typing a marker into the text) stays
// closed. All fixtures are synthetic — no patient content.
import { render, screen, cleanup } from '@testing-library/svelte';
import { fireEvent } from '@testing-library/dom';
import { tick } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Recording } from '../types';

// ── Mocks ──────────────────────────────────────────────────────────────────

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  save: vi.fn().mockResolvedValue(null),
}));

vi.mock('../api/export', () => ({
  exportAudio: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('../api/contentSync', () => ({
  fetchAudioFromServer: vi.fn().mockResolvedValue(undefined),
}));

const mockCopyToClipboard = vi.fn().mockResolvedValue(undefined);
vi.mock('../utils/clipboard', () => ({
  copyToClipboard: (...args: unknown[]) => mockCopyToClipboard(...args),
}));

vi.mock('../icd', () => ({
  resolveIcdCodes: () => [],
  billingCodesLabel: () => 'ICD-9',
}));

const mockSettings = {
  state: {
    icd_version: '9',
    sync_content: false,
    diarization_enabled: false,
    diarization_models_installed: false,
  },
};
vi.mock('../stores/settings.svelte', () => ({
  settings: mockSettings,
}));

vi.mock('../stores/icd9.svelte', () => ({
  icd9: { codeSet: [], descriptions: {}, loadError: null, retry: vi.fn() },
}));

vi.mock('../stores/rsvp.svelte', () => ({
  rsvp: { openSoap: vi.fn(), openGeneric: vi.fn() },
}));

vi.mock('../stores/toasts.svelte', () => ({
  toasts: { add: vi.fn(), error: vi.fn(), success: vi.fn() },
}));

vi.mock('../stores/settingsNav.svelte', () => ({
  settingsNav: { navigateTo: vi.fn() },
}));

// The soap-tab render pulls in RichEditor, whose spellcheck extension kicks
// off a dictionary fetch with a path URL that jsdom cannot resolve — an
// unhandled rejection unrelated to the behavior under test. Replace the
// spellchecker with a no-op.
vi.mock('../components/rich_editor/spellcheck/spellchecker', () => ({
  getSpellchecker: () => ({
    load: () => Promise.resolve(),
    check: () => true,
    checkWord: () => true,
  }),
}));

// ── Recordings store mock ──────────────────────────────────────────────────

let mockSelectedRecording: Recording | null = null;

const mockRecordings = {
  get selectedRecording() {
    return mockSelectedRecording;
  },
  set selectedRecording(v: Recording | null) {
    mockSelectedRecording = v;
  },
  list: [],
  loading: false,
  loadingMore: false,
  searchQuery: '',
  hasMore: false,
  syncing: false,
  syncPending: false,
  lastSyncedAt: null,
  lastSyncError: null,
  protectFromRemoteUpdate: () => {},
  unprotectFromRemoteUpdate: () => {},
  load: vi.fn(),
  loadMore: vi.fn(),
  search: vi.fn(),
};

vi.mock('../stores/recordings.svelte', () => ({
  recordings: mockRecordings,
  selectRecording: vi.fn(),
}));

// ── Helpers ────────────────────────────────────────────────────────────────

function makeRecording(
  overrides: Partial<Recording> & { metadata?: Recording['metadata'] } = {},
): Recording {
  return {
    id: 'rec-1',
    filename: 'synthetic-fixture.wav',
    transcript: 'Synthetic transcript content for testing.',
    soap_note: 'Synthetic SOAP note for testing.',
    referral: null,
    letter: null,
    peer_discussion: null,
    chat: null,
    patient_name: 'Test Patient',
    audio_path: '/tmp/synthetic-fixture.wav',
    duration_seconds: 120,
    file_size_bytes: 1024,
    stt_provider: 'whisper',
    ai_provider: 'local',
    tags: [],
    status: { status: 'completed', completed_at: '2026-09-12T00:00:00Z' },
    created_at: '2026-09-12T00:00:00Z',
    metadata: overrides.metadata ?? null,
    ...overrides,
  };
}

const CAVEAT =
  'Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.';

const FOLD_METADATA = {
  diarization_outcome: 'completed-with-unassigned',
  diarization_fold_evidence: 'fold_possible',
} as const;

// ── Tests ──────────────────────────────────────────────────────────────────

describe('EditorTab — Copy button behavior (metadata-only gate)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSelectedRecording = null;
  });

  afterEach(() => {
    cleanup();
    mockSelectedRecording = null;
  });

  it('version >= 2: caveat prepended, text verbatim below it', async () => {
    const storedText = [
      '00:00:01,000 --> 00:00:02,000 [Speaker 1]',
      'Hello, how are you today?',
      '',
      '00:00:03,000 --> 00:00:04,000 [Speaker unassigned]',
      'Some background noise.',
    ].join('\n');

    mockSelectedRecording = makeRecording({
      transcript: storedText,
      metadata: { transcript_format_version: 2 },
    });

    const EditorTab = (await import('./EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    const copyButton = screen.getByRole('button', { name: /^Copy$/i });
    await fireEvent.click(copyButton);

    expect(mockCopyToClipboard).toHaveBeenCalledTimes(1);
    const copiedText = mockCopyToClipboard.mock.calls[0]![0] as string;

    expect(copiedText).toContain(CAVEAT);
    expect(copiedText).toContain('[Speaker unassigned]');
    const prefix = copiedText.slice(0, copiedText.indexOf(CAVEAT) + CAVEAT.length + 2);
    expect(copiedText.slice(prefix.length)).toBe(storedText);
  });

  it('non-transcript tab copy: no caveat, verbatim text', async () => {
    mockSelectedRecording = makeRecording({
      soap_note: 'Original SOAP note content.',
      metadata: { diarization_outcome: 'completed' },
    });

    const EditorTab = (await import('./EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'soap' as const });

    const copyButton = screen.getByRole('button', { name: /^Copy$/i });
    await fireEvent.click(copyButton);

    expect(mockCopyToClipboard).toHaveBeenCalledTimes(1);
    const copiedText = mockCopyToClipboard.mock.calls[0]![0] as string;
    expect(copiedText).not.toContain(CAVEAT);
    expect(copiedText).toBe('Original SOAP note content.');
  });

  it('BYPASS REGRESSION (ui-consultant): typing [Speaker unassigned] into the text does NOT clear a fold_possible block', async () => {
    // The superseded rule read the text: append ONE user-typable marker
    // paragraph and the decision flipped to proceed while the suspect
    // passage sat untouched. The corrected gate reads ONLY system-written
    // metadata — the draft is routed into the clipboard but never judged.
    const storedText = 'Speaker 1: Hello.\n\nSome unattributed speech, possibly folded.';
    mockSelectedRecording = makeRecording({
      transcript: storedText,
      metadata: { ...FOLD_METADATA },
    });

    const EditorTab = (await import('./EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    const editButton = screen.getByText('Edit');
    await fireEvent.click(editButton);
    const textarea = document.querySelector('textarea') as HTMLTextAreaElement;
    textarea.value = storedText + '\n\n[Speaker unassigned]';
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    await tick();

    const copyButton = screen.getByRole('button', { name: /^Copy$/i });
    await fireEvent.click(copyButton);

    // HARD block: clipboard untouched despite the typed marker.
    expect(mockCopyToClipboard).not.toHaveBeenCalled();
    expect(screen.getByTestId('copy-block-message')).toBeTruthy();
  });

  it('PARTIAL MARKER REGRESSION: text carrying a marker + old/absent version + fold_possible -> still BLOCKED', async () => {
    // A legacy folded transcript that happens to contain the marker
    // substring (paste, partial re-transcribe, anything) must not proceed
    // on version-less text: the version is the only proceed signal.
    const textWithMarker =
      'Speaker 1: Hello.\n\n[Speaker unassigned]\n\nSome folded-looking span.';
    mockSelectedRecording = makeRecording({
      transcript: textWithMarker,
      metadata: { ...FOLD_METADATA, transcript_format_version: 1 },
    });

    const EditorTab = (await import('./EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    const copyButton = screen.getByRole('button', { name: /^Copy$/i });
    await fireEvent.click(copyButton);

    expect(mockCopyToClipboard).not.toHaveBeenCalled();
    expect(screen.getByTestId('copy-block-message')).toBeTruthy();
  });

  it('fold_possible block: clipboard untouched, reason + remedy visible WITHOUT hovering', async () => {
    const foldedText = [
      'Speaker 1: Hello, how are you?',
      '',
      'Some unattributed speech that could have been folded.',
      '',
      'Speaker 2: Fine, thanks.',
    ].join('\n');

    mockSelectedRecording = makeRecording({
      transcript: foldedText,
      // Post-save state: a prior save cleared transcript_segments and the
      // backend wrote the fold verdict (commit a58924b).
      metadata: { ...FOLD_METADATA },
    });

    const EditorTab = (await import('./EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    const copyButton = screen.getByRole('button', { name: /^Copy$/i });
    await fireEvent.click(copyButton);

    // HARD block: clipboard must not have been written, no override path.
    expect(mockCopyToClipboard).not.toHaveBeenCalled();

    // The message renders INLINE (role=status), not tooltip-only.
    const msg = screen.getByTestId('copy-block-message');
    expect(msg.textContent).toContain('no identified speaker');
    expect(msg.textContent).toContain('unlabelled speech after a labelled span');
    expect(msg.textContent).toContain('Re-transcribe');
    expect(msg.textContent).toContain('not guaranteed correct');
    // Wording rule: never asserts misattribution/fabrication/pre-fix history.
    const lower = (msg.textContent ?? '').toLowerCase();
    expect(lower).not.toContain('misattribut');
    expect(lower).not.toContain('fabricat');
    expect(lower).not.toContain('pre-fix');
  });

  it('key-absent block: clipboard untouched, exact unknown wording visible', async () => {
    const plainText = 'First paragraph, no speaker labels.\n\nSecond paragraph.';
    mockSelectedRecording = makeRecording({
      transcript: plainText,
      // Pre-flag / stale recording: no fold-evidence key, no version.
      metadata: { diarization_outcome: 'completed-with-unassigned' },
    });

    const EditorTab = (await import('./EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    const copyButton = screen.getByRole('button', { name: /^Copy$/i });
    await fireEvent.click(copyButton);

    expect(mockCopyToClipboard).not.toHaveBeenCalled();
    const msg = screen.getByTestId('copy-block-message');
    expect(msg.textContent).toContain(
      'Speaker attribution cannot be verified from the saved metadata.',
    );
    expect(msg.textContent).toContain('Re-transcribe');
  });

  it('honest legacy (none_observed): proceeds with caveat', async () => {
    const honestText = [
      'First paragraph of speech, no labels.',
      '',
      'Second paragraph of speech, no labels.',
    ].join('\n');

    mockSelectedRecording = makeRecording({
      transcript: honestText,
      metadata: {
        diarization_outcome: 'completed-with-unassigned',
        diarization_fold_evidence: 'none_observed',
      },
    });

    const EditorTab = (await import('./EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    const copyButton = screen.getByRole('button', { name: /^Copy$/i });
    await fireEvent.click(copyButton);

    expect(mockCopyToClipboard).toHaveBeenCalledTimes(1);
    const copiedText = mockCopyToClipboard.mock.calls[0]![0] as string;
    expect(copiedText).toContain(CAVEAT);
    expect(copiedText).toContain('First paragraph of speech, no labels.');
  });

  it('block survives an unrelated correction and save: the metadata, not the text, decides', async () => {
    // The save path cleared transcript_segments and persisted
    // diarization_fold_evidence = fold_possible (backend, a58924b). The user
    // then fixes a typo (unrelated correction) and saves; Copy must STILL
    // block — editing text changes neither the version nor the flag.
    const savedText = 'Speaker 1: Hello.\n\nUnattributed span, possibly folded.';
    mockSelectedRecording = makeRecording({
      transcript: savedText,
      metadata: { ...FOLD_METADATA },
    });

    const EditorTab = (await import('./EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    // Unrelated correction: edit, then Done (saves via onChange).
    const editButton = screen.getByText('Edit');
    await fireEvent.click(editButton);
    const textarea = document.querySelector('textarea') as HTMLTextAreaElement;
    textarea.value = 'Speaker 1: Hello.\n\nUnattributed span, possibly folded. Typo fixed.';
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    await tick();
    const doneButton = screen.getByText('Done');
    await fireEvent.click(doneButton);
    await tick();

    const copyButton = screen.getByRole('button', { name: /^Copy$/i });
    await fireEvent.click(copyButton);

    expect(mockCopyToClipboard).not.toHaveBeenCalled();
    expect(screen.getByTestId('copy-block-message')).toBeTruthy();
  });

  it('successful re-transcription (fresh version 2, flag cleared) proceeds — the remedy actually works', async () => {
    // Retranscription binding, success path: the backend atomically
    // replaces the text AND writes transcript_format_version=2 AND removes
    // the stale fold flag (same persist). The gate must let this through —
    // otherwise the remedy is advertised but broken, and permanent blocks
    // teach users to distrust the block.
    const freshText = [
      '00:00:01,000 --> 00:00:02,000 [Speaker 1]',
      'Hello.',
      '',
      '00:00:03,000 --> 00:00:04,000 [Speaker unassigned]',
      'Gap.',
    ].join('\n');
    mockSelectedRecording = makeRecording({
      transcript: freshText,
      metadata: { transcript_format_version: 2 },
    });

    const EditorTab = (await import('./EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    const copyButton = screen.getByRole('button', { name: /^Copy$/i });
    await fireEvent.click(copyButton);

    expect(mockCopyToClipboard).toHaveBeenCalledTimes(1);
    const copiedText = mockCopyToClipboard.mock.calls[0]![0] as string;
    expect(copiedText).toContain(CAVEAT);
    expect(copiedText).toContain(freshText);
  });
});
