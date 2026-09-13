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

## Reference procedure

1. `FS_EVAL_MODE=template` → `local-eval/harness-artifacts/reference/
   clip-03.template.txt` (shortest clip; spans from V0 baseline with opaque
   IDs and hypothesis speaker labels).
2. Andre fills each span: corrected text (empty = hallucination, delete)
   and S1/S2 (who spoke it). ~10 minutes for a 30 s clip.
3. Save as `clip-03.filled.txt`; `FS_EVAL_MODE=score` re-scores ALL
   variants against the true reference, re-runs the gates.

## Unverified / out of scope (stated plainly)

- The 8 kHz source ceiling is a constant across variants on this clip set;
  it is measured (inventory) but not A/B-able here.
- Overlapping speech remains the cascade's known limitation; this harness
  measures attribution deltas, it does not fix overlap.
- Speaker-error counting depends on reference span windows derived from
  V0's decode; if Andre's corrections move a span's true boundary, the
  >50% overlap match may mis-bin a span. Counted matches are reported
  (speaker_matches) so the denominator is always visible.
- Runtime measured on one machine (this Mac, Metal); relative ordering is
  the claim, not absolute ms.
