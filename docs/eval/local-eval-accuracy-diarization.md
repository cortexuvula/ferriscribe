# Local Eval: Accuracy + Diarization Harness (pre-registered protocol)

Status: PRE-REGISTERED 2026-09-13, before any variant was run.
Branch: `local-eval-harness` (worktree `.worktrees/local-eval-harness`).
Harness: `crates/stt-providers/examples/fs_local_eval.rs` (env-gated, local-only).

## Privacy (binding)

- Samples are REAL clinical recordings (PHI): `local-eval/clinical-samples/`.
- Guards (verified in place, DO NOT TOUCH): `.gitignore` PHI rule,
  `.git/info/exclude` belt-and-braces rule, `.git/hooks/pre-commit` blocking
  `git add -f` of `local-eval/`+`clinical-samples/` paths. 0 files tracked,
  0 in history, 0 on remote (verified 2026-09-13).
- Harness output policy: ONLY content-free metrics in the room/console —
  opaque clip IDs (clip-01..N), opaque span IDs (span-A/B/...), aggregate
  counts, runtimes. Transcript-bearing artifacts are written ONLY under
  `local-eval/harness-artifacts/` (gitignored). No transcript excerpts, no
  quoted phrases, no patient details in any report.
- Delete nothing from `local-eval/`.

## Clips (opaque inventory)

| ID      | Duration | Notes                       |
|---------|----------|-----------------------------|
| clip-01 | 345.6 s  |                              |
| clip-02 | 189.6 s  |                              |
| clip-03 | 30.1 s   | SHORTEST → hand-reference    |
| clip-04 | 187.2 s  |                              |

All four are 8 kHz mono MP3 at the source (the "as-shipped sample rate"
factor: the app resamples 8k→16k before Whisper; content above 4 kHz is
already absent at the source for every variant, so it is a constant across
variants, not a factor we can A/B on this set).

## Three-stage comparison (run BEFORE interpreting any variant deltas)

- Stage A: raw whisper decoder output (segments from `whisper.rs`).
- Stage B: stored transcript = A → `filter_segment_repetitions` →
  `filter_cross_segment_repetitions` (production transforms, now shared via
  `medical-processing::transcript_post`).
- Stage C: displayed/copied text = B → `format_transcript_with_speakers`.

Per clip+variant the harness reports: A segment/word counts, B dropped
words/segments (split into loop-collapses vs cross-segment runs dropped),
C unattributed spans under both attribution rules. Stage deltas are
measured independently; no loss is attributed to a stage that was not
measured.

## Variants (one factor at a time; same audio, same decode route)

| ID           | Factor            | Setting                                    |
|--------------|-------------------|--------------------------------------------|
| v0-baseline  | (production)      | greedy best_of=1, turbo, no_context=true   |
| v1-beam      | decoder strategy  | BeamSearch beam_size=5 (rest = V0)         |
| v2-context   | context carry     | no_context=false (rest = V0)               |
| v3-largev3   | model             | ggml-large-v3 (rest = V0)                  |

