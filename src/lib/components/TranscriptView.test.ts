// @vitest-environment jsdom
import { render, screen, cleanup } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import TranscriptView from './TranscriptView.svelte';

// Uncertainty-honesty fix: unlabeled sections must be visually distinct from
// labeled sections (never silently continue the previous speaker), and a
// transcript-level caveat must be visible above the scrolling transcript.

// B3 regression: the stored/copied transcript text format is
// `HH:MM:SS,mmm --> HH:MM:SS,mmm [Speaker N]\ntext` (1-based labels from
// format_transcript_with_speakers). When metadata segments are absent the
// rich view must parse that format into badges — and the badge numbering
// must match the labels in the text.
const STORED_TWO_SPEAKER = [
  '00:00:01,340 --> 00:00:03,750 [Speaker 1]',
  'Good, good. You need some refills today.',
  '',
  '00:00:04,100 --> 00:00:06,000 [Speaker 2]',
  'Yes, please.',
  '',
  '00:00:06,500 --> 00:00:08,000 [Speaker 1]',
  'Great.',
].join('\n');

describe('TranscriptView speaker fallback parsing', () => {
  afterEach(cleanup);

  it('renders 1-based Speaker badges from the stored text format', () => {
    render(TranscriptView, { value: STORED_TWO_SPEAKER });
    // Speaker 1 has two turns → two badges; use getAllByText.
    expect(screen.getAllByText('Speaker 1').length).toBe(2);
    expect(screen.getAllByText('Speaker 2').length).toBe(1);
    // 0-based labels were the pre-fix off-by-one — must never appear.
    expect(screen.queryByText('Speaker 0')).toBeNull();
    expect(screen.getByText('Yes, please.')).toBeTruthy();
  });

  it('renders plain unlabeled text without badges', () => {
    render(TranscriptView, { value: 'Just a plain transcript.' });
    expect(screen.queryByText('Speaker 1')).toBeNull();
    expect(screen.getByText('Just a plain transcript.')).toBeTruthy();
  });

  // Codie follow-up / repo-auditor legacy acceptance: recordings persisted
  // BEFORE the 1-based canonical fix carry 0-based bracketed labels. They
  // must render with a "Speaker 0" badge VERBATIM — never renumbered to
  // Speaker 1 (silent attribution change) and never rewritten. Rendering
  // is read-only: `value` must be unchanged after mount.
  it('renders legacy zero-based [Speaker 0] stored text verbatim, no renumber, no mutation', async () => {
    const legacy = [
      '00:00:01,340 --> 00:00:03,750 [Speaker 0]',
      'Old zero-based first turn.',
      '',
      '00:00:04,100 --> 00:00:06,000 [Speaker 1]',
      'Old second turn.',
    ].join('\n');
    // No persistence side effect: rendering must never fire onChange —
    // asserted on the spy (badge-text equality alone doesn't prove it).
    const onChange = vi.fn();
    render(TranscriptView, { value: legacy, onChange });
    // Verbatim: the stored numbers become the badges, unchanged.
    expect(screen.getAllByText('Speaker 0').length).toBe(1);
    expect(screen.getAllByText('Speaker 1').length).toBe(1);
    // No renumbering side effect: a "Speaker 2" badge must not be invented.
    expect(screen.queryByText('Speaker 2')).toBeNull();
    expect(screen.getByText('Old zero-based first turn.')).toBeTruthy();
    // Flush the parse debounce, then assert onChange never fired.
    await new Promise((r) => setTimeout(r, 50));
    expect(onChange).not.toHaveBeenCalled();
  });

  // Compatibility pin (review regression): the legacy COLON form
  // (`Speaker N: text` paragraphs) must keep rendering badges alongside
  // the bracketed canonical form — the bracketed parser supplements, it
  // does not replace.
  it('renders colon-formatted Speaker N: paragraphs with badges (both formats supported)', () => {
    const colonForm = [
      'Speaker 1: Hello, how are you today?',
      '',
      'Speaker 2: Fine, thanks.',
    ].join('\n');
    render(TranscriptView, { value: colonForm });
    expect(screen.getAllByText('Speaker 1').length).toBe(1);
    expect(screen.getAllByText('Speaker 2').length).toBe(1);
    expect(screen.getByText('Hello, how are you today?')).toBeTruthy();
  });

  // Mixed content: one transcript carrying both conventions — every
  // labeled paragraph must badge under its own format.
  it('renders mixed bracketed and colon formats in the same transcript', () => {
    const mixed = [
      '00:00:01,340 --> 00:00:03,750 [Speaker 1]',
      'Bracketed canonical turn.',
      '',
      'Speaker 2: Colon-formatted turn.',
    ].join('\n');
    render(TranscriptView, { value: mixed });
    expect(screen.getAllByText('Speaker 1').length).toBe(1);
    expect(screen.getAllByText('Speaker 2').length).toBe(1);
    expect(screen.getByText('Bracketed canonical turn.')).toBeTruthy();
    expect(screen.getByText('Colon-formatted turn.')).toBeTruthy();
  });
});

