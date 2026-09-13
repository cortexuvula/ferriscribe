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

  it('shows a "Speaker not identified" heading for unlabeled sections between speakers', () => {
    const transcript = [
      'Speaker 1: Hello, how are you?',
      '',
      'Some background noise or unattributable speech.',
      '',
      'Speaker 2: I am fine, thanks.',
    ].join('\n');
    render(TranscriptView, { value: transcript });
    const headings = screen.getAllByText('Speaker not identified');
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
    const headings = screen.getAllByText('Speaker not identified');
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
    const headings = screen.getAllByText('Speaker not identified');
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
    // Must remain plain text, not show "Speaker not identified".
    render(TranscriptView, { value: 'Just a plain transcript.' });
    expect(screen.queryByText('Speaker not identified')).toBeNull();
    expect(
      screen.queryByText(
        'Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.',
      ),
    ).toBeNull();
    expect(screen.getByText('Just a plain transcript.')).toBeTruthy();
  });
});
