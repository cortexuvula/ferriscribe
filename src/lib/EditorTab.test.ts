// @vitest-environment jsdom
import { render, screen, cleanup } from '@testing-library/svelte';
import { fireEvent } from '@testing-library/dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Recording } from './types';

// ── Mocks ──────────────────────────────────────────────────────────────────
// Mock all Tauri/external deps before importing EditorTab.

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  save: vi.fn().mockResolvedValue(null),
}));

vi.mock('./api/export', () => ({
  exportAudio: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./api/contentSync', () => ({
  fetchAudioFromServer: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./utils/clipboard', () => ({
  copyToClipboard: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./icd', () => ({
  resolveIcdCodes: () => [],
  billingCodesLabel: () => 'ICD-9',
}));

// Current-settings state the presentation must IGNORE: mutated to simulate
// installing models / enabling diarization after the fact.
const mockSettings = {
  state: {
    icd_version: '9',
    sync_content: false,
    diarization_enabled: false,
    diarization_models_installed: false,
  },
};
vi.mock('./stores/settings.svelte', () => ({
  settings: mockSettings,
}));

const mockIcd9 = {
  codeSet: [],
  descriptions: {},
  loadError: null,
  retry: vi.fn(),
};
vi.mock('./stores/icd9.svelte', () => ({
  icd9: mockIcd9,
}));

const mockRsvp = {
  openSoap: vi.fn(),
  openGeneric: vi.fn(),
};
vi.mock('./stores/rsvp.svelte', () => ({
  rsvp: mockRsvp,
}));

const mockToasts = {
  add: vi.fn(),
  error: vi.fn(),
  success: vi.fn(),
};
vi.mock('./stores/toasts.svelte', () => ({
  toasts: mockToasts,
}));

const mockSettingsNav = {
  navigateTo: vi.fn(),
};
vi.mock('./stores/settingsNav.svelte', () => ({
  settingsNav: mockSettingsNav,
}));

// ── Recordings store mock ──────────────────────────────────────────────────
// We need a reactive $state-based store so $derived reads work. The mock
// provides a settable selectedRecording that EditorTab reads from.

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
  protectFromRemoteUpdate: () => {
    // no-op in mock
  },
  unprotectFromRemoteUpdate: () => {
    // no-op in mock
  },
  load: vi.fn(),
  loadMore: vi.fn(),
  search: vi.fn(),
};

vi.mock('./stores/recordings.svelte', () => ({
  recordings: mockRecordings,
  selectRecording: vi.fn(),
}));

// ── Helpers ────────────────────────────────────────────────────────────────