Fixed across variants: language=en, temperature 0.0/0.2 fallback,
suppress_blank, suppress_nst, token timestamps ON, production diarization
(pyannote, max_speakers=2), production audio path (afconvert MP3→16k,
matching the app's decode route for these files).

## Per-variant metrics (score mode, once the reference lands)

- substitutions / insertions / deletions SEPARATELY (substitutions are the
  clinical-risk class — always on their own row).
- speaker-attribution errors (spans matched >50% time-overlap to a
  reference span whose label differs; counted under BOTH attribution rules).
- repetition/hallucination regressions: in-segment loop collapses and
  cross-segment runs dropped per variant.
- runtime (decode ms, warm context, Metal).
- ElevenLabs agreement: SECONDARY, labelled DISAGREEMENT, never accuracy.
  EL has its own artifacts; it is a proxy reference, not truth.

## Acceptance gates (pre-registered BEFORE running)

Gates evaluated on clip-03 (the hand-referenced clip) plus content-free
whole-set regressions. A no-go is reported exactly like a go.

- G1 (stage attribution): A variant may only be recommended for a stage-
  specific fix if its stage deltas were individually measured. (Process
  gate — enforced by metrics.jsonl schema.)
- G2 (beam, v1): GO only if sub_count(v1) < sub_count(v0) on clip-03 AND
  no cross-segment repetition regression anywhere in the set (runs_dropped
  per clip must not exceed V0's by >0 on any clip) AND runtime ≤ 3× V0.
- G3 (context, v2): GO only if sub_count(v2) < sub_count(v0) on clip-03
  AND whole-set crossseg_runs(v2) ≤ crossseg_runs(v0). Context carry is a
  known repetition-loop risk; any run regression is an automatic NO-GO.
- G4 (large-v3, v3): GO only if sub_count(v3) < sub_count(v0) on clip-03
  AND runtime ≤ 3× V0. (large-v3 is ~4x slower on paper; if it can't stay
  within 3x it's not shippable for clinic-hour use.)
- G5 (word-window attribution): GO only if speaker_errors(wordwin) <
  speaker_errors(segment) on clip-03 under the matched-span count, with
  attribution_rule_flips reported for context. Unattributed-span counts
  under both rules are reported alongside (a rule that "wins" by labeling
  less is not a win).
- G6 (EL secondary): EL disagreement NEVER gates anything. Reported as
  `el_disagreement_secondary` only.
- G7 (reference honesty): until the filled template exists, every accuracy
  number is DISAGREEMENT vs EL or vs V0, and labelled as such. No number
  is called accuracy until Andre's hand reference lands.

## Reference procedure (v2 — whisper-independent rows, 2026-09-13 revision)

The v1 procedure (rows = spans of the current baseline decode) is RETIRED.
Round-trip finding: a fresh decode produced 3 spans where an earlier decode
had 4 and ElevenLabs' own segmentation also says 4 — a reference keyed to a
decode inherits that decode's blind spots, so its span count can never
establish coverage. A dropped utterance was invisible by construction.

1. `FS_EVAL_MODE=template` → `local-eval/harness-artifacts/reference/
   clip-03.review.txt`. Rows derive from the pyannote diarization turns
   over the AUDIO (whisper-independent): speech regions (split at speaker
   changes and gaps > 0.5 s, capped at 8 s per row) + silence-candidate
   rows, together partitioning [0, duration]. For clip-03 that is 7 speech
   rows + 3 silence rows; ElevenLabs independently suggests 4 cues (shown
   as by-ear comments only, never as the row source, never gating).
2. Format (`review-v2`, one line per row, 6 tab-separated fields — the
   terminal status field is never empty, so an editor stripping trailing
   whitespace cannot change the field count):
     <row-id> TAB <start-s> TAB <end-s> TAB <speaker> TAB <text> TAB <status>
   - `OK` + empty text = CONFIRMED SILENCE (nonempty ASR output mapped
     there scores as the INSERTION class).
   - `OK` + text = the true words (a row with text and no ASR cue scores
     as the DELETION class — dropped utterances are representable).
   - `???` or a deleted row = NOT REVIEWED; scoring refuses until every
     row is OK. The scorer audits the filled copy against the emitted
     review file (row-set diff), so deleting rows cannot skip review.
   - Missed utterance inside a long row = add `row-90+` with its interval
     and text. Human rows never collide with template rows.
   - Legacy v1 files are refused outright (format header check).
3. Save as `clip-03.filled.txt`; `FS_EVAL_MODE=score` re-scores all
   variants against the complete human-reviewed interval.
4. Segmentation invariance: WER is computed on concatenated tokens, so
   identical words in identical order score identically whether the
   decoder emitted one cue or ten. Cue-count/segmentation quality is a
   SEPARATE score row (`seg_boundary_error`, plus `hyp_cues`/`ref_cues`),
   and can never be gamed into a word-error win. Pinned by regression
   tests (`identical_words_score_identically_at_one_or_ten_cues`,
   `cue_count_difference_is_visible_only_in_seg_error_not_wer`), as are
   the format round trip (`round_trip_template_fill_and_editor_save_
   survives`, `editor_whitespace_mangling_cannot_change_field_count`),
   the v1 refusal, the not-reviewed audit, and the silence/speech
   insertion/deletion classes.

## Unverified / out of scope (stated plainly)

- The 8 kHz source ceiling is a constant across variants on this clip set;
  it is measured (inventory) but not A/B-able here.
- Overlapping speech remains the cascade's known limitation; this harness
  measures attribution deltas, it does not fix overlap.
- Speaker-error matching uses reference row windows derived from the
  diarization turns (whisper-independent since the v2 revision); if Andre's
  added rows shift a boundary, the >50% overlap match may mis-bin a span.
  Counted matches are reported (speaker_matches) so the denominator is
  always visible.
- Runtime measured on one machine (this Mac, Metal); relative ordering is
  the claim, not absolute ms.
