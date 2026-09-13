// Decision-table coverage for the metadata-only copy gate.
// One test per row of the contract table, plus transport-safety edges.
// The transcript TEXT is deliberately not an input to copyDecision —
// user-controlled text can never be evidence of safety (2026-09-13
// contract correction). All fixtures synthetic.
import { describe, expect, it } from 'vitest';
import {
  BLOCK_REMEDY,
  FOLD_POSSIBLE_REASON,
  UNKNOWN_REASON,
  classifyFoldEvidence,
  copyDecision,
  transcriptFormatVersion,
} from './copyLogic';

describe('copyDecision — decision table (one test per row)', () => {
  it('row 1: transcript_format_version >= 2 -> PROCEED (marker-emitting formatter rendered the text)', () => {
    expect(copyDecision({ transcript_format_version: 2 })).toEqual({ action: 'proceed' });
    expect(copyDecision({ transcript_format_version: 3 })).toEqual({ action: 'proceed' });
    // Proceeds even alongside a fold_possible flag: the flag described the
    // SEGMENTS' ordering (formatter-independent); the version proves the
    // saved text explicitly represents unassigned spans. Without the
    // version taking precedence, every new honest recording containing
    // unassigned speech after labelled speech would be blocked forever.
    expect(
      copyDecision({ transcript_format_version: 2, diarization_fold_evidence: 'fold_possible' }),
    ).toEqual({ action: 'proceed' });
  });

  it('row 2: version absent/<2 + fold_possible -> BLOCK with the fold wording', () => {
    const decision = copyDecision({ diarization_fold_evidence: 'fold_possible' });
    expect(decision).toEqual({
      action: 'block',
      reason: FOLD_POSSIBLE_REASON,
      remedy: BLOCK_REMEDY,
    });
    // Version 1 explicitly = pre-fix formatter: same row.
    expect(copyDecision({ transcript_format_version: 1, diarization_fold_evidence: 'fold_possible' }).action).toBe('block');
  });

  it('row 3: version absent/<2 + none_observed -> PROCEED', () => {
    expect(copyDecision({ diarization_fold_evidence: 'none_observed' })).toEqual({
      action: 'proceed',
    });
  });

  it('row 4: version absent/<2 + flag absent/unknown -> BLOCK with the exact unknown wording', () => {
    expect(copyDecision(null)).toEqual({
      action: 'block',
      reason: UNKNOWN_REASON,
      remedy: BLOCK_REMEDY,
    });
    expect(copyDecision({})).toEqual({
      action: 'block',
      reason: 'Speaker attribution cannot be verified from the saved metadata.',
      remedy: BLOCK_REMEDY,
    });
    // Unrecognised flag value is unknown, never guessed as clean.
    expect(copyDecision({ diarization_fold_evidence: 'probably fine' }).action).toBe('block');
  });
});

describe('copyDecision — transport safety', () => {
  it('version must be a system-written integer; strings/floats/null read as absent', () => {
    expect(transcriptFormatVersion({ transcript_format_version: 2 })).toBe(2);
    expect(transcriptFormatVersion({ transcript_format_version: '2' })).toBeNull();
    expect(transcriptFormatVersion({ transcript_format_version: 2.5 })).toBeNull();
    expect(transcriptFormatVersion({ transcript_format_version: null })).toBeNull();
    expect(transcriptFormatVersion(null)).toBeNull();
    // A user cannot reach these fields by editing text; if a corrupt value
    // appears anyway, the decision falls to the flag row (block unless
    // none_observed) — never defaults to "clean".
    expect(copyDecision({ transcript_format_version: '2' }).action).toBe('block');
  });

  it('fold evidence passes through verbatim or reads as unknown — never re-derived', () => {
    expect(classifyFoldEvidence({ diarization_fold_evidence: 'fold_possible' })).toBe('fold_possible');
    expect(classifyFoldEvidence({ diarization_fold_evidence: 'none_observed' })).toBe('none_observed');
    expect(classifyFoldEvidence({})).toBe('unknown');
    expect(classifyFoldEvidence(null)).toBe('unknown');
  });
});

describe('wording pins', () => {
  it('fold_possible reason states the evidence, never asserts fabrication or pre-fix history', () => {
    const lower = FOLD_POSSIBLE_REASON.toLowerCase();
    expect(lower).toContain('no identified speaker');
    expect(lower).toContain('unlabelled speech after a labelled span');
    expect(lower).not.toContain('misattribut');
    expect(lower).not.toContain('fabricat');
    expect(lower).not.toContain('pre-fix');
    expect(lower).not.toContain('folded');
  });

  it('unknown reason is the exact contract sentence', () => {
    expect(UNKNOWN_REASON).toBe(
      'Speaker attribution cannot be verified from the saved metadata.',
    );
  });

  it('remedy names re-transcribe AND its costs: manual corrections may be replaced, labels not guaranteed', () => {
    expect(BLOCK_REMEDY).toContain('Re-transcribe');
    expect(BLOCK_REMEDY).toContain('may replace manual corrections');
    expect(BLOCK_REMEDY).toContain('not guaranteed correct');
  });
});
