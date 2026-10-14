# zcode prompt — Fix Whisper repetition-loop collapse on low-energy recordings

You are fixing a transcription failure in FerriScribe (Tauri 2 + Svelte 5 + Rust workspace, local-first AI medical scribe). A valid, non-silent clinical recording is being marked **Failed** with the misleading error `"Transcription produced no text — the recording may be silent or too short."` The audio is NOT silent and NOT corrupt — the bundled Whisper decoder collapses the whole recording into a repetition loop, and the app's anti-hallucination filter then discards everything. Do not touch the SOAP pipeline, the privacy model, or the audio-encryption layer.

## The bug (verified end-to-end, do not re-litigate)

A 24.8-minute recording (`Recording_2026-10-13_15-31-02_048.wav`, 48 kHz stereo Float32, `rms=0.0088`, `peak=1.0`, `signal_secs=1485`) fails transcription. The app log chain is:

```
Loaded WAV audio data samples=142630912 sample_rate=48000 channels=2 duration_secs=1485.7 peak_amplitude=1.000000 rms_level=0.008812
Local transcription complete segments=50 text_len=549
Transcription complete provider=local text_len=549 segments=50
dropping cross-segment repetition (whisper hallucination) text_len=10 run_len=50
Transcription produced no text — the recording may be silent or too short. provider=local segments=0
```

Root cause, proven by direct reproduction on the same decrypted audio with the same model (`ggml-large-v3-turbo`):
- The app's bundled decoder (**whisper.cpp 1.8.3**, via `whisper-rs 0.16` → `whisper-rs-sys 0.15.0`) collapses the entire recording into **50 identical "Thank you." segments** (a full repetition loop) under the app's decode parameters.
- The same audio + same model + same decode params through **whisper-cli 1.9.2** produces a **full coherent transcript (1,325 segments)**.
- The only variable is the whisper.cpp version (1.8.3 vs 1.9.2). This is a known class of whisper.cpp repetition-loop regression on low-energy audio, hardened in the 1.9.x line.
- `whisper-rs 0.16.0` is the latest on crates.io and still bundles whisper.cpp 1.8.3, so **a dependency bump to 1.9.x is NOT available** — the fix must be a decode-parameter retry, not a version bump.

The app's `filter_cross_segment_repetitions` (in `src-tauri/src/commands/transcription/helpers.rs:150`) is **working correctly** — it drops runs of 3+ identical segments. The defect is that when the *entire* transcript is one run, it drops everything and the empty-text guard (`inner.rs:360`) hard-fails a valid recording with no recovery path.

## Hard facts about the existing infrastructure (reuse; do not reinvent or guess)

- **Decode parameters are hardcoded** in `crates/stt-providers/src/whisper.rs` `WhisperTranscriber::transcribe` (lines ~201-241): `SamplingStrategy::Greedy { best_of: 1 }`, `set_no_context(true)`, `set_suppress_blank(true)`, `set_suppress_nst(true)`, `set_temperature(0.0)`, `set_temperature_inc(0.2)`. There is currently **no way to request beam search or disable no_context** from a caller.
- **`SttConfig`** (`crates/core/src/types/stt.rs:38`) has fields: `language`, `diarize`, `num_speakers`, `model`, `smart_formatting`, `profanity_filter`. It has **no decode-parameter knobs**.
- **The STT call site** is `src-tauri/src/commands/transcription/inner.rs:260` — `stt.transcribe(audio, config, token).await`. The config is built at `inner.rs:222-227`. The empty-text guard is at `inner.rs:360-371` (fires when `display_text.trim().is_empty()` after the repetition filters at `inner.rs:328-333`).
- **The local provider** is `crates/stt-providers/src/local_provider.rs` `LocalSttProvider::transcribe` (line 103). It resamples to 16 kHz mono (`audio_prep::to_16k_mono_f32`), trims trailing silence (`trim_trailing_silence`, threshold 0.01), then calls `WhisperTranscriber::transcribe(&audio_16k, language)` inside `spawn_blocking`. The `audio_16k` buffer is returned alongside the segments (line 138-146) so a retry can reuse it without re-resampling.
- **The repetition filters** run in `inner.rs` AFTER the provider returns: `filter_segment_repetitions` (`helpers.rs:110`) then `filter_cross_segment_repetitions` (`helpers.rs:150`). They mutate `transcript.segments` and rebuild `transcript.text`.
- **Cancellation** is threaded via a `CancellationToken`; the provider checks it before/after the blocking whisper call. A retry must respect the same token.
- **PHI rules (absolute, load-bearing):** never log transcript text, segment content, or anything that could carry clinical content — log counts/lengths/ids only. The existing filters already log `text_len`/`run_len` only; preserve that.
- **Binary name:** the app binary is `rust-medical-assistant`, NOT "ferriscribe".

