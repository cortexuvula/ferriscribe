//! Local-only accuracy + diarization evaluation harness (content-free output).
//!
//! PRIVACY — READ BEFORE MODIFYING:
//! This example reads REAL clinical recordings (PHI) from a local,
//! gitignored directory. It writes transcript-bearing artifacts ONLY under
//! `local-eval/harness-artifacts/` (gitignored, never committed). Everything
//! it PRINTS is content-free: opaque clip IDs (clip-01..N), opaque span IDs
//! (span-A/B/...), aggregate counts, runtimes, and S/I/D counts. No
//! transcript text, no quoted phrases, no patient details may ever reach
//! stdout/stderr. Keep it that way.
//!
//! PROTOCOL (room-settled, docs/eval/local-eval-accuracy-diarization.md):
//! 1. THREE-STAGE COMPARISON FIRST: stage A (raw decoder output) ->
//!    stage B (stored transcript: repetition filters) -> stage C
//!    (displayed/copied text: speaker formatting). Measure each stage's
//!    delta before proposing any fix.
//! 2. ONE FACTOR AT A TIME, same audio, each variant scored separately:
//!    V0 baseline (production: greedy, turbo, no_context) -> V1 beam
//!    (beam_size 5) -> V2 context-on (no_context=false) -> V3 large-v3
//!    (greedy, else V0).
//! 3. PER-VARIANT METRICS: substitutions / insertions / deletions reported
//!    SEPARATELY (substitutions are the clinical-risk class), speaker-
//!    attribution error counts, repetition/hallucination regression counts,
//!    runtime. ElevenLabs agreement is a SECONDARY, separately-labelled
//!    DISAGREEMENT number — a proxy reference with its own artifacts,
//!    never "accuracy".
//! 4. REFERENCE: every number is DISAGREEMENT until the hand-corrected
//!    reference for the shortest clip lands (`template` mode generates it;
//!    `score` re-scores everything once it's filled in).
//! 5. DIARIZATION: A/B segment-span attribution (production) vs word-window
//!    attribution (the word-window-attribution branch rule) on the same
//!    decode and same diarization turns; measure the delta, don't assume it.
//!
//! Modes (FS_EVAL_MODE):
//!   inventory — clip inventory: durations + stored-transcript word counts.
//!   run       — decode all variants x clips; write per-variant artifacts
//!               (JSON with spans + word windows) and content-free
//!               metrics.jsonl (runtime, stage A/B/C deltas, attribution
//!               rule A/B).
//!   template  — hand-correction template for the SHORTEST clip (V0 stage-C
//!               text, one span per line, opaque span IDs, " || " correction
//!               columns). ~10 minutes to fill in by ear.
//!   score     — after the template is filled: per-variant S/I/D, speaker
//!               error counts, repetition counts vs the true reference; EL
//!               disagreement as a separate secondary number.
//!   report    — content-free summary from metrics.jsonl + scores.json.
//!
//! Run (local-only, env-gated; never in CI):
//!   FS_EVAL_MODE=run FS_EVAL_SAMPLES_DIR=local-eval/clinical-samples \
//!   FS_EVAL_MODEL_DIR="$HOME/Library/Application Support/rust-medical-assistant/models" \
//!   FS_EVAL_LARGEV3_DIR=~/Downloads/fs-eval-models \
//!     cargo run --release -p medical-stt-providers --example fs_local_eval

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

type Turn = medical_stt_providers::diarization::SpeakerTurn;

// ───────────────────────── variants ─────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Variant {
    /// Production config: greedy best_of=1, turbo, no_context=true.
    V0Baseline,
    /// Factor: decoder strategy (BeamSearch beam_size=5), rest as V0.
    V1Beam,
    /// Factor: context carry (no_context=false), rest as V0.
    V2Context,
    /// Factor: model (large-v3), rest as V0.
    V3Largev3,
}

impl Variant {
    fn id(self) -> &'static str {
        match self {
            Variant::V0Baseline => "v0-baseline",
            Variant::V1Beam => "v1-beam",
            Variant::V2Context => "v2-context",
            Variant::V3Largev3 => "v3-largev3",
        }
    }
    fn all() -> [Variant; 4] {
        [
            Variant::V0Baseline,
            Variant::V1Beam,
            Variant::V2Context,
            Variant::V3Largev3,
        ]
    }
    #[allow(dead_code)]
    fn from_id(id: &str) -> Variant {
        match id {
            "v0-baseline" => Variant::V0Baseline,
            "v1-beam" => Variant::V1Beam,
            "v2-context" => Variant::V2Context,
            "v3-largev3" => Variant::V3Largev3,
            other => panic!("unknown variant id {other}"),
        }
    }
    /// Model resolution: turbo comes from the app models dir; large-v3 from
    /// FS_EVAL_LARGEV3_DIR (2.9 GB eval-only download, not shipped).
    fn model_path(self, models_dir: &Path) -> PathBuf {
        match self {
            Variant::V3Largev3 => {
                let d = std::env::var_os("FS_EVAL_LARGEV3_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("local-eval/models"));
                d.join("ggml-large-v3.bin")
            }
            _ => models_dir.join("whisper").join("ggml-large-v3-turbo.bin"),
        }
    }
}

// ───────────────────────── artifacts (PHI, local-eval/ only) ─────────────────────────

/// One decoded span with word windows. Serialized to
/// harness-artifacts/run/<clip>/<variant>.json — LOCAL ONLY (PHI).
#[derive(Serialize, Deserialize, Debug, Clone)]
struct Span {
    /// Opaque span ID: span-A, span-B, ...
    id: String,
    start: f64,
    end: f64,
    text: String,
    /// Word-level windows (start, end, word) from whisper token timestamps.
    words: Vec<(f64, f64, String)>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct DiarizationArt {
    /// Production diarize(max_speakers=2): (speaker_id, start, end).
    turns: Vec<(usize, f64, f64)>,
}

/// Per-clip, per-variant stage texts, both attribution rules, stage counts.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct VariantArt {
    clip: String,
    variant: String,
    decode_ms: u128,
    dur_s: f64,
    /// Raw spans (stage A), opaque IDs, with word windows.
    spans_raw: Vec<Span>,
    /// Stage-B spans (post repetition-filters), opaque IDs, with inherited
    /// word windows.
    b_spans: Vec<Span>,
    /// Speaker per b_span under the production segment-span rule.
    speakers_segment_rule: Vec<Option<String>>,
    /// Speaker per b_span under the word-window rule.
    speakers_wordwin_rule: Vec<Option<String>>,
    /// Stage-A word count (raw).
    a_words: usize,
    /// Words dropped by the stage-B filters (A minus B).
    b_dropped_words: usize,
    /// Segments dropped by the stage-B filters.
    b_dropped_segments: usize,
    /// Segments whose text was collapsed by the in-segment loop filter.
    b_loop_collapses: usize,
    /// Runs of 3+ identical consecutive segments dropped by the cross-
    /// segment filter (repetition/hallucination regression count).
    b_crossseg_runs_dropped: usize,
}

