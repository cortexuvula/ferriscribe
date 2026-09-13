// Copy/export gate for the transcript tab — METADATA-ONLY contract.
//
// Pure module, no Svelte imports: EditorTab.svelte calls `copyDecision` on
// every Copy click — the tests and the component exercise the SAME function.
//
// SECURITY INVARIANT (contract correction, 2026-09-13): the transcript TEXT
// is user-editable, so NO text predicate can be evidence of safety — not
// marker presence, not a [Speaker prefix, not length, not shape. A user
// (or paste) appending one "[Speaker unassigned]" paragraph must not be
// able to flip a block to proceed. Block decisions therefore read ONLY
// system-written persisted fields:
//
//   transcript_format_version  (integer, written BY the transcription path
//                               at render time; >= 2 means the text was
//                               produced by the marker-emitting formatter,
//                               so unassigned spans are explicitly marked.
//                               Absent or < 2 = pre-fix or unknown.)
//   diarization_fold_evidence  (fold_possible | none_observed; written at
//                               transcript-segment clear time, commit
//                               a58924b; key absent = unknown)
//
// Decision table (one test per row):
//   version >= 2                      -> PROCEED (text explicitly represents
//                                        unassigned spans)
//   otherwise + fold_possible         -> BLOCK
//   otherwise + none_observed         -> PROCEED
//   otherwise + absent/unknown        -> BLOCK ("Speaker attribution cannot
//                                        be verified from the saved metadata.")
//
// Manual edits change NEITHER field: the edit path never writes the version
// or the flag, which is exactly why a typed marker cannot clear a block.
// A successful re-transcription atomically replaces the text AND both
// fields (fresh version >= 2, stale flag removed); a failed one preserves
// the old pair (the failure path writes only the status column).
//
// Wording rules pinned by test:
//   - fold_possible: states the transcript contains spans with no identified
//     speaker AND that saved metadata shows unlabelled speech after a
//     labelled span, so attribution cannot be verified. NEVER
//     "misattribution detected" — that claim needs evidence we don't have,
//     and recordings get retranscribed.
//   - unknown: exactly "Speaker attribution cannot be verified from the
//     saved metadata."
//   - NEITHER message asserts fabrication or pre-fix history.
//   - Remedy on both: re-transcribe for an honest copy, AND it may replace
//     manual corrections and yields a NEW attribution attempt, not
//     guaranteed-correct labels.

/**
 * Caveat line prepended to all copied transcripts. Must match the on-screen
 * TranscriptView caveat word-for-word (pinned by test).
 */
export const COPY_CAVEAT =
  '⚠ Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.\n\n';

/** Metadata key holding the system-written transcript format version. */
export const TRANSCRIPT_FORMAT_VERSION_KEY = 'transcript_format_version';

/**
 * Minimum format version whose renderer explicitly marks unassigned spans
 * (`[Speaker unassigned]`, never an inherited speaker label).
 */
export const MIN_EXPLICIT_MARKER_VERSION = 2;

/** Metadata key written by the backend at segment-clear time (a58924b). */
export const FOLD_EVIDENCE_KEY = 'diarization_fold_evidence';

/** Closed vocabulary the backend writes; anything else is unknown. */
const FOLD_EVIDENCE_VALUES = ['fold_possible', 'none_observed'] as const;

export type FoldEvidence = 'fold_possible' | 'none_observed' | 'unknown';

export type CopyAction =
  | { action: 'proceed' }
  | {
      action: 'block';
      /** Why the copy is blocked — wording pinned by test. */
      reason: string;
      /** Path forward, visible without hovering. */
      remedy: string;
    };

/**
 * Reason shown for fold_possible. States exactly what the evidence shows:
 * unlabelled spans exist AND saved metadata shows unlabelled speech after a
 * labelled span — so attribution cannot be verified. Deliberately does NOT
 * say "misattribution detected": that would assert a fabrication event,
 * which requires evidence we don't have.
 */
export const FOLD_POSSIBLE_REASON =
  'This transcript contains spans with no identified speaker, and the saved metadata shows unlabelled speech after a labelled span — speaker attribution cannot be verified.';

/** Reason shown when the fold-evidence key is absent (unknown). Exact wording pinned by test. */
export const UNKNOWN_REASON =
  'Speaker attribution cannot be verified from the saved metadata.';

/**
 * Remedy shown on BOTH blocks. Two honest halves: re-transcribing is the way
 * to an honest copy, AND it has costs — it may replace manual corrections
 * and produces a NEW attribution attempt whose labels are not guaranteed
 * correct.
 */
export const BLOCK_REMEDY =
  'Re-transcribe this recording to get an honest copy. Re-transcription may replace manual corrections you have made, and it produces a new attribution attempt — speaker labels are not guaranteed correct.';

/**
 * Read the persisted transcript format version.
 *
 * Transport, not inference: a system-written integer passes through; a
 * missing key, a non-number, or a non-integer yields null. The value is
 * NEVER derived from the text at read time — that is the whole point of
 * the field.
 */
export function transcriptFormatVersion(
  metadata: Record<string, unknown> | null,
): number | null {
  if (!metadata) return null;
  const raw = metadata[TRANSCRIPT_FORMAT_VERSION_KEY];
  if (typeof raw === 'number' && Number.isInteger(raw)) return raw;
  return null;
}

/**
 * Read the persisted fold-evidence verdict from recording metadata.
 * Transport, not inference: 'fold_possible'/'none_observed' pass through
 * verbatim; a missing key or any other value is UNKNOWN — never guessed,
 * never defaulted to "clean" (a stale or pre-flag recording must not read
 * as safe).
 */
export function classifyFoldEvidence(
  metadata: Record<string, unknown> | null,
): FoldEvidence {
  if (!metadata) return 'unknown';
  const raw = metadata[FOLD_EVIDENCE_KEY];
  if (typeof raw === 'string' && (FOLD_EVIDENCE_VALUES as readonly string[]).includes(raw)) {
    return raw as FoldEvidence;
  }
  return 'unknown';
}

/**
 * The copy decision. Pure: metadata in, proceed/block out — the TEXT is
 * deliberately not an input (user-controlled text is never evidence of
 * safety). EditorTab.svelte calls exactly this on Copy.
 */
export function copyDecision(
  metadata: Record<string, unknown> | null,
): CopyAction {
  // Row 1: system-written version proves the text was rendered by the
  // marker-emitting formatter — unassigned spans are explicitly represented.
  const version = transcriptFormatVersion(metadata);
  if (version !== null && version >= MIN_EXPLICIT_MARKER_VERSION) {
    return { action: 'proceed' };
  }

  // Pre-fix or unknown version: consult the persisted fold evidence.
  const evidence = classifyFoldEvidence(metadata);
  if (evidence === 'none_observed') return { action: 'proceed' };
  return {
    action: 'block',
    reason: evidence === 'fold_possible' ? FOLD_POSSIBLE_REASON : UNKNOWN_REASON,
    remedy: BLOCK_REMEDY,
  };
}

/**
 * Build the clipboard text: caveat prepended to the text being copied.
 * Draft routing: a non-null draftText (unsaved edit in progress) is copied
 * instead of the stored text — Copy exports what is on screen. The draft
 * has NO effect on the block decision (see copyDecision).
 */
export function buildCopyText(storedText: string, draftText: string | null): string {
  return COPY_CAVEAT + (draftText !== null ? draftText : storedText);
}