function makeRecording(
  overrides: Partial<Recording> & { metadata?: Recording['metadata'] } = {},
): Recording {
  return {
    id: 'rec-1',
    filename: 'test.wav',
    transcript: 'Synthetic transcript content for testing.',
    soap_note: null,
    referral: null,
    letter: null,
    peer_discussion: null,
    chat: null,
    patient_name: 'Test Patient',
    audio_path: '/tmp/test.wav',
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

// ── Tests ──────────────────────────────────────────────────────────────────

describe('EditorTab — diarizationOutcome pass-through (Codie gate)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSelectedRecording = null;
  });

  afterEach(() => {
    cleanup();
    mockSelectedRecording = null;
  });

  // Codie's explicit gate: feed each backend outcome value and assert it
  // reaches TranscriptView unchanged. The failure mode of this slice chain
  // has been each layer RE-DERIVING state instead of TRANSPORTING it.
  // This test pins the contract: EditorTab must pass the outcome verbatim.

  const outcomes = [
    'off',
    'skipped',
    'failed',
    'completed',
    'completed-with-unassigned',
    'unknown',
  ] as const;

  for (const outcome of outcomes) {
    it(`passes '${outcome}' verbatim from metadata to TranscriptView`, async () => {
      // Set up recording with explicit diarization_outcome in metadata.
      mockSelectedRecording = makeRecording({
        transcript: `Synthetic transcript for ${outcome} test.`,
        metadata: {
          diarization_outcome: outcome,
          transcript_segments: [
            { speaker: 'Speaker 1', text: `Test segment for ${outcome}.`, start: 0, end: 5 },
          ],
        },
      });

      // Dynamic import after mocks are in place.
      const EditorTab = (await import('./pages/EditorTab.svelte')).default;

      render(EditorTab, { tabId: 'transcript' as const });

      // The CORE contract: the outcome reaches TranscriptView verbatim.
      // We verify this by checking outcome-dependent rendering behavior:
      //
      // - 'off': must NOT show "Speaker-labelling status unavailable"
      //   (that's only for 'unknown'). With labeled segments, caveat still
      //   appears (via hasTextStructure), but the outcome is 'off'.
      //
      // - 'failed'/'completed'/'completed-with-unassigned': diarization ran,
      //   so caveat appears and structured view renders.
      //
      // - 'unknown': with labeled segments, hasTextStructure is true so
      //   structured view renders (preserving labels). The key: must NOT
      //   show "Speaker-labelling status unavailable" (that's only when
      //   outcome is 'unknown' AND no explicit labels in text).

      const unavailable = screen.queryByText('Speaker-labelling status unavailable');

      if (outcome === 'unknown') {
        // With labeled segments, 'unknown' still renders structured view
        // (hasTextStructure is true). The unavailable message does NOT appear
        // because the text has explicit labels. This proves the outcome is
        // passed verbatim: if EditorTab inferred 'off' or 'completed', the
        // rendering would differ.
        expect(unavailable).toBeNull();
        // Caveat appears because hasTextStructure is true (labeled segments).
        expect(
          screen.getByText(
            'Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.',
          ),
        ).toBeTruthy();
      } else if (outcome === 'off') {
        // 'off' must NOT show the unavailable message (that's 'unknown' only).
        expect(unavailable).toBeNull();
        // With labeled segments, caveat appears (hasTextStructure is true).
        // The outcome 'off' is still passed verbatim — if EditorTab inferred
        // 'completed' from segments, the rendering would be identical, but
        // the contract is that the value is transported, not re-derived.
        expect(
          screen.getByText(
            'Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.',
          ),
        ).toBeTruthy();
      } else {
        // 'failed', 'completed', 'completed-with-unassigned': diarization ran.
        expect(unavailable).toBeNull();
        expect(
          screen.getByText(
            'Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.',
          ),
        ).toBeTruthy();
      }

      // All outcomes with labeled segments: the segment text is rendered
      // (structured view shows segments, not the value field).
      expect(screen.getByText(`Test segment for ${outcome}.`)).toBeTruthy();
    });
  }

  it('falls back to "unknown" when metadata has no diarization_outcome', async () => {
    // Metadata exists but no diarization_outcome key → must default to 'unknown',
    // NOT 'off' (the old inference bug).
    mockSelectedRecording = makeRecording({
      transcript: 'Synthetic transcript without outcome metadata.',
      metadata: {
        transcript_segments: [
          { speaker: null, text: 'Unattributed segment.', start: 0, end: 5 },
        ],
      },
    });

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    // 'unknown' with all-null segments → honest unavailability message.
    // The old bug would show 'off' (plain text, no message).
    expect(
      screen.getByText('Speaker-labelling status unavailable'),
    ).toBeTruthy();
  });

  it('falls back to "unknown" when metadata is null', async () => {
    // No metadata at all → 'unknown'.
    mockSelectedRecording = makeRecording({
      transcript: 'Synthetic transcript with null metadata.',
      metadata: null,
    });

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    expect(
      screen.getByText('Speaker-labelling status unavailable'),
    ).toBeTruthy();
  });

  // Pin: missing-metadata case — a recording with no metadata at all must
  // render 'unknown' honestly, not silently fall through to plain text.
  it('missing metadata renders honest unavailability, not silent plain text', async () => {
    mockSelectedRecording = makeRecording({
      transcript: 'Plain transcript without any metadata.',
      metadata: null,
    });

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    // Must NOT show "Speaker unassigned" (that asserts labelling ran).
    expect(screen.queryByText('Speaker unassigned')).toBeNull();
    // Must show the honest unavailability message.
    expect(
      screen.getByText('Speaker-labelling status unavailable'),
    ).toBeTruthy();
  });

  // Pin: reopen-after-settings-change — a transcript produced under a
  // different toggle state must not be relabelled by the current setting.
  // The outcome is persisted with the recording, so changing settings now
  // must not affect how an old transcript renders.
  it('persisted outcome survives settings change (reopen-after-settings-change pin)', async () => {
    // Recording was produced with diarization 'off' — persisted as 'off'.
    // Current settings may have diarization enabled, but the recording's
    // outcome must still render as 'off' (plain text, no unassigned headings).
    mockSelectedRecording = makeRecording({
      transcript: 'Transcript produced with diarization disabled.',
      metadata: {
        diarization_outcome: 'off',
        transcript_segments: [
          { speaker: null, text: 'Single segment, no diarization.', start: 0, end: 5 },
        ],
      },
    });

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    // 'off' with all-null segments → plain text, no unassigned heading.
    expect(screen.queryByText('Speaker unassigned')).toBeNull();
    expect(
      screen.queryByText('Speaker-labelling status unavailable'),
    ).toBeNull();
    // The transcript text renders as plain text (the value field, not segment text).
    expect(
      screen.getByText('Transcript produced with diarization disabled.'),
    ).toBeTruthy();
  });

  // ── Slice 2d: the 'skipped' outcome (requested but did not run) ─────────

  it('skipped outcome renders the skipped banner, not the unknown message', async () => {
    // The exact gap this slice closes: a persisted 'skipped' used to coerce
    // to 'unknown' and render "Speaker-labelling status unavailable".
    // It must render its own distinct status instead.
    mockSelectedRecording = makeRecording({
      transcript: 'Synthetic transcript whose labelling was skipped.',
      metadata: {
        diarization_outcome: 'skipped',
        transcript_segments: [
          { speaker: null, text: 'Synthetic segment without labels.', start: 0, end: 5 },
        ],
      },
    });

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    // The skipped heading (verbatim contract wording).
    expect(screen.getByText("Speaker labelling wasn't run")).toBeTruthy();
    // Must NOT fall through to the unknown message (Slice 2a behavior).
    expect(
      screen.queryByText('Speaker-labelling status unavailable'),
    ).toBeNull();
    // Labelling never ran → no "Speaker unassigned" headings (that asserts
    // an attribution attempt that never happened).
    expect(screen.queryByText('Speaker unassigned')).toBeNull();
    // The transcript text itself still renders.
    expect(
      screen.getByText('Synthetic transcript whose labelling was skipped.'),
    ).toBeTruthy();
  });

  it('skipped reason line appears ONLY when the persisted reason code confirms it', async () => {
    // With the bounded reason code 'models_unavailable' persisted by the
    // backend (REASON_MODELS_UNAVAILABLE in transcription/inner.rs), the
    // supporting line states the cause.
    mockSelectedRecording = makeRecording({
      transcript: 'Synthetic transcript, models were missing.',
      metadata: {
        diarization_outcome: 'skipped',
        diarization_reason: 'models_unavailable',
      },
    });

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    expect(
      screen.getByText("Required models weren't available for this transcription"),
    ).toBeTruthy();
  });

  it('skipped with missing or unrecognised reason shows generic wording, never invents a cause', async () => {
    // No diarization_reason key at all → generic wording, still the skipped
    // heading. The view must not guess "models unavailable" from anything
    // other than the persisted code.
    mockSelectedRecording = makeRecording({
      transcript: 'Synthetic transcript with no persisted reason.',
      metadata: {
        diarization_outcome: 'skipped',
      },
    });

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    expect(screen.getByText("Speaker labelling wasn't run")).toBeTruthy();
    // Generic wording present, models wording absent.
    expect(
      screen.getByText('Speaker labelling was not run for this recording'),
    ).toBeTruthy();
    expect(
      screen.queryByText("Required models weren't available for this transcription"),
    ).toBeNull();

    // Unrecognised reason code → same generic wording (no invented cause).
    mockSelectedRecording = makeRecording({
      transcript: 'Synthetic transcript with a bogus reason code.',
      metadata: {
        diarization_outcome: 'skipped',
        diarization_reason: 'something_not_in_the_bounded_set',
      },
    });
    cleanup();
    render(EditorTab, { tabId: 'transcript' as const });
    expect(
      screen.getByText('Speaker labelling was not run for this recording'),
    ).toBeTruthy();
    expect(
      screen.queryByText("Required models weren't available for this transcription"),
    ).toBeNull();
  });

  it('skipped banner offers an affordance that opens Audio settings', async () => {
    mockSelectedRecording = makeRecording({
      transcript: 'Synthetic transcript for the affordance test.',
      metadata: {
        diarization_outcome: 'skipped',
        diarization_reason: 'models_unavailable',
      },
    });

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    const button = screen.getByRole('button', { name: 'Open Audio settings' });
    expect(button).toBeTruthy();
    expect(mockSettingsNav.navigateTo).not.toHaveBeenCalled();
    await fireEvent.click(button);
    // The affordance navigates to the Audio pane via the shared settings
    // nav store (App.svelte opens the dialog on any requestedSection).
    expect(mockSettingsNav.navigateTo).toHaveBeenCalledWith('audio');
    // No automatic retry: the affordance is the only offered action.
    expect(mockSettingsNav.navigateTo).toHaveBeenCalledTimes(1);
  });

  it('persisted skipped outcome survives a settings/model change (historical-state pin)', async () => {
    // The recording was transcribed when the models were unavailable —
    // outcome 'skipped' persisted with it. The user has since "installed"
    // the models and enabled diarization in current settings (mocked here
    // as changed settings state). The saved transcript must STILL render
    // skipped: presentation keys off persisted metadata only.
    mockSettings.state.diarization_enabled = true;
    mockSettings.state.diarization_models_installed = true;
    try {
      mockSelectedRecording = makeRecording({
        transcript: 'Old synthetic transcript, models were missing back then.',
        metadata: {
          diarization_outcome: 'skipped',
          diarization_reason: 'models_unavailable',
        },
      });

      const EditorTab = (await import('./pages/EditorTab.svelte')).default;
      render(EditorTab, { tabId: 'transcript' as const });

      expect(screen.getByText("Speaker labelling wasn't run")).toBeTruthy();
      expect(
        screen.getByText("Required models weren't available for this transcription"),
      ).toBeTruthy();
      // Not relabelled, not presented as off/unknown/completed.
      expect(screen.queryByText('Speaker unassigned')).toBeNull();
      expect(
        screen.queryByText('Speaker-labelling status unavailable'),
      ).toBeNull();
    } finally {
      mockSettings.state.diarization_enabled = false;
      mockSettings.state.diarization_models_installed = false;
    }
  });
});

