// Writer/reader symmetry (review contract line I,
// docs/reviews/transcript-render-2026-09-13): actual Rust formatter output
// must be exercised through the production TranscriptView text fallback
// with segment metadata ABSENT — segments take precedence and would
// otherwise green-light a parser that never matches real formatter output.
//
// The fixture is the checked-in byte-exact output of
// format_transcript_with_speakers (src-tauri transcription inner.rs). It is
// pinned on the Rust side by `checked_in_fixture_equals_formatter_output`;
// if the stored format changes without regenerating the fixture, THAT test
// fails first. Never edit this file by hand — regenerate it with:
//   cargo test -p rust-medical-assistant regenerate_formatter_fixture -- --ignored
// @vitest-environment jsdom
import { render, screen, cleanup } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
// Vite ?raw import: the formatter's exact bytes, no transform.
import formatterOutput from './__fixtures__/formatter-speaker-unassigned.txt?raw';
import TranscriptView from './TranscriptView.svelte';

describe('TranscriptView writer/reader symmetry — real formatter output', () => {
  afterEach(cleanup);

  it('formatter fixture contains exactly one unassigned marker and two labeled turns', () => {
    // Guard the guard: if the fixture stops exercising the marker path the
    // round-trip below is vacuous.
    expect(formatterOutput).toContain('[Speaker unassigned]');
    expect(formatterOutput.match(/\[Speaker unassigned\]/g)!.length).toBe(1);
    expect(formatterOutput).toContain('[Speaker 1]');
    expect(formatterOutput).toContain('[Speaker 2]');
  });

  it('round-trips the unassigned middle span as its own section, metadata absent', async () => {
    // Segments deliberately NOT passed: the text fallback is the only
    // reader that runs for stored text without metadata (legacy rows,
    // copy-paste, and post-edit clears — see save path change).
    const onChange = vi.fn();
    render(TranscriptView, { value: formatterOutput, onChange });

    // Contract line A: Speaker 1 → unassigned → Speaker 2 stay three
    // separate sections in reading order; the middle span is NOT
    // attributed to Speaker 1 (the old fold / last-speaker inheritance).
    expect(screen.getAllByText('Speaker 1').length).toBe(1);
    expect(screen.getAllByText('Speaker 2').length).toBe(1);
    const headings = screen.getAllByText('Speaker unassigned');
    expect(headings.length).toBe(1);

    // The marker's words render as the unassigned section's body.
    expect(screen.getByText('Unattributable gap.')).toBeTruthy();
    expect(screen.getByText('Alpha turn.')).toBeTruthy();
    expect(screen.getByText('Beta turn.')).toBeTruthy();

    // The marker text itself must never surface as a visible speaker
    // badge — "[Speaker unassigned]" as a BADGE would be a real-speaker
    // mis-parse. (The heading above IS the unassigned presentation.)
    expect(screen.queryByText('[Speaker unassigned]')).toBeNull();

    // Rendering never persists (contract line J).
    await new Promise((r) => setTimeout(r, 50));
    expect(onChange).not.toHaveBeenCalled();
  });

  it('marker is not mis-parseable as a numbered speaker', () => {
    // If the marker ever matched the numbered-speaker arms, it would badge
    // as e.g. "Speaker unassigned" alongside real badges with palette
    // colors. The heading path renders it neutral; this pins that.
    render(TranscriptView, { value: formatterOutput });
    const badges = screen.getAllByText(/Speaker \d+/);
    expect(badges.length).toBe(2); // Speaker 1 + Speaker 2 only
  });
});
