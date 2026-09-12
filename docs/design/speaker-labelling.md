# Speaker Labelling (Diarization) — Restoration Design

Status: implemented on branch `feat/speaker-labelling-restoration`, default OFF, not merged until Codie review passes.

## Model

- `AppConfig.diarize: bool` (default `false`), crates/core/src/types/settings.rs, next to `max_speakers`. `#[serde(default)]` means every pre-existing config deserializes to the intended default-off; no explicit `migrate()` case is needed (documented on `migrate()`).
- `transcribe_recording_inner` (src-tauri/src/commands/transcription/inner.rs): per-call `diarize: Option<bool>` override falls back to `app_config.diarize`.
- Pipeline stage (src-tauri/src/commands/pipeline.rs): forwards `None` (no override) — the setting governs. Previously hard-coded `Some(false)`, which made the setting unreachable from the one-click pipeline.

## Numbering convention (B3)

"Speaker N" is 1-based on every surface, single source of truth:

- crates/stt-providers/src/merge.rs: `format!("Speaker {}", id + 1)` — labels attached to segments (metadata `transcript_segments`).
- Rich view (src/lib/components/TranscriptView.svelte): renders the metadata label verbatim; the text-fallback regex parses `00:00:01,340 --> 00:00:03,750 [Speaker N]` blocks emitted by the stored text.
- Stored/copied text (inner.rs `format_transcript_with_speakers`): passes the merge label through UNCHANGED as `[Speaker N]`. The pre-change code subtracted 1 (`n.saturating_sub(1)`), producing `[Speaker 0]`/`[Speaker 1]` against the rich view's `Speaker 1`/`Speaker 2` — an off-by-one between surfaces. Fixed by removing the conversion entirely rather than re-numbering, so no surface ever rewrites the label.

## Label semantics (B7 acceptance constraints)

- Labels render as neutral `Speaker 1` / `Speaker 2` / … — NEVER `Doctor` / `Patient`. The diarizer cannot determine clinical roles; any role inference would be fabrication.
- Attribution is machine-guessed and may be wrong. UI copy (Settings → Audio / STT) carries the caveat: labels are neutral, automatic, possibly incorrect.
- Translation-store isolation: `capture_stop` in src-tauri/src/commands/translation.rs builds its `SttConfig` with `diarize: false` hard-coded and an INVARIANT comment forbidding wiring it to `AppConfig.diarize`. The Provider/Patient-keyed translation store must never receive diarization Speaker-N output. The store consumes only `transcript.text`; with `diarize: false` segments never carry labels anyway.

## Outcome reporting (B1)

`diarization_outcome()` (pure, unit-tested, in inner.rs) classifies:

| State | Condition | Surfacing |
|---|---|---|
| NotRequested | diarize off (default) | none |
| Skipped | requested, `diarization_attempted == false` (models missing) | `diarization-warning` event → pipeline store warning "download models" |
| Failed(reason) | attempted, metadata `diarization_failed` non-null | `diarization-failed` event → pipeline store warning "failed, check logs"; `tracing::error!` with reason |
| SucceededNoSpeakers | attempted, no failure, zero labeled segments (single-speaker collapse) | `tracing::info!` only — NOT a failure |
| Succeeded | ≥1 labeled segment | none |

### B1 exact uncovered failure path (pre-change), source trace

Before this change the mid-run failure path was UNOBSERVABLE at the call-site:

1. crates/stt-providers/src/local_provider.rs:170-179 (pre-change; identical shape in remote_provider.rs:313-322): `SpeakerDiarizer::diarize` returns `Ok(Err(e))` or the `spawn_blocking` join fails → the match arm logs `warn!(error = %e, "Diarization failed — proceeding without speaker labels")` and substitutes `Vec::new()` for the turns, with `(turns, true)` returned as `(empty, attempted = true)`.
2. inner.rs (pre-change :291-300): the only signal consumed was `diarization_attempted`. With `attempted == true` the block emitted NOTHING — no event, no log at the call-site. A hard mid-run failure was therefore indistinguishable from a successful single-speaker collapse.
3. Fix: both providers now also return `diarization_failed: Option<String>` in the tuple and set it in the metadata; `diarization_outcome` classifies it as `Failed(reason)`; inner.rs emits `diarization-failed` + `tracing::error!`.

Not covered even now (deliberate, scoped): diarization failures never fail the transcription — the transcript is still saved, unlabeled. That trade-off (labels are best-effort polish; STT text is the product) is the providers' existing design and is retained.

## Settings UI

- Toggle "Identify speakers (diarization)" beside Max speakers in the Advanced transcription disclosure (Audio.svelte).
- Both controls disabled with an explanatory hint when the pyannote models are not both downloaded (frontend mirror of the backend's `supports_diarization()`, derived from `listPyannoteModels().downloaded`).
- Copy states the default-off state and the neutral/unverified nature of labels (Audio.svelte toggle hint + delete-model dialog; DiarizationModelsSection.svelte hints).

## Regression tests (B8)

Rust (src-tauri/src/commands/transcription/inner.rs `format_tests`):
- `single_speaker_collapse_yields_no_labels` — empty turns → no labels, no brackets, raw text.
- `diarization_outcome_skipped_when_models_missing`
- `diarization_outcome_failed_when_provider_reports_failure`
- `diarization_outcome_single_speaker_collapse_is_not_a_failure`
- `diarization_outcome_not_requested_when_off`
- `diarization_outcome_succeeded_with_labels`
- Updated `all_labeled_groups_by_speaker` / fold tests assert 1-based labels and absence of `[Speaker 0]`.

crates/core settings tests: `default_config_values` asserts `!config.diarize`.