#[derive(Serialize, Debug)]
struct MetricsLine {
    clip: String,
    variant: String,
    decode_ms: u128,
    dur_s: f64,
    a_segments: usize,
    a_words: usize,
    b_dropped_words: usize,
    b_dropped_segments: usize,
    b_loop_collapses: usize,
    b_crossseg_runs_dropped: usize,
    c_spans: usize,
    c_unattributed_segment_rule: usize,
    c_unattributed_wordwin_rule: usize,
    /// Spans whose label differs between the two attribution rules
    /// (label change or labeled<->unassigned).
    attribution_rule_flips: usize,
}

// ───────────────────────── entry ─────────────────────────

fn main() {
    let mode = std::env::var("FS_EVAL_MODE").unwrap_or_else(|_| "inventory".into());
    let samples_dir = std::env::var_os("FS_EVAL_SAMPLES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("local-eval/clinical-samples"));
    let artifacts = PathBuf::from(
        std::env::var("FS_EVAL_ARTIFACTS")
            .unwrap_or_else(|_| "local-eval/harness-artifacts".into()),
    );

    match mode.as_str() {
        "inventory" => inventory(&samples_dir),
        "run" => run_all(&samples_dir, &artifacts),
        "template" => make_template(&samples_dir, &artifacts),
        "score" => score(&artifacts, &samples_dir),
        "report" => report(&artifacts),
        other => panic!("unknown FS_EVAL_MODE={other}"),
    }
}

// ───────────────────────── clip loading ─────────────────────────

struct Clip {
    id: String,
    /// file stem, e.g. record-2026-09-08_170812
    stem: String,
    path: PathBuf,
}

fn collect_clips(samples_dir: &Path) -> Vec<Clip> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(samples_dir)
        .unwrap_or_else(|e| panic!("read samples dir {}: {e}", samples_dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "mp3"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no clips in {}", samples_dir.display());
    paths
        .into_iter()
        .enumerate()
        .map(|(i, path)| Clip {
            id: format!("clip-{:02}", i + 1),
            stem: path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string(),
            path,
        })
        .collect()
}

/// Decode MP3 -> 16 kHz mono f32 via afconvert into a temp WAV (macOS).
fn load_clip_16k(clip: &Clip) -> Vec<f32> {
    let tmp = std::env::temp_dir().join(format!("fs-eval-{}-{}.wav", std::process::id(), clip.id));
    let status = std::process::Command::new("afconvert")
        .args(["-f", "WAVE", "-d", "LEI16@16000", "-c", "1"])
        .arg(&clip.path)
        .arg(&tmp)
        .status()
        .expect("afconvert spawn");
    assert!(status.success(), "afconvert failed on {}", clip.id);
    let bytes = std::fs::read(&tmp).expect("read converted wav");
    let _ = std::fs::remove_file(&tmp);
    decode_wav_i16_bytes(&bytes)
        .into_iter()
        .map(|s| s as f32 / 32768.0)
        .collect()
}

fn decode_wav_i16_bytes(bytes: &[u8]) -> Vec<i16> {
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
    bytes[doff..doff + dsz]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

// ───────────────────────── opaque IDs / tokens ─────────────────────────

/// span-A, span-B, ..., span-Z, span-AA, ... (opaque; no content leaks).
fn span_id(i: usize) -> String {
    let mut id = String::new();
    let mut n = i;
    loop {
        id.insert(0, (b'A' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    format!("span-{id}")
}

/// Normalize a word for scoring: lowercase, strip surrounding punctuation.
fn norm_word(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(norm_word)
        .filter(|w| !w.is_empty())
        .collect()
}

// ───────────────────────── inventory ─────────────────────────

fn inventory(samples_dir: &Path) {
    for clip in collect_clips(samples_dir) {
        let samples = load_clip_16k(&clip);
        let dur = samples.len() as f64 / 16_000.0;
        let fs_rtf = samples_dir.join(format!("FerriScribe Transcript {}.rtf", clip.stem));
        let el_txt = samples_dir.join(format!("Elevenlabs Transcript {}.txt", clip.stem));
        let fs_words = fs_rtf
            .exists()
            .then(|| word_count_rtf(&fs_rtf))
            .unwrap_or(0);
        let el_words = el_txt
            .exists()
            .then(|| plain_word_count(&el_txt))
            .unwrap_or(0);
        println!(
            "CLIP {} dur_s={:.1} fs_words={} el_words={}",
            clip.id, dur, fs_words, el_words
        );
    }
}

/// Minimal RTF text extraction (word count only — content never printed).
fn strip_rtf(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' | '}' => {}
            '\\' => match chars.next() {
                Some('\\') | Some('{') | Some('}') => out.push(' '),
                Some('*') => {}
                Some(cmd) if cmd.is_ascii_alphabetic() => {
                    let mut word = String::new();
                    word.push(cmd);
                    while let Some(&n) = chars.peek() {
                        if n.is_ascii_alphabetic() {
                            word.push(n);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if matches!(chars.peek(), Some(' ')) {
                        chars.next();
                    }
                    if word == "par" || word == "line" {
                        out.push('\n');
                    }
                }
                Some('\'') => {
                    let h: String = chars.by_ref().take(2).collect();
                    if let Ok(b) = u8::from_str_radix(&h, 16) {
                        out.push(b as char);
                    }
                }
                _ => {}
            },
            _ => out.push(c),
        }
    }
    out
}

fn word_count_rtf(path: &Path) -> usize {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    strip_rtf(&raw).split_whitespace().count()
}

fn plain_word_count(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .split_whitespace()
        .count()
}

// ───────────────────────── run ─────────────────────────

fn run_all(samples_dir: &Path, artifacts: &Path) {
    use whisper_rs::{WhisperContext, WhisperContextParameters};

    let models_dir = PathBuf::from(
        std::env::var("FS_EVAL_MODEL_DIR")
            .expect("FS_EVAL_MODEL_DIR=<app models dir (contains whisper/, pyannote/)>"),
    );

    let run_dir = artifacts.join("run");
    std::fs::create_dir_all(&run_dir).expect("create run dir");
    let mut metrics_file = std::fs::File::create(run_dir.join("metrics.jsonl")).expect("metrics");

    let mut contexts: HashMap<PathBuf, Arc<WhisperContext>> = HashMap::new();
    let mut warmed: HashMap<PathBuf, bool> = HashMap::new();

    for clip in collect_clips(samples_dir) {
        let samples = load_clip_16k(&clip);
        let dur_s = samples.len() as f64 / 16_000.0;

        let clip_dir = run_dir.join(&clip.id);
        std::fs::create_dir_all(&clip_dir).expect("clip dir");

        // Diarization once per clip — the SAME turns feed both attribution
        // rules and all variants (attribution is decode-dependent only).
        eprintln!("diarize {} ({:.0}s)", clip.id, dur_s);
        let diar = diarize_clip(&models_dir, &samples);
        std::fs::write(
            clip_dir.join("diarization.json"),
            serde_json::to_string_pretty(&DiarizationArt {
                turns: diar
                    .iter()
                    .map(|t| (t.speaker_id, t.start, t.end))
                    .collect(),
            })
            .unwrap(),
        )
        .unwrap();

        for variant in Variant::all() {
            let model_path = variant.model_path(&models_dir);
            assert!(
                model_path.exists(),
                "model missing for {}: {} — set FS_EVAL_MODEL_DIR / FS_EVAL_LARGEV3_DIR",
                variant.id(),
                model_path.display()
            );
            let ctx = contexts
                .entry(model_path.clone())
                .or_insert_with(|| {
                    Arc::new(
                        WhisperContext::new_with_params(
                            &model_path,
                            WhisperContextParameters::default(),
                        )
                        .expect("load whisper model"),
                    )
                })
                .clone();

            // Warm-up (excluded): the first full() on a context builds the
            // Metal pipelines — one-time cost that must not pollute the
            // variant runtime comparison.
            if !warmed.get(&model_path).copied().unwrap_or(false) {
                let warm: Vec<f32> = samples[..(16_000 * 30).min(samples.len())].to_vec();
                let _ = decode(&ctx, &warm, variant);
                warmed.insert(model_path.clone(), true);
            }

            eprintln!("decode {} {}", clip.id, variant.id());
            let t0 = Instant::now();
            let spans_raw = decode(&ctx, &samples, variant);
            let decode_ms = t0.elapsed().as_millis();

            let art = build_stages(&clip.id, variant, decode_ms, spans_raw, &diar, dur_s);
            std::fs::write(
                clip_dir.join(format!("{}.json", variant.id())),
                serde_json::to_string_pretty(&art).unwrap(),
            )
            .unwrap();

            let m = metrics_line(&art);
            let mut line = serde_json::to_string(&m).unwrap();
            line.push('\n');
            metrics_file.write_all(line.as_bytes()).unwrap();
            metrics_file.flush().unwrap();

            println!(
                "RUN {} {} decode_ms={} a_seg={} a_w={} b_drop_w={} b_drop_seg={} collapses={} runs_dropped={} c_unattr(seg={}/{} ww={}/{}) rule_flips={}",
                m.clip,
                m.variant,
                m.decode_ms,
                m.a_segments,
                m.a_words,
                m.b_dropped_words,
                m.b_dropped_segments,
                m.b_loop_collapses,
                m.b_crossseg_runs_dropped,
                m.c_unattributed_segment_rule,
                m.c_spans,
                m.c_unattributed_wordwin_rule,
                m.c_spans,
                m.attribution_rule_flips
            );
        }
    }
    println!("RUN complete; artifacts under {}", artifacts.display());
}

/// Decode with the variant's single factor differing; everything else the
/// production parameter set. Token timestamps always ON (word windows feed
/// the wordwin attribution rule).
fn decode(ctx: &whisper_rs::WhisperContext, samples: &[f32], variant: Variant) -> Vec<Span> {
    use whisper_rs::{FullParams, SamplingStrategy};
    let mut state = ctx.create_state().expect("state");
    let mut params = match variant {
        Variant::V1Beam => FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: 5,
            patience: -1.0,
        }),
        _ => FullParams::new(SamplingStrategy::Greedy { best_of: 1 }),
    };
    params.set_language(Some("en"));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_translate(false);
    params.set_no_timestamps(false);
    params.set_temperature(0.0);
    params.set_temperature_inc(0.2);
    params.set_token_timestamps(true);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    // The variant's factor: V2 carries decoded context across segments.
    params.set_no_context(!matches!(variant, Variant::V2Context));

    state.full(params, samples).expect("decode");

    let n = state.full_n_segments();
    let mut spans = Vec::with_capacity(n as usize);
    for i in 0..n {
        let seg = state.get_segment(i).expect("segment");
        let text = seg.to_str_lossy().expect("seg text").trim().to_string();
        if text.is_empty() {
            continue;
        }
        let start = seg.start_timestamp() as f64 / 100.0;
        let end = seg.end_timestamp() as f64 / 100.0;
        // Word windows from token DTW timestamps (the worktree's extraction
        // rule: positive-width, clamped to segment bounds, >=10 ms).
        let mut words = Vec::new();
        for ti in 0..seg.n_tokens() {
            let Some(tok) = seg.get_token(ti) else {
                continue;
            };
            let td = tok.token_data();
            let (t0, t1) = (td.t0, td.t1);
            if t0 < 0 || t1 <= t0 {
                continue;
            }
            let w_start = (t0 as f64 / 100.0).clamp(start, end);
            let w_end = (t1 as f64 / 100.0).clamp(start, end);
            if w_end - w_start >= 0.01 {
                if let Ok(w) = tok.to_str() {
                    let t = w.trim();
                    if !t.is_empty() {
                        words.push((w_start, w_end, t.to_string()));
                    }
                }
            }
        }
        spans.push(Span {
            id: String::new(),
            start,
            end,
            text,
            words,
        });
    }
    spans
}

/// Production diarization on the clip's 16k samples (max_speakers=2).
fn diarize_clip(models_dir: &Path, samples_f32: &[f32]) -> Vec<Turn> {
    let seg_model = models_dir.join("pyannote").join("segmentation-3.0.onnx");
    let emb_model = models_dir
        .join("pyannote")
        .join("wespeaker_en_voxceleb_CAM++.onnx");
    assert!(
        seg_model.exists() && emb_model.exists(),
        "pyannote models missing under {}",
        models_dir.display()
    );
    let i16s: Vec<i16> = samples_f32.iter().map(|&s| (s * 32767.0) as i16).collect();
    let diarizer = medical_stt_providers::diarization::SpeakerDiarizer::new(seg_model, emb_model);
    diarizer.diarize(&i16s, 16000, Some(2)).expect("diarize")
}

/// Run the 3-stage pipeline over raw spans; compute both attribution rules.
/// Stages B/C use the SHARED production transforms
/// (medical-processing::transcript_post) — not a copy.
fn build_stages(
    clip_id: &str,
    variant: Variant,
    decode_ms: u128,
    spans_raw: Vec<Span>,
    turns: &[Turn],
    dur_s: f64,
) -> VariantArt {
    let a_words: usize = spans_raw
        .iter()
        .map(|s| s.text.split_whitespace().count())
        .sum();
    let a_segments = spans_raw.len();

    let seg_of = |s: &Span| medical_core::types::stt::TranscriptSegment {
        text: s.text.clone(),
        start: s.start,
        end: s.end,
        speaker: None,
        confidence: None,
    };
    let mk_transcript = |segs: Vec<medical_core::types::stt::TranscriptSegment>| {
        medical_core::types::stt::Transcript {
            text: segs
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            segments: segs,
            language: Some("en".into()),
            duration_seconds: Some(dur_s),
            provider: "harness".into(),
            metadata: serde_json::json!({}),
        }
    };

    let mut transcript = mk_transcript(spans_raw.iter().map(|s| seg_of(s)).collect());

    // In-segment loop collapses (count before any drops).
    let mut probe = transcript.clone();
    medical_processing::transcript_post::filter_segment_repetitions(&mut probe);
    let b_loop_collapses = probe
        .segments
        .iter()
        .zip(transcript.segments.iter())
        .filter(|(p, o)| p.text != o.text)
        .count();

    medical_processing::transcript_post::filter_segment_repetitions(&mut transcript);

    // Cross-segment runs of 3+ identical consecutive segments (exact count
    // on the pre-filter list), then apply the filter.
    let b_crossseg_runs_dropped = count_identical_runs(&transcript);
    medical_processing::transcript_post::filter_cross_segment_repetitions(&mut transcript);

    let b_segments = transcript.segments.len();
    let b_words: usize = transcript
        .segments
        .iter()
        .map(|s| s.text.split_whitespace().count())
        .sum();

    // Stage-B spans with opaque IDs; word windows inherited from the raw
    // span with maximal time overlap (filters drop/collapse text, never
    // shift timings, so overlap matching is exact for survivors).
    let mut b_spans: Vec<Span> = transcript
        .segments
        .iter()
        .map(|s| Span {
            id: String::new(),
            start: s.start,
            end: s.end,
            text: s.text.clone(),
            words: Vec::new(),
        })
        .collect();
    for b in &mut b_spans {
        let overlap = |x: &Span| (x.end.min(b.end) - x.start.max(b.start)).max(0.0);
        if let Some(raw) = spans_raw
            .iter()
            .filter(|r| overlap(r) > 0.0)
            .max_by(|a, c| {
                overlap(a)
                    .partial_cmp(&overlap(c))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        {
            b.words = raw.words.clone();
        }
    }
    for (i, b) in b_spans.iter_mut().enumerate() {
        b.id = span_id(i);
    }

    // ── Attribution rule A/B (same decode, same turns) ──
    // Rule 1 (production, master): the app's merge module on segments with
    // no word windows — its segment-window path IS today's production rule.
    let ws_no_words: Vec<medical_stt_providers::whisper::WhisperSegment> = b_spans
        .iter()
        .map(|s| medical_stt_providers::whisper::WhisperSegment {
            text: s.text.clone(),
            start: s.start,
            end: s.end,
        })
        .collect();
    let merged = medical_stt_providers::merge::merge_segments_with_speakers(&ws_no_words, turns);
    let speakers_segment_rule: Vec<Option<String>> =
        merged.iter().map(|s| s.speaker.clone()).collect();

    // Rule 2 (word-window): per-word overlap voting when >=2 valid windows,
    // else the segment-window fallback (word-window-attribution branch).
    let speakers_wordwin_rule: Vec<Option<String>> = b_spans
        .iter()
        .map(|s| attribute_wordwin(s, turns))
        .collect();

    VariantArt {
        clip: clip_id.to_string(),
        variant: variant.id().to_string(),
        decode_ms,
        dur_s,
        spans_raw: spans_raw
            .into_iter()
            .enumerate()
            .map(|(i, mut s)| {
                s.id = span_id(i);
                s
            })
            .collect(),
        b_spans,
        speakers_segment_rule,
        speakers_wordwin_rule,
        a_words,
        b_dropped_words: a_words.saturating_sub(b_words),
        b_dropped_segments: a_segments.saturating_sub(b_segments),
        b_loop_collapses,
        b_crossseg_runs_dropped,
    }
}

/// Count runs of 3+ identical consecutive (case-insensitive, trimmed)
/// segments — the cross-segment repetition-regression metric.
fn count_identical_runs(t: &medical_core::types::stt::Transcript) -> usize {
    const MIN_RUN: usize = 3;
    let segs = &t.segments;
    if segs.len() < MIN_RUN {
        return 0;
    }
    let mut runs = 0usize;
    let mut i = 0usize;
    while i < segs.len() {
        let key = segs[i].text.trim().to_lowercase();
        if key.is_empty() {
            i += 1;
            continue;
        }
        let mut run = 1usize;
        while i + run < segs.len() && segs[i + run].text.trim().to_lowercase() == key {
            run += 1;
        }
        if run >= MIN_RUN {
            runs += 1;
        }
        i += run;
    }
    runs
}

/// Word-window attribution rule (word-window-attribution branch): with
/// >=2 valid word windows, per-word overlap votes with dominance >=0.7;
/// otherwise the whole-segment window rule (10 ms floor, dominance 0.7).
fn attribute_wordwin(span: &Span, turns: &[Turn]) -> Option<String> {
    const MIN_OVERLAP_S: f64 = 0.01;
    const DOMINANCE: f64 = 0.7;
    const MIN_WORD_WINDOWS: usize = 2;

    let vote = |windows: &[(f64, f64)]| -> Option<String> {
        let mut votes: HashMap<usize, f64> = HashMap::new();
        for &(ws, we) in windows {
            for t in turns {
                let ov = (we.min(t.end) - ws.max(t.start)).max(0.0);
                if ov > MIN_OVERLAP_S {
                    *votes.entry(t.speaker_id).or_insert(0.0) += ov;
                }
            }
        }
        let total: f64 = votes.values().sum();
        if votes.is_empty() || total < MIN_OVERLAP_S {
            return None;
        }
        let (&best, &best_ov) = votes
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        if best_ov / total >= DOMINANCE {
            Some(format!("Speaker {}", best + 1))
        } else {
            None
        }
    };

    if span.words.len() >= MIN_WORD_WINDOWS {
        let windows: Vec<(f64, f64)> = span.words.iter().map(|w| (w.0, w.1)).collect();
        if let Some(label) = vote(&windows) {
            return Some(label);
        }
        // No dominant speaker at word level — ambiguous, not a fallback case.
        // (Mirrors the branch: empty votes -> None; dominance failure -> None.)
        let has_any = span.words.iter().any(|w| {
            turns
                .iter()
                .any(|t| (w.1.min(t.end) - w.0.max(t.start)).max(0.0) > MIN_OVERLAP_S)
        });
        if has_any {
            return None;
        }
    }
    vote(&[(span.start, span.end)])
}

// ───────────────────────── metrics ─────────────────────────

fn metrics_line(art: &VariantArt) -> MetricsLine {
    let c_spans = art.b_spans.len();
    MetricsLine {
        clip: art.clip.clone(),
        variant: art.variant.clone(),
        decode_ms: art.decode_ms,
        dur_s: art.dur_s,
        a_segments: art.spans_raw.len(),
        a_words: art.a_words,
        b_dropped_words: art.b_dropped_words,
        b_dropped_segments: art.b_dropped_segments,
        b_loop_collapses: art.b_loop_collapses,
        b_crossseg_runs_dropped: art.b_crossseg_runs_dropped,
        c_spans,
        c_unattributed_segment_rule: art
            .speakers_segment_rule
            .iter()
            .filter(|s| s.is_none())
            .count(),
        c_unattributed_wordwin_rule: art
            .speakers_wordwin_rule
            .iter()
            .filter(|s| s.is_none())
            .count(),
        attribution_rule_flips: art
            .speakers_segment_rule
            .iter()
            .zip(art.speakers_wordwin_rule.iter())
            .filter(|(a, b)| a != b)
            .count(),
    }
}

// ───────────────────────── template (hand-correction) ─────────────────────────

/// Generate the hand-correction template for the SHORTEST clip from the V0
/// baseline artifacts. Output (PHI, local-only):
///   local-eval/harness-artifacts/reference/<clip>.template.txt
/// Format per span (one line each):
///   <opaque-id> \t <hypothesis text> \t || \t <correction> \t || \t <speaker>
/// Andre fills the last two columns by ear: corrected text (empty = span is
/// a hallucination, delete it) and S1/S2 (who actually spoke it).
fn make_template(samples_dir: &Path, artifacts: &Path) {
    let clips = collect_clips(samples_dir);
    let mut best: Option<(&Clip, f64)> = None;
    for c in &clips {
        let dur_s = load_clip_16k(c).len() as f64 / 16_000.0;
        if best.as_ref().is_none_or(|(_, d)| dur_s < *d) {
            best = Some((c, dur_s));
        }
    }
    let (shortest, dur_s) = best.expect("at least one clip");

    let art_path = artifacts
        .join("run")
        .join(&shortest.id)
        .join("v0-baseline.json");
    let art: VariantArt = serde_json::from_str(&std::fs::read_to_string(&art_path).unwrap())
        .unwrap_or_else(|e| {
            panic!(
                "run artifacts missing for {} — run FS_EVAL_MODE=run first: {e}",
                shortest.id
            )
        });

    let ref_dir = artifacts.join("reference");
    std::fs::create_dir_all(&ref_dir).unwrap();
    let tpl_path = ref_dir.join(format!("{}.template.txt", shortest.id));
    let mut out = String::new();
    out.push_str("# Hand-correction template (PHI — local only, never commit)\n");
    out.push_str(&format!(
        "# Clip: {} Duration: {dur_s:.1}s; spans from the current baseline decode.\n",
        shortest.id
    ));
    out.push_str("# For each span: fix the text (empty correction = span is a hallucination,\n");
    out.push_str("# delete it), and mark who spoke it: S1 or S2. Keep the span-ID prefix!\n");
    out.push_str("# Per span TWO lines: (1) <span-id> TAB [hyp-speaker] TAB || TAB <corrected text> TAB || TAB <S1|S2|->\n");
    out.push_str("#                    (2) an indented line showing what the app transcribed (do not edit).\n");
    out.push_str("# Fill column <corrected text> with the app text if it was right (copy it).\n");
    out.push_str(
        "# Speaker '-' = leave unassigned. Save the filled file as <clip>.filled.txt.\n\n",
    );
    for (i, span) in art.b_spans.iter().enumerate() {
        let hyp_speaker = art
            .speakers_segment_rule
            .get(i)
            .cloned()
            .flatten()
            .unwrap_or_else(|| "unassigned".into());
        out.push_str(&format!("{}\t[{}]\t||\t\t||\t\n", span.id, hyp_speaker));
        // Text on its own indented line: tabs inside text would break TSV.
        out.push_str(&format!("    {}\n", span.text));
    }
    std::fs::write(&tpl_path, out).unwrap();
    println!(
        "TEMPLATE {} spans={} dur_s={:.1} path={}",
        shortest.id,
        art.b_spans.len(),
        dur_s,
        tpl_path.display()
    );
}

// ───────────────────────── score ─────────────────────────

/// One span of the hand-corrected reference.
#[derive(Debug, Clone)]
struct RefSpan {
    id: String,
    text: String,
    speaker: Option<usize>, // 1-based S1/S2 -> 0/1
}

/// Parse the filled template. Accepted filled-line shape (two lines per
/// span, as emitted by `template`):
///   <span-id> \t [<hyp speaker>] \t || \t <corrected text> \t || \t <S1|S2>
///   (the hypothesis text line that follows is ignored)
/// A span with empty corrected text and empty speaker = deleted
/// (hallucination). Unfilled lines (both columns empty AND no speaker) are
/// treated as "not corrected yet" and STOP scoring with a clear error.
/// Content-free placeholder for panic messages: the leading span-ID token
/// only (never transcript text — privacy: PHI must not reach the console).
fn id_placeholder(line: &str) -> String {
    line.split('\t')
        .next()
        .unwrap_or("<no-id>")
        .trim()
        .to_string()
}

fn parse_template(path: &Path) -> Vec<RefSpan> {
    let raw = std::fs::read_to_string(path).expect("read filled template");
    let mut out = Vec::new();
    let mut lines = raw.lines().peekable();
    while let Some(line) = lines.next() {
        // Strip ONLY line endings. trim_end() here would eat the trailing
        // TAB of an unfilled row (6 TSV fields -> 5), so the "not fully
        // filled" panic below was unreachable and unfilled spans were
        // silently skipped — scoring against an EMPTY reference (found
        // 2026-09-13: filled==template byte-identical, wer=NaN output).
        let line = line.trim_end_matches(['\r', '\n']);
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if !line.contains("||") {
            // hypothesis-text line (indented) — skip
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        // parts: [id, "[spk]", "||", corr, "||", spk] (corr may be empty)
        if parts.len() < 6 {
            // A ||-row with fewer than 6 TSV fields is malformed (trailing
            // tab stripped by an editor, or hand-mangled). Silently
            // skipping it silently drops a reference span — hard error
            // instead (regression: 2026-09-13 empty-reference incident).
            panic!(
                "malformed reference line ({} tab fields, expected 6): {:?}\
                 — keep the span-ID prefix and both || separators",
                parts.len(),
                id_placeholder(&line)
            );
        }
        let id = parts[0].trim().to_string();
        let corr = parts[3].trim().to_string();
        let spk = parts[5].trim().to_string();
        if corr.is_empty() && spk.is_empty() {
            panic!(
                "template not fully filled: span {id} has no correction and no speaker — \
                 fill every span (use explicit '-' speaker for spans you want deleted)"
            );
        }
        let speaker = match spk.as_str() {
            "S1" => Some(0),
            "S2" => Some(1),
            "-" => None,
            other => panic!("span {id}: speaker must be S1, S2, or '-', got {other:?}"),
        };
        out.push(RefSpan {
            id,
            text: corr,
            speaker,
        });
    }
    out
}

/// Word-level S/I/D via Levenshtein backtrace (reference -> hypothesis).
/// Returns (substitutions, insertions, deletions).
fn sid(reference: &[String], hypothesis: &[String]) -> (usize, usize, usize) {
    let m = reference.len();
    let n = hypothesis.len();
    // DP matrix of ops; m,n are small (hundreds..thousands).
    let mut dp = vec![vec![(0usize, 0usize, 0usize); n + 1]; m + 1];
    for (i, row) in dp.iter_mut().enumerate().skip(1) {
        row[0] = (i, 0, 0); // all deletions
    }
    for j in 1..=n {
        dp[0][j] = (0, j, 0); // all insertions
    }
    for i in 1..=m {
        for j in 1..=n {
            let cost = if reference[i - 1] == hypothesis[j - 1] {
                0
            } else {
                1
            };
            let del = dp[i - 1][j].0 + dp[i - 1][j].1 + dp[i - 1][j].2 + 1;
            let ins = dp[i][j - 1].0 + dp[i][j - 1].1 + dp[i][j - 1].2 + 1;
            let sub = dp[i - 1][j - 1].0 + dp[i - 1][j - 1].1 + dp[i - 1][j - 1].2 + cost;
            dp[i][j] = if sub <= del && sub <= ins {
                (
                    dp[i - 1][j - 1].0 + cost,
                    dp[i - 1][j - 1].1,
                    dp[i - 1][j - 1].2,
                )
            } else if del <= ins {
                (dp[i - 1][j].0, dp[i - 1][j].1, dp[i - 1][j].2 + 1)
            } else {
                (dp[i][j - 1].0, dp[i][j - 1].1 + 1, dp[i][j - 1].2)
            };
        }
    }
    let (s, in_, d) = dp[m][n];
    (s, in_, d)
}

#[derive(Serialize, Debug)]
struct ScoreLine {
    clip: String,
    variant: String,
    /// Vs the hand reference: substitutions (clinical-risk class).
    sub: usize,
    ins: usize,
    del: usize,
    ref_words: usize,
    hyp_words: usize,
    /// Word error rate vs hand reference: (S+I+D)/ref_words.
    wer: f64,
    /// Speaker errors: spans matched to reference spans by time overlap
    /// (>50% of the variant span inside a reference span) whose speaker
    /// label differs (incl. labeled vs unassigned). Count only.
    speaker_errors_matched: usize,
    speaker_matches: usize,
    /// Repetition regressions: cross-segment runs dropped (from run stage).
    crossseg_runs_dropped: usize,
    /// SECONDARY — labelled disagreement, NOT accuracy: ElevenLabs whole-
    /// clip word disagreement (S+I+D)/max(ref,el) between the variant's
    /// stage-B text and the EL transcript. EL is a proxy with its own
    /// artifacts.
    el_disagreement_secondary: f64,
    /// Attribution-rule A/B on this clip/variant (from run stage).
    attribution_rule_flips: usize,
}

fn score(artifacts: &Path, samples_dir: &Path) {
    let run_dir = artifacts.join("run");
    let ref_dir = artifacts.join("reference");

    // Locate the filled template (any *.filled.txt).
    let filled: Vec<PathBuf> = std::fs::read_dir(&ref_dir)
        .expect("reference dir (run FS_EVAL_MODE=template first)")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|x| x == "txt")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".filled.txt"))
        })
        .collect();
    assert!(
        !filled.is_empty(),
        "no *.filled.txt under {}",
        ref_dir.display()
    );
    assert_eq!(filled.len(), 1, "exactly one filled reference expected");

    let clip_id = filled[0]
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .trim_end_matches(".filled.txt")
        .to_string();
    let reference = parse_template(&filled[0]);

    // Reference tokens + per-span windows for speaker matching.
    let art_v0: VariantArt = serde_json::from_str(
        &std::fs::read_to_string(run_dir.join(&clip_id).join("v0-baseline.json")).unwrap(),
    )
    .expect("v0 artifacts");
    let ref_window_of: HashMap<&str, (f64, f64)> = art_v0
        .b_spans
        .iter()
        .map(|s| (s.id.as_str(), (s.start, s.end)))
        .collect();

    let ref_tokens: Vec<String> = reference.iter().flat_map(|r| tokens(&r.text)).collect();
    let ref_words = ref_tokens.len();
    // A hand reference with ZERO tokens is never scoreable. Before the fix
    // above this could happen silently (unfilled rows skipped), producing
    // wer=NaN / all-insertion output that looked like a real score line.
    assert!(
        ref_words > 0,
        "reference has 0 tokens — the filled template is empty or was not \
         parsed; refusing to score"
    );

    // EL transcript for the same clip (secondary disagreement).
    let clips = collect_clips(samples_dir);
    let clip = clips
        .iter()
        .find(|c| c.id == clip_id)
        .unwrap_or_else(|| panic!("clip {clip_id} not found in samples dir"));
    let el_path = samples_dir.join(format!("Elevenlabs Transcript {}.txt", clip.stem));
    let el_tokens: Vec<String> = std::fs::read_to_string(&el_path)
        .map(|t| tokens(&t))
        .unwrap_or_default();

    let mut out = std::fs::File::create(run_dir.join("scores.jsonl")).expect("scores.jsonl");
    for variant in Variant::all() {
        let art: VariantArt = serde_json::from_str(
            &std::fs::read_to_string(
                run_dir
                    .join(&clip_id)
                    .join(format!("{}.json", variant.id())),
            )
            .unwrap(),
        )
        .expect("variant artifacts");

        // Stage-B text vs hand reference (S/I/D on normalized tokens).
        let hyp_tokens: Vec<String> = art.b_spans.iter().flat_map(|s| tokens(&s.text)).collect();
        let (sub, ins, del) = sid(&ref_tokens, &hyp_tokens);
        let wer = if ref_words == 0 {
            f64::NAN
        } else {
            (sub + ins + del) as f64 / ref_words as f64
        };

        // Speaker errors: match each variant span to the reference span
        // covering >50% of it; compare speaker labels.
        let hyp_speaker_of = |sp: &Span, rule: &[Option<String>]| -> Option<usize> {
            let idx = art.b_spans.iter().position(|s| s.id == sp.id)?;
            rule.get(idx)?.as_ref().and_then(|l| {
                l.strip_prefix("Speaker ")
                    .and_then(|n| n.parse::<usize>().ok())
                    .map(|n| n - 1)
            })
        };
        let mut speaker_matches = 0usize;
        let mut speaker_errors = 0usize;
        for sp in &art.b_spans {
            let best: Option<(&RefSpan, f64)> = reference
                .iter()
                .filter_map(|r| {
                    let (rs, re_) = ref_window_of.get(r.id.as_str()).copied()?;
                    let ov = (sp.end.min(re_) - sp.start.max(rs)).max(0.0);
                    Some((r, ov))
                })
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((r, ov)) = best {
                let span_dur = sp.end - sp.start;
                if span_dur > 0.0 && ov / span_dur > 0.5 {
                    speaker_matches += 1;
                    // Compare under BOTH attribution rules.
                    for rule in [&art.speakers_segment_rule, &art.speakers_wordwin_rule] {
                        let hyp = hyp_speaker_of(sp, rule);
                        match (hyp, r.speaker) {
                            (Some(h), Some(r)) if h == r => {}
                            (None, None) => {}
                            _ => speaker_errors += 1,
                        }
                    }
                }
            }
        }

        // EL secondary disagreement (whole clip, stage-B text vs EL text).
        let (el_sub, el_ins, el_del) = sid(&el_tokens, &hyp_tokens);
        let el_dis = if el_tokens.is_empty() {
            f64::NAN
        } else {
            (el_sub + el_ins + el_del) as f64 / el_tokens.len().max(hyp_tokens.len()) as f64
        };

        let rule_flips = art
            .speakers_segment_rule
            .iter()
            .zip(art.speakers_wordwin_rule.iter())
            .filter(|(a, b)| a != b)
            .count();

        let line = ScoreLine {
            clip: clip_id.clone(),
            variant: variant.id().to_string(),
            sub,
            ins,
            del,
            ref_words,
            hyp_words: hyp_tokens.len(),
            wer,
            speaker_errors_matched: speaker_errors,
            speaker_matches,
            crossseg_runs_dropped: art.b_crossseg_runs_dropped,
            el_disagreement_secondary: el_dis,
            attribution_rule_flips: rule_flips,
        };
        let mut json = serde_json::to_string(&line).unwrap();
        json.push('\n');
        out.write_all(json.as_bytes()).unwrap();

        println!(
            "SCORE {} {} sub={} ins={} del={} wer={:.3} spk_err={}/{} (x2 rules) el_disagree_sec={:.3} rule_flips={}",
            line.clip,
            line.variant,
            line.sub,
            line.ins,
            line.del,
            line.wer,
            line.speaker_errors_matched,
            line.speaker_matches,
            line.el_disagreement_secondary,
            line.attribution_rule_flips
        );
    }
    println!("SCORE complete; content-free lines above; scores.jsonl written");
}

// ───────────────────────── report ─────────────────────────

fn report(artifacts: &Path) {
    let run_dir = artifacts.join("run");
    println!("== metrics (all clips, all variants; content-free) ==");
    if let Ok(raw) = std::fs::read_to_string(run_dir.join("metrics.jsonl")) {
        for line in raw.lines().filter(|l| !l.trim().is_empty()) {
            let m: serde_json::Value = serde_json::from_str(line).expect("metrics line");
            println!(
                "{:<8} {:<11} rt={:>5}ms a_seg={:>3} a_w={:>5} b_dw={:>4} b_ds={:>3} col={:>2} runs={:>2} unattr_s={:>3} unattr_w={:>3} flips={:>3}",
                m["clip"].as_str().unwrap_or("?"),
                m["variant"].as_str().unwrap_or("?"),
                m["decode_ms"].as_u64().unwrap_or(0),
                m["a_segments"].as_u64().unwrap_or(0),
                m["a_words"].as_u64().unwrap_or(0),
                m["b_dropped_words"].as_u64().unwrap_or(0),
                m["b_dropped_segments"].as_u64().unwrap_or(0),
                m["b_loop_collapses"].as_u64().unwrap_or(0),
                m["b_crossseg_runs_dropped"].as_u64().unwrap_or(0),
                m["c_unattributed_segment_rule"].as_u64().unwrap_or(0),
                m["c_unattributed_wordwin_rule"].as_u64().unwrap_or(0),
                m["attribution_rule_flips"].as_u64().unwrap_or(0),
            );
        }
    } else {
        println!("(no metrics.jsonl — run FS_EVAL_MODE=run)");
    }
    println!("\n== scores (hand-reference clip only; EL is SECONDARY DISAGREEMENT) ==");
    if let Ok(raw) = std::fs::read_to_string(run_dir.join("scores.jsonl")) {
        for line in raw.lines().filter(|l| !l.trim().is_empty()) {
            let s: serde_json::Value = serde_json::from_str(line).expect("score line");
            println!(
                "{:<8} {:<11} sub={:>3} ins={:>3} del={:>3} wer={:.3} spk_err={:>3} (of {} matches) runs={:>2} EL-disagree={:.3} flips={:>3}",
                s["clip"].as_str().unwrap_or("?"),
                s["variant"].as_str().unwrap_or("?"),
                s["sub"].as_u64().unwrap_or(0),
                s["ins"].as_u64().unwrap_or(0),
                s["del"].as_u64().unwrap_or(0),
                s["wer"].as_f64().unwrap_or(f64::NAN),
                s["speaker_errors_matched"].as_u64().unwrap_or(0),
                s["speaker_matches"].as_u64().unwrap_or(0),
                s["crossseg_runs_dropped"].as_u64().unwrap_or(0),
                s["el_disagreement_secondary"].as_f64().unwrap_or(f64::NAN),
                s["attribution_rule_flips"].as_u64().unwrap_or(0),
            );
        }
    } else {
        println!("(no scores.jsonl — fill the template, then FS_EVAL_MODE=score)");
    }
}

// ───────────────────────── tests (parse_template + sid kernel) ─────────────────────────
// Regression tests for the 2026-09-13 incident: an unfilled template
// (byte-identical to template.txt, all correction/speaker columns empty)
// scored silently as an EMPTY reference (wer=NaN, all-insertion output).
// Root cause: trim_end() ate the trailing TAB of unfilled rows (6 TSV
// fields -> 5), making the "not fully filled" panic unreachable.

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(name: &str, contents: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("fs_local_eval_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, contents).unwrap();
        p
    }

    #[test]
    fn unfilled_template_must_panic_not_score_silently() {
        // Exact shape of an unfilled row as emitted by template mode:
        // trailing TAB present; both fill columns empty.
        let raw = "# header\n\nspan-A\t[Speaker 1]\t||\t\t||\t\n    hypothesis text\n";
        let p = write_tmp("unfilled.txt", raw);
        let result = std::panic::catch_unwind(move || parse_template(&p));
        assert!(
            result.is_err(),
            "unfilled template must panic, not return spans"
        );
    }

    #[test]
    fn unfilled_row_without_trailing_tab_also_panics() {
        // Editor-saved variant: trailing whitespace stripped by the editor.
        let raw = "span-A\t[Speaker 1]\t||\t\t||\n    hypothesis text\n";
        let p = write_tmp("unfilled_notab.txt", raw);
        let result = std::panic::catch_unwind(move || parse_template(&p));
        assert!(result.is_err(), "unfilled row (no trailing tab) must panic");
    }

    #[test]
    fn filled_row_parses_correction_and_speaker() {
        let raw = "span-A\t[Speaker 1]\t||\tCORRECTED WORDS HERE\t||\tS2\n    hypothesis text\n";
        let p = write_tmp("filled.txt", raw);
        let spans = parse_template(&p);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].id, "span-A");
        assert_eq!(spans[0].speaker, Some(1)); // S2 -> 0-based 1
        // Tokenization happens later; text preserved verbatim.
        assert_eq!(spans[0].text, "CORRECTED WORDS HERE");
    }

    #[test]
    fn deleted_span_empty_text_with_dash_speaker() {
        let raw = "span-A\t[Speaker 1]\t||\t\t||\t-\n    hypothesis text\n";
        let p = write_tmp("deleted.txt", raw);
        let spans = parse_template(&p);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].speaker, None);
        assert!(spans[0].text.is_empty(), "deleted span has empty text");
    }

    #[test]
    fn sid_counts_sub_ins_del_separately() {
        let r: Vec<String> = ["the", "cat", "sat", "here"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let h: Vec<String> = ["the", "bat", "sat"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (s, i, d) = sid(&r, &h);
        assert_eq!((s, i, d), (1, 0, 1), "cat->bat sub; 'here' deleted");
    }

    #[test]
    fn sid_identical_texts_zero_error() {
        let r: Vec<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let (s, i, d) = sid(&r, &r.clone());
        assert_eq!((s, i, d), (0, 0, 0));
    }
}