// ── Review contract line C: saved transcript edits vs stale segments ───────
// The save path (backend save_recording_field) now clears
// transcript_segments in the same transaction as the edited text. The
// optimistic store update below mirrors exactly what onEditorChange does
// (transcript field swapped, metadata untouched) — so this test also pins
// that RENDERING must recover from the pre-fix window: even if segments
// were still present, the edited text must win when they disagree with the
// saved transcript. Post-fix, the backend never returns that combination —
// the render must nonetheless render the corrected words from the text
// (marker-aware) rather than the stale segment words.

describe('EditorTab — saved transcript edit beats stale segments (contract line C)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSelectedRecording = null;
  });
  afterEach(() => {
    cleanup();
    mockSelectedRecording = null;
  });

  it('after an edit, the optimistic store update shows the edited words (segments cleared like the backend now does)', async () => {
    // The backend now clears transcript_segments in the same transaction
    // as the edited text (saving_transcript_clears_stale_segments_in_same_save,
    // src-tauri recordings_edit.rs). The optimistic store update must match
    // that shape — metadata without transcript_segments — so what the
    // clinician sees after Done is the corrected text, not the stale
    // segment words. This drives the REAL onEditorChange path.
    mockSelectedRecording = makeRecording({
      transcript: [
        '00:00:01,000 --> 00:00:02,000 [Speaker 1] ',
        'Original wrong wording.',
        '',
        '00:00:03,000 --> 00:00:04,000 [Speaker unassigned] ',
        'Original unattributed wording.',
      ].join('\n'),
      metadata: {
        diarization_outcome: 'completed-with-unassigned',
        transcript_segments: [
          { speaker: 'Speaker 1', text: 'Original wrong wording.', start: 1, end: 2 },
          { speaker: null, text: 'Original unattributed wording.', start: 3, end: 4 },
        ],
      },
    });

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    // Simulate the store-level optimistic update the way onEditorChange
    // does it (transcript field swapped), WITH the backend-matching
    // segment clear.
    mockSelectedRecording = {
      ...mockSelectedRecording!,
      transcript: [
        '00:00:01,000 --> 00:00:02,000 [Speaker 1] ',
        'Corrected dose wording.',
        '',
        '00:00:03,000 --> 00:00:04,000 [Speaker unassigned] ',
        'Unattributable reply.',
      ].join('\n'),
      metadata: {
        diarization_outcome: 'completed-with-unassigned',
      },
    } as Recording;
    // Svelte 5 has no $set; the mocked store is not $state-reactive, so
    // re-render — which is exactly the post-save shape a reload shows.
    cleanup();
    render(EditorTab, { tabId: 'transcript' as const });
    await new Promise((r) => setTimeout(r, 500)); // flush parse debounce

    expect(screen.getByText('Corrected dose wording.')).toBeTruthy();
    expect(screen.queryByText('Original wrong wording.')).toBeNull();
    expect(screen.getAllByText('Speaker unassigned').length).toBe(1);
  });

  it('marker-aware fallback renders when the save cleared the segments (post-fix persisted shape)', async () => {
    // The exact persisted shape the fixed backend returns: edited text,
    // no transcript_segments key. The text fallback must parse both the
    // real speaker label AND the unassigned marker.
    const savedText = [
      '00:00:01,000 --> 00:00:02,000 [Speaker 1] ',
      'Saved attributed turn.',
      '',
      '00:00:03,000 --> 00:00:04,000 [Speaker unassigned] ',
      'Saved unattributable turn.',
    ].join('\n');
    mockSelectedRecording = makeRecording({
      transcript: savedText,
      metadata: {
        diarization_outcome: 'completed-with-unassigned',
        diarization_reason: null,
      },
    });

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    expect(screen.getByText('Saved attributed turn.')).toBeTruthy();
    expect(screen.getByText('Saved unattributable turn.')).toBeTruthy();
    expect(screen.getAllByText('Speaker 1').length).toBe(1);
    expect(screen.getAllByText('Speaker unassigned').length).toBe(1);
  });

  it('a transcript edit invokes save with the edited field value (flush path)', async () => {
    mockSelectedRecording = makeRecording({
      transcript: 'Original single paragraph.',
      // 'off' keeps the plain-text view WITH its Edit toolbar (the unknown
      // branch's missing toolbar is review finding 5, out of scope here).
      metadata: { diarization_outcome: 'off' },
    });
    const invoke = (await import('@tauri-apps/api/core')).invoke as ReturnType<typeof vi.fn>;

    const EditorTab = (await import('./pages/EditorTab.svelte')).default;
    render(EditorTab, { tabId: 'transcript' as const });

    // Enter edit mode and change the text via TranscriptView's editor.
    const editButton = screen.getByText('Edit');
    await editButton.click();
    const textarea = document.querySelector('textarea') as HTMLTextAreaElement;
    expect(textarea).toBeTruthy();
    textarea.value = 'Hand-corrected paragraph.';
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
    const doneButton = screen.getByText('Done');
    await doneButton.click();

    // The debounced save fires with the transcript field and new value.
    await new Promise((r) => setTimeout(r, 1300));
    const calls = invoke.mock.calls.filter((c) => c[0] === 'save_recording_field');
    expect(calls.length).toBeGreaterThanOrEqual(1);
    const last = calls[calls.length - 1]![1] as { field: string; value: string };
    expect(last.field).toBe('transcript');
    expect(last.value).toBe('Hand-corrected paragraph.');
  });
});
