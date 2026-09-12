// @vitest-environment jsdom
import { render, screen, cleanup } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';
import TranscriptView from './TranscriptView.svelte';

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
});