## What you must implement

### 1. Make decode parameters configurable (the enabler)

Add a small decode-parameter surface so a caller can request a different sampling strategy. Concretely:

- Add fields to `SttConfig` (`crates/core/src/types/stt.rs`), e.g. `best_of: Option<u32>` and `no_context: Option<bool>` (or a single `decode_strategy` enum — your call, but keep it minimal and default-preserving). `Default` must keep the current behavior (greedy best_of=1, no_context=true) so nothing else changes.
- Thread these through `LocalSttProvider::transcribe` (`local_provider.rs`) into `WhisperTranscriber::transcribe` (`whisper.rs`). When `best_of` is `Some(n)` with `n > 1`, use `SamplingStrategy::BeamSearch { beam_size: n, patience: 1.0 }` instead of Greedy. When `no_context` is `Some(false)`, skip `set_no_context(true)`. Keep all other anti-hallucination params unchanged.

### 2. Retry with beam search when the transcript collapses to empty

In `src-tauri/src/commands/transcription/inner.rs`, when the empty-text guard fires (the `display_text.trim().is_empty()` branch at line 360), do **not** immediately mark the recording Failed. Instead:

- Retry the transcription **once** with beam search (`best_of: Some(5)`) and `no_context: Some(false)` — the two knobs that break repetition loops.
- Reuse the already-loaded `audio` (it's still in scope) and the same `stt` provider. Build a modified `SttConfig` from the original with the decode overrides set.
- Respect the cancellation token before and after the retry (same pattern as the first call).
- Run the SAME post-processing on the retry result: `filter_segment_repetitions`, `filter_cross_segment_repetitions`, then the speaker-attribution formatting (`format_transcript_with_speakers`), then re-check the empty-text guard.
- Only if the retry ALSO produces empty text should you mark the recording Failed with the existing error. If the retry succeeds, continue the normal path (vocabulary correction, persistence, etc.) exactly as the first-pass success would.
- Log the retry as a distinct event (e.g. `"transcription retried with beam search after empty first pass"`) with counts/lengths only — never content.

### 3. Regression test

Add a test that pins the retry behavior. The cleanest seam is the pure filter + a fake STT provider that returns a repetition-loop transcript on the first call and a real transcript on the second, asserting:
- the first-pass empty result triggers exactly one retry,
- the retry uses beam-search decode params,
- the final result is the retry's real content (not Failed),
- a second empty result still marks Failed.

Follow the existing test patterns in `inner.rs` / `helpers.rs` (the `mk_transcript`/`seg` helpers in `helpers.rs:1011-1035` are the model to copy). Do NOT add a test that requires a real ggml model file unless it is env-gated like the existing `FERRISCRIBE_STT_MODEL` test in `whisper.rs:467`.

## Deliverable

1. The `SttConfig` decode-parameter fields + `Default` preserving current behavior.
2. The `WhisperTranscriber` / `LocalSttProvider` threading of those params (beam search + no_context override).
3. The single retry-with-beam-search in `inner.rs` on empty first-pass output, with the same post-processing and cancellation discipline, and Failed only after a second empty result.
4. The regression test(s) above.
5. A short note in the code comment at the empty-text guard explaining WHY the retry exists (whisper.cpp 1.8.3 repetition-loop collapse on low-energy audio; whisper-rs 0.16.0 is the latest and still bundles 1.8.3, so a version bump is not available).

## Verification gates (run all and report each individually — do not chain into one opaque failure)

- `cargo test --workspace --lib` (summarize per-binary with `grep -E "^test result:"`, then total)
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `npx vitest run`
- `npm run check`

Report each gate's result explicitly. If a gate fails, fix it before claiming done. Do not claim a gate passed without running it.
