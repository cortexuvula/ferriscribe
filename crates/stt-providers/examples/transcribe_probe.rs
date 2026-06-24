//! Throwaway diagnostic binary: transcribe a WAV with three pipelines and compare.
//!
//! Usage: cargo run --release --example transcribe_probe -p medical-stt-providers -- <wav_path>
//!
//! Pipelines:
//!   A. BASELINE — linear-interp resample + Greedy { best_of: 1 } (matches current LocalSttProvider).
//!   B. BEAM     — linear-interp resample + BeamSearch { beam_size: 5 } (isolate sampling effect).
//!   C. BEAM+RES — rubato SincFixedIn resample + BeamSearch { beam_size: 5 } (combined fix).
//!
//! This is a diagnostic tool. Not used at runtime; not shipped.

use std::{env, fs, path::PathBuf, time::Instant};

use hound::{SampleFormat, WavReader};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn main() {
    let args: Vec<String> = env::args().collect();
    let wav = args.get(1).expect("pass a WAV path");
    let model_path = env::var("WHISPER_MODEL").unwrap_or_else(|_| {
        let home = env::var("HOME").unwrap();
        format!(
            "{home}/Library/Application Support/rust-medical-assistant/models/whisper/ggml-large-v3-turbo.bin"
        )
    });

    println!("WAV:   {wav}");
    println!("MODEL: {model_path}");
    println!();

    let (samples_f32, src_rate, channels) = load_wav_f32(wav);
    println!(
        "loaded: {} samples, {} Hz, {} ch, {:.1}s",
        samples_f32.len(),
        src_rate,
        channels,
        samples_f32.len() as f64 / src_rate as f64 / channels as f64,
    );
    println!();

    let ctx = WhisperContext::new_with_params(&model_path, WhisperContextParameters::default())
        .expect("load model");

    println!("=== A. BASELINE (linear resample + Greedy best_of=1) ===");
    let t = Instant::now();
    let audio_a = resample_linear(&samples_f32, src_rate, channels);
    let transcript_a = run_whisper(&ctx, &audio_a, SamplingStrategy::Greedy { best_of: 1 });
    println!("{:.1}s compute", t.elapsed().as_secs_f32());
    print_summary(&transcript_a);

    println!();
    println!("=== B. BEAM (linear resample + BeamSearch beam_size=5) ===");
    let t = Instant::now();
    let transcript_b = run_whisper(
        &ctx,
        &audio_a,
        SamplingStrategy::BeamSearch {
            beam_size: 5,
            patience: -1.0,
        },
    );
    println!("{:.1}s compute", t.elapsed().as_secs_f32());
    print_summary(&transcript_b);

    println!();
    println!("=== C. BEAM + PROPER RESAMPLE (rubato SincFixedIn + beam=5) ===");
    let t = Instant::now();
    let audio_c = resample_rubato(&samples_f32, src_rate, channels);
    let transcript_c = run_whisper(
        &ctx,
        &audio_c,
        SamplingStrategy::BeamSearch {
            beam_size: 5,
            patience: -1.0,
        },
    );
    println!("{:.1}s compute", t.elapsed().as_secs_f32());
    print_summary(&transcript_c);

    // Write full transcripts to /tmp for eyeballing.
    for (name, transcript) in [
        ("A_baseline", &transcript_a),
        ("B_beam", &transcript_b),
        ("C_beam_rubato", &transcript_c),
    ] {
        let out = PathBuf::from(format!("/tmp/probe_{name}.txt"));
        fs::write(&out, transcript.join("\n")).expect("write");
        println!("wrote {}", out.display());
    }
}

fn load_wav_f32(path: &str) -> (Vec<f32>, u32, u16) {
    let reader = WavReader::open(path).expect("open wav");
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<_, _>>()
            .expect("decode f32"),
        SampleFormat::Int => {
            let max = (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .collect::<Result<Vec<i32>, _>>()
                .expect("decode int")
                .into_iter()
                .map(|s| s as f32 / max)
                .collect()
        }
    };
    (samples, spec.sample_rate, spec.channels)
}

/// Mirror of `audio_prep::to_16k_mono_f32` — linear interpolation, NO anti-aliasing.
fn resample_linear(samples: &[f32], src_rate: u32, channels: u16) -> Vec<f32> {
    let chan = channels.max(1) as usize;
    let mono: Vec<f32> = if chan > 1 {
        samples
            .chunks_exact(chan)
            .map(|f| f.iter().sum::<f32>() / chan as f32)
            .collect()
    } else {
        samples.to_vec()
    };
    if src_rate == 16_000 {
        return mono;
    }
    let ratio = src_rate as f64 / 16_000.0;
    let out_len = (mono.len() as f64 / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let x = i as f64 * ratio;
        let i0 = x.floor() as usize;
        let i1 = (i0 + 1).min(mono.len().saturating_sub(1));
        let f = (x - i0 as f64) as f32;
        out.push(mono[i0] * (1.0 - f) + mono[i1] * f);
    }
    out
}

/// Proper resampling via rubato: polyphase sinc with anti-aliasing.
fn resample_rubato(samples: &[f32], src_rate: u32, channels: u16) -> Vec<f32> {
    let chan = channels.max(1) as usize;
    let mono: Vec<f32> = if chan > 1 {
        samples
            .chunks_exact(chan)
            .map(|f| f.iter().sum::<f32>() / chan as f32)
            .collect()
    } else {
        samples.to_vec()
    };
    if src_rate == 16_000 {
        return mono;
    }

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let chunk = 4096;
    let mut resampler = SincFixedIn::<f32>::new(16_000.0 / src_rate as f64, 2.0, params, chunk, 1)
        .expect("build resampler");

    let mut out: Vec<f32> =
        Vec::with_capacity((mono.len() as f64 * 16_000.0 / src_rate as f64) as usize + chunk);
    let mut pos = 0;
    let mut in_buf = vec![vec![0.0_f32; chunk]];
    while pos + chunk <= mono.len() {
        in_buf[0].copy_from_slice(&mono[pos..pos + chunk]);
        let o = resampler.process(&in_buf, None).expect("resample");
        out.extend_from_slice(&o[0]);
        pos += chunk;
    }
    // Flush tail with zero-padding.
    if pos < mono.len() {
        let tail = mono.len() - pos;
        in_buf[0][..tail].copy_from_slice(&mono[pos..]);
        in_buf[0][tail..].fill(0.0);
        let o = resampler.process(&in_buf, None).expect("resample tail");
        out.extend_from_slice(&o[0]);
    }
    out
}

fn run_whisper(ctx: &WhisperContext, audio: &[f32], sampling: SamplingStrategy) -> Vec<String> {
    let mut state = ctx.create_state().expect("state");
    let mut params = FullParams::new(sampling);
    params.set_language(Some("en"));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_translate(false);
    params.set_no_timestamps(false);
    state.full(params, audio).expect("whisper full");
    let n = state.full_n_segments();
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let seg = state.get_segment(i).expect("segment");
        let t = seg.to_str_lossy().expect("text").trim().to_owned();
        if !t.is_empty() {
            out.push(t);
        }
    }
    out
}

fn print_summary(segments: &[String]) {
    let total_words: usize = segments.iter().map(|s| s.split_whitespace().count()).sum();
    println!("{} segments, {} words", segments.len(), total_words);
    println!("--- first 3 segments ---");
    for s in segments.iter().take(3) {
        println!("  {}", s);
    }
    println!("--- last 3 segments ---");
    for s in segments
        .iter()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .iter()
        .rev()
    {
        println!("  {}", s);
    }
}
