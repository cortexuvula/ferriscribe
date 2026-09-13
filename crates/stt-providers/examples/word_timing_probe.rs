//! Runtime-cost probe for token-level timestamps (content-free).
//!
//! Decodes one real audio clip through the PRODUCTION transcriber twice —
//! once with token timestamps enabled (the new default) and once with them
//! disabled (the pre-refinement decode) — and reports WALL-CLOCK time and
//! SEGMENT/TOKEN COUNTS only. No transcript text is read, printed, or
//! stored: the segments returned by `transcribe` are consumed purely for
//! counts. This is the runtime evidence the repo auditor requires for the
//! word-window attribution refinement.
//!
//! The "without" leg cannot go through `WhisperTranscriber::transcribe`
//! (which now hard-enables token timestamps), so it drives whisper-rs
//! directly with the SAME parameter set as production except
//! `token_timestamps(false)`. Both legs share one loaded model context so
//! the comparison isolates decode cost, not model-load cost.
//!
//! Run:
//!   FERRISCRIBE_STT_MODEL=<ggml bin> FERRISCRIBE_TIMING_CLIP=<16kHz mono wav> \
//!     cargo run -p medical-stt-providers --example word_timing_probe --release

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use medical_stt_providers::whisper::{WhisperContextCache, WhisperTranscriber};

fn main() {
    let model =
        std::env::var_os("FERRISCRIBE_STT_MODEL").expect("set FERRISCRIBE_STT_MODEL=<ggml bin>");
    let clip = std::env::var_os("FERRISCRIBE_TIMING_CLIP")
        .expect("set FERRISCRIBE_TIMING_CLIP=<16kHz mono 16-bit wav>");
    let (model, clip) = (PathBuf::from(model), PathBuf::from(clip));

    // ---- Decode the WAV (16 kHz mono i16) to f32 — same parser as the evals.
    let bytes = std::fs::read(&clip).expect("read wav");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut pos = 12;
    let mut data: Option<(usize, usize)> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let sz = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            data = Some((pos + 8, sz));
        }
        pos += 8 + sz + (sz % 2);
    }
    let (doff, dsz) = data.expect("data chunk");
    let samples: Vec<f32> = bytes[doff..doff + dsz]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
        .collect();
    let duration_s = samples.len() as f64 / 16000.0;

    // ---- Shared context: model load excluded from both timed legs.
    let ctx = Arc::new(
        WhisperContext::new_with_params(&model, WhisperContextParameters::default())
            .expect("load model"),
    );

    // ================= Leg 1: WITH token timestamps (production path) =====
    // A one-entry cache whose loader hands back the shared context — keeps
    // this leg on the production code path without a second model load.
    let cache = Arc::new(WhisperContextCache::new(Arc::new({
        let ctx = Arc::clone(&ctx);
        move |_path: &std::path::Path| Ok(Arc::clone(&ctx))
    })));
    let transcriber = WhisperTranscriber::new(model.clone(), cache);

    // Warm-up decode (excluded): first full() initializes Metal pipelines.
    let _ = transcriber
        .transcribe(&samples, Some("en"))
        .expect("warmup");

    let t0 = Instant::now();
    let segs = transcriber
        .transcribe(&samples, Some("en"))
        .expect("decode with token ts");
    let with_ms = t0.elapsed().as_millis();
    let n_segments_with = segs.len();
    let n_words: usize = segs.iter().map(|s| s.words.len()).sum();
    // Content-free: drop the segments without touching text.
    drop(segs);

    // ================= Leg 2: WITHOUT token timestamps ====================
    // Same parameter set as production `transcribe` except the flag under
    // test. Runs on the SAME loaded context.
    let decode_without = |samples: &[f32]| -> (u128, usize) {
        let mut state = ctx.create_state().expect("state");
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_translate(false);
        params.set_no_timestamps(false);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.2);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_no_context(true);
        params.set_token_timestamps(false); // <-- the only difference
        let t = Instant::now();
        state
            .full(params, samples)
            .expect("decode without token ts");
        let ms = t.elapsed().as_millis();
        (ms, state.full_n_segments() as usize)
    };

    let (_warm_ms, _warm_n) = decode_without(&samples); // warm-up, excluded
    let (without_ms, n_segments_without) = decode_without(&samples);

    // ---- Content-free report: wall-clock + counts only.
    println!(
        "RESULT: clip_duration_s={:.1} with_token_ts_ms={} without_token_ts_ms={} delta_ms={} delta_pct={:.1} segments_with={} segments_without={} timed_word_windows={}",
        duration_s,
        with_ms,
        without_ms,
        with_ms as i64 - without_ms as i64,
        (with_ms as f64 / without_ms as f64 - 1.0) * 100.0,
        n_segments_with,
        n_segments_without,
        n_words
    );
}