// Uncertainty-honesty: unlabeled sections must never silently read as a
// continuation of the previous speaker, and a transcript-level caveat must
// be visible above the scrolling transcript so a clinician knows speaker
// labels are unverified before quoting them.
describe('TranscriptView uncertainty honesty', () => {
  afterEach(cleanup);

  it('shows a "Speaker unassigned" heading for unlabeled sections between speakers', () => {
    const transcript = [
      'Speaker 1: Hello, how are you?',
      '',
      'Some background noise or unattributable speech.',
      '',
      'Speaker 2: I am fine, thanks.',
    ].join('\n');
    render(TranscriptView, { value: transcript });
    const headings = screen.getAllByText('Speaker unassigned');
    expect(headings.length).toBe(1);
    expect(screen.getByText('Some background noise or unattributable speech.')).toBeTruthy();
    // Must NOT say "Unknown speaker" (implies an extra identified person).
    expect(screen.queryByText('Unknown speaker')).toBeNull();
    expect(screen.queryByText(/unknown speaker/i)).toBeNull();
  });

  it('shows one heading per contiguous unlabeled block, never merged across attributed passages', () => {
    const transcript = [
      'Speaker 1: First turn.',
      '',
      'Unlabeled passage A.',
      '',
      'Speaker 2: Middle turn.',
      '',
      'Unlabeled passage B.',
      '',
      'Speaker 1: Final turn.',
    ].join('\n');
    render(TranscriptView, { value: transcript });
    const headings = screen.getAllByText('Speaker unassigned');
    expect(headings.length).toBe(2);
  });

  it('shows a status heading for an entirely-unlabeled diarized result (not silent plain text)', () => {
    // Multi-paragraph all-unlabeled text — looks like diarization ran but
    // produced no speaker labels. Must NOT render as silent plain text
    // (which would hide the fact that attribution was attempted).
    const transcript = [
      'First paragraph of unattributed speech.',
      '',
      'Second paragraph of unattributed speech.',
      '',
      'Third paragraph of unattributed speech.',
    ].join('\n');
    render(TranscriptView, { value: transcript });
    const headings = screen.getAllByText('Speaker unassigned');
    expect(headings.length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('First paragraph of unattributed speech.')).toBeTruthy();
    expect(screen.getByText('Second paragraph of unattributed speech.')).toBeTruthy();
  });

  it('displays a persistent transcript-level caveat above the sections', () => {
    const transcript = [
      'Speaker 1: Hello.',
      '',
      'Speaker 2: Hi there.',
    ].join('\n');
    render(TranscriptView, { value: transcript });
    const caveat = screen.getByText(
      'Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.',
    );
    expect(caveat).toBeTruthy();
    // Must NOT be a role=alert (descriptive, not an interrupting warning).
    expect(caveat.getAttribute('role')).not.toBe('alert');
  });

  it('keeps the caveat visible during editing', async () => {
    const transcript = 'Speaker 1: Hello.\n\nSpeaker 2: Hi.';
    render(TranscriptView, { value: transcript });
    // Click Edit to enter editing mode.
    const editButton = screen.getByText('Edit');
    await editButton.click();
    // Caveat must still be present in the DOM.
    expect(
      screen.getByText(
        'Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.',
      ),
    ).toBeTruthy();
  });

  it('preserves single-paragraph plain text rendering (no headings, no caveat)', () => {
    // Single paragraph, no speaker labels — diarization-disabled state.
    // Must remain plain text, not show "Speaker unassigned".
    render(TranscriptView, { value: 'Just a plain transcript.' });
    expect(screen.queryByText('Speaker unassigned')).toBeNull();
    expect(
      screen.queryByText(
        'Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.',
      ),
    ).toBeNull();
    expect(screen.getByText('Just a plain transcript.')).toBeTruthy();
  });

  // Codie test gap: rail-colour non-inheritance — an unassigned block must
  // never inherit the preceding speaker's accent colour. The dashed border
  // uses var(--border), not the previous speaker's palette entry.
  it('unassigned block rail uses neutral border, never inherits preceding speaker colour', () => {
    const transcript = [
      'Speaker 1: First attributed turn.',
      '',
      'Unattributable passage here.',
      '',
      'Speaker 2: Second attributed turn.',
    ].join('\n');
    render(TranscriptView, { value: transcript });
    // Find the unlabeled section by its heading.
    const unlabeledHeading = screen.getByText('Speaker unassigned');
    const unlabeledSection = unlabeledHeading.closest('.speaker-section.unlabeled');
    expect(unlabeledSection).toBeTruthy();
    // The unlabeled section must have the 'unlabeled' class, which applies
    // the dashed border style (verified in CSS: border-left: 3px dashed).
    expect(unlabeledSection!.classList.contains('unlabeled')).toBe(true);
    // Speaker sections must NOT have the unlabeled class (they get solid borders
    // with speaker-specific colors via inline style).
    const speaker1Section = screen.getByText('First attributed turn.').closest('.speaker-section');
    expect(speaker1Section).toBeTruthy();
    expect(speaker1Section!.classList.contains('unlabeled')).toBe(false);
  });

  // Codie Warning 1 fix: single-paragraph all-null diarized result must show
  // unassigned status via the outcome prop, never silently fall through to
  // plain text. This is the exact case the contract forbids.
  it('single-paragraph all-null diarized result shows unassigned status (outcome=completed)', () => {
    // Diarization ran but produced no speaker labels — segments exist but all
    // speaker fields are null. Must render structured view with "Speaker
    // unassigned" heading, not silent plain text.
    const segments = [
      { speaker: null, text: 'Single paragraph of unattributed speech.', start: 0, end: 5 },
    ];
    render(TranscriptView, {
      value: 'Single paragraph of unattributed speech.',
      segments,
      diarizationOutcome: 'completed',
    });
    // Must show the unassigned heading, not silent plain text.
    expect(screen.getByText('Speaker unassigned')).toBeTruthy();
    expect(screen.getByText('Single paragraph of unattributed speech.')).toBeTruthy();
    // Caveat must be visible (diarization ran).
    expect(
      screen.getByText(
        'Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.',
      ),
    ).toBeTruthy();
  });

  it('diarization failed outcome shows unassigned status for all-null segments', () => {
    // Diarization was attempted but errored — distinct from 'off' and from
    // 'completed with unassigned passages'.
    const segments = [
      { speaker: null, text: 'Diarization failed, no labels available.', start: 0, end: 5 },
    ];
    render(TranscriptView, {
      value: 'Diarization failed, no labels available.',
      segments,
      diarizationOutcome: 'failed',
    });
    // Must show structured view with unassigned heading.
    expect(screen.getByText('Speaker unassigned')).toBeTruthy();
    expect(screen.getByText('Diarization failed, no labels available.')).toBeTruthy();
  });

  it('diarization off with single-paragraph plain text remains plain (no headings)', () => {
    // Diarization disabled, single paragraph — must remain plain text even
    // when passed as segments with all-null speakers.
    const segments = [
      { speaker: null, text: 'Just plain text, no diarization attempted.', start: 0, end: 5 },
    ];
    render(TranscriptView, {
      value: 'Just plain text, no diarization attempted.',
      segments,
      diarizationOutcome: 'off',
    });
    // Must NOT show unassigned heading or caveat.
    expect(screen.queryByText('Speaker unassigned')).toBeNull();
    expect(
      screen.queryByText(
        'Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.',
      ),
    ).toBeNull();
    expect(screen.getByText('Just plain text, no diarization attempted.')).toBeTruthy();
  });
});
