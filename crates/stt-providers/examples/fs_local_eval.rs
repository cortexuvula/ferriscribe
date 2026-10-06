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
        .as_chunks::<2>()
        .0
        .iter()
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
        let fs_words = if fs_rtf.exists() {
            word_count_rtf(&fs_rtf)
        } else {
            0
        };
        let el_words = if el_txt.exists() {
            plain_word_count(&el_txt)
        } else {
            0
        };
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
            if w_end - w_start >= 0.01
                && let Ok(w) = tok.to_str()
            {
                let t = w.trim();
                if !t.is_empty() {
                    words.push((w_start, w_end, t.to_string()));
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

    let mut transcript = mk_transcript(spans_raw.iter().map(seg_of).collect());

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
/// > otherwise the whole-segment window rule (10 ms floor, dominance 0.7).
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

// ───────────────────────── template (hand-review reference, format v2) ─────────────────────────

/// Format tag every emitted reference carries. parse_reference REFUSES files
/// without it (a v1 two-line-per-span file must not silently mis-parse).
const REVIEW_FORMAT: &str = "review-v2";
/// Max review-row duration: long speech intervals are split on this grid so
/// no row asks a human to transcribe more than a few seconds by ear.
const MAX_ROW_S: f64 = 8.0;
/// Diarization-turn gaps up to this length are treated as the same speech
/// region (pyannote often breaks voicemail-style continuous speech).
const GAP_MERGE_S: f64 = 0.5;

/// One whisper-independent review row. Rows are derived from the diarization
/// turns (pyannote over the AUDIO — never from a decode), so a dropped
/// whisper utterance is still representable: its audio interval has a row.
#[derive(Debug, Clone, PartialEq)]
struct ReviewRow {
    id: String,
    start: f64,
    end: f64,
    /// Suggested speaker (diarization majority overlap), 0-based.
    speaker: Option<usize>,
    /// True when the interval is speech per diarization (false = gap/silence
    /// candidate emitted to guarantee full [0, dur] coverage).
    speech: bool,
}

/// Build review rows from diarization turns (whisper-INDEPENDENT source).
/// Turn sequence is grouped into speech regions: a new region starts on a
/// gap > GAP_MERGE_S OR a speaker change (both from diarization over the
/// audio, never from a decode). Regions longer than MAX_ROW_S are split on
/// the time grid; gaps become silence-candidate rows. Rows partition [0,dur].
fn review_rows_from_turns(turns: &[(usize, f64, f64)], dur_s: f64) -> Vec<ReviewRow> {
    let mut ivs: Vec<(usize, f64, f64)> = turns
        .iter()
        .map(|&(spk, s, e)| (spk, s.min(e), s.max(e)))
        .filter(|&(_, s, e)| e > s)
        .collect();
    ivs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    // Group into speech regions: split on gap > GAP_MERGE_S or speaker change.
    let mut regions: Vec<(usize, f64, f64)> = Vec::new();
    for &(spk, s, e) in &ivs {
        match regions.last_mut() {
            Some(last) if last.0 == spk && s - last.2 <= GAP_MERGE_S => last.2 = last.2.max(e),
            _ => regions.push((spk, s, e)),
        }
    }

    let mut rows = Vec::new();
    let mut cursor = 0.0f64;
    let push =
        |start: f64, end: f64, speech: bool, spk: Option<usize>, rows: &mut Vec<ReviewRow>| {
            let mut s = start;
            while end - s > 1e-9 {
                let row_end = (s + MAX_ROW_S).min(end);
                rows.push(ReviewRow {
                    id: format!("row-{:02}", rows.len() + 1),
                    start: s,
                    end: row_end,
                    speaker: if speech { spk } else { None },
                    speech,
                });
                s = row_end;
            }
        };
    for &(spk, s, e) in &regions {
        if s - cursor > 1e-9 {
            push(cursor, s, false, None, &mut rows); // silence candidate
        }
        push(s, e, true, Some(spk), &mut rows);
        cursor = e;
    }
    if dur_s - cursor > 1e-9 {
        push(cursor, dur_s, false, None, &mut rows); // trailing silence
    }
    rows
}

/// One EL cue parsed from the ElevenLabs SRT-ish transcript: (start, end, text).
fn parse_el_cues(path: &Path) -> Vec<(f64, f64, String)> {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let ts = |s: &str| -> Option<f64> {
        let p: Vec<&str> = s.trim().split(':').collect();
        match p.as_slice() {
            [h, m, rest] => {
                let (sec, ms) = rest.split_once(',')?;
                Some(
                    h.parse::<f64>().ok()? * 3600.0
                        + m.parse::<f64>().ok()? * 60.0
                        + sec.parse::<f64>().ok()?
                        + ms.parse::<f64>().ok()? / 1000.0,
                )
            }
            _ => None,
        }
    };
    let mut cues = Vec::new();
    let mut lines = raw.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some((l, r)) = line.split_once("-->") {
            // Each half may carry a trailing label ("00:00:04,460 [Speaker 0]").
            fn clean(s: &str) -> &str {
                s.split_whitespace().next().unwrap_or("")
            }
            if let (Some(s), Some(e)) = (ts(clean(l)), ts(clean(r))) {
                let mut text = String::new();
                while let Some(t) = lines.peek() {
                    if t.trim().is_empty() || t.contains("-->") {
                        break;
                    }
                    text.push_str(lines.next().unwrap_or("").trim());
                    text.push(' ');
                }
                cues.push((s, e, text.trim().to_string()));
            }
        }
    }
    cues
}

/// Emit the v2 reference file text (ONE line per row, 6 tab-separated
/// fields, terminal field is the never-empty review status — a plain text
/// editor stripping trailing whitespace cannot change the field count).
fn emit_reference(rows: &[ReviewRow], dur_s: f64, el_cues: &[(f64, f64, String)]) -> String {
    let speech_rows = rows.iter().filter(|r| r.speech).count();
    let silence_rows = rows.len() - speech_rows;
    let mut out = String::new();
    out.push_str("# Hand-review reference (PHI — local only, never commit)\n");
    out.push_str(&format!("# format: {REVIEW_FORMAT}\n"));
    out.push_str(&format!(
        "# Clip rows are whisper-INDEPENDENT (from diarization over the audio):\n\
         # this clip has {speech_rows} speech rows + {silence_rows} silence rows.\n\
         # ElevenLabs independently suggests {} cues (comments below, by-ear aid only).\n",
        el_cues.len()
    ));
    out.push_str(&format!(
        "# Duration: {dur_s:.1}s. ONE LINE PER ROW, 6 tab-separated fields:\n"
    ));
    out.push_str("#   <row-id> TAB <start-s> TAB <end-s> TAB <speaker> TAB <text> TAB <status>\n");
    out.push_str("# Fill by ear, then change ONLY the last ??? of each row to OK:\n");
    out.push_str("#   OK + EMPTY text = you confirm this interval is SILENCE.\n");
    out.push_str("#   OK + text       = the true words spoken in this interval.\n");
    out.push_str("#   UNINTEL + EMPTY text = you LISTENED but could not resolve it by ear.\n");
    out.push_str("#     (Reviewed — does not block scoring; leave ??? only if never reached.)\n");
    out.push_str("#   ??? (or a deleted row) = not reviewed — scoring refuses until every\n");
    out.push_str("#     row is OK or UNINTEL. A row you tried and could not resolve is\n");
    out.push_str("#     UNINTEL, never ??? — the two are counted differently.\n");
    out.push_str(
        "# Speaker: S1/S2 = who spoke it ('-' = leave unassigned; suggestion prefilled).\n",
    );
    out.push_str("# Do NOT edit columns 1-3. Do NOT delete rows. A missed utterance = add a\n");
    out.push_str("# new row (row-90+) inside the interval it belongs to, with its text.\n");
    out.push_str("# Save as <clip>.filled.txt when every row ends in OK.\n\n");
    for r in rows {
        let spk = match r.speaker {
            Some(0) => "S1",
            Some(1) => "S2",
            Some(n) => {
                // max_speakers=2 in production; keep the label honest anyway.
                let _ = n;
                "-"
            }
            None => "-",
        };
        out.push_str(&format!(
            "{}\t{:.2}\t{:.2}\t{}\t\t???\n",
            r.id, r.start, r.end, spk
        ));
        for &(cs, ce, ref ct) in el_cues {
            let ov = (r.end.min(ce) - r.start.max(cs)).max(0.0);
            if ov > 0.0 && !ct.is_empty() {
                out.push_str(&format!("#   ElevenLabs cue {cs:.2}-{ce:.2}: {ct}\n"));
            }
        }
    }
    out
}

/// Generate the hand-review reference for the SHORTEST clip. Rows come from
/// the diarization turns (whisper-INDEPENDENT — a reference keyed to a
/// decode inherits that decode's blind spots, so its span count can never
/// establish coverage). EL cues are included as SUGGESTIONS only; every row
/// still requires human confirmation. Output (PHI, local-only):
///   local-eval/harness-artifacts/reference/<clip>.review.txt
fn make_template(samples_dir: &Path, artifacts: &Path) {
    let clips = collect_clips(samples_dir);
    // FS_EVAL_CLIP=<clip-id> overrides the shortest-clip default, so the
    // next reference clip can be selected by overlap/balance criteria
    // (Codie 2026-09-15) instead of by duration alone. The override must
    // name a real clip; anything else fails loudly.
    let mut best: Option<(&Clip, f64)> = None;
    if let Some(want) = std::env::var_os("FS_EVAL_CLIP") {
        let want = want.to_string_lossy().to_string();
        best = clips
            .iter()
            .map(|c| (c, load_clip_16k(c).len() as f64 / 16_000.0))
            .find(|(c, _)| c.id == want);
        assert!(best.is_some(), "FS_EVAL_CLIP={want}: no such clip");
    }
    if best.is_none() {
        for c in &clips {
            let dur_s = load_clip_16k(c).len() as f64 / 16_000.0;
            if best.as_ref().is_none_or(|(_, d)| dur_s < *d) {
                best = Some((c, dur_s));
            }
        }
    }
    let (shortest, dur_s) = best.expect("at least one clip");

    // Duration cross-check + clip identity from the stored V0 artifacts (no
    // decode happens here; the artifacts only supply duration metadata).
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
    assert!(
        (art.dur_s - dur_s).abs() < 0.5,
        "duration mismatch artifacts vs audio"
    );

    // Whisper-independent row source: the stored diarization turns.
    let diar_path = artifacts
        .join("run")
        .join(&shortest.id)
        .join("diarization.json");
    let diar: DiarizationArt =
        serde_json::from_str(&std::fs::read_to_string(&diar_path).unwrap()).unwrap();
    let rows = review_rows_from_turns(&diar.turns, dur_s);

    // EL segmentation as a SUGGESTION (never the row source, never gating).
    let el_path = samples_dir.join(format!("Elevenlabs Transcript {}.txt", shortest.stem));
    let el_cues = parse_el_cues(&el_path);

    let ref_dir = artifacts.join("reference");
    std::fs::create_dir_all(&ref_dir).unwrap();
    let out_path = ref_dir.join(format!("{}.review.txt", shortest.id));
    std::fs::write(&out_path, emit_reference(&rows, dur_s, &el_cues)).unwrap();

    let speech_rows = rows.iter().filter(|r| r.speech).count();
    println!(
        "TEMPLATE {} dur_s={:.1} speech_rows={} silence_rows={} total_rows={} el_cues={} path={}",
        shortest.id,
        dur_s,
        speech_rows,
        rows.len() - speech_rows,
        rows.len(),
        el_cues.len(),
        out_path.display()
    );
}

// ───────────────────────── score ─────────────────────────

/// One row of the review reference after human review.
#[derive(Debug, Clone, PartialEq)]
enum RowStatus {
    /// `???` (or the row is missing entirely) — not reviewed. Scoring
    /// REFUSES while any row is in this state.
    NotReviewed,
    /// `UNINTEL` — the reviewer reached this row and could not resolve it
    /// by ear. Reviewed (does not block scoring) but contributes no
    /// reference tokens; distinct from ??? so "tried and failed" can never
    /// be confused with "never reached".
    Unintelligible,
    /// `OK` with empty text: the human confirms the interval is SILENCE.
    /// Nonempty ASR output mapped here is an INSERTION.
    ConfirmedSilence,
    /// `OK` with text: the true words spoken in this interval.
    Speech(String),
}

#[derive(Debug, Clone, PartialEq)]
struct RefRow {
    id: String,
    start: f64,
    end: f64,
    /// S1->Some(0), S2->Some(1), '-'->None.
    speaker: Option<usize>,
    status: RowStatus,
}

/// A fully hand-reviewed reference (every row OK). `not_reviewed` counts
/// ??? rows PLUS rows present in the emitted review file but deleted from
/// the filled copy — both block scoring.
#[allow(dead_code)] // clip/dur_s carried for score-mode reporting parity
struct ReviewedRef {
    clip: String,
    /// DECLARED audio interval end: the '# Duration:' header of the emitted
    /// review file (the template partitioned [0, dur] when generated). The
    /// completion gate accounts for this ENTIRE interval — not just the
    /// union of the proposed rows (repo-auditor, 2026-09-13).
    dur_s: f64,
    /// Rows sorted by start time (human-added rows interleave correctly).
    rows: Vec<RefRow>,
    /// Whole-clip reference tokens (Speech rows in time order).
    ref_tokens: Vec<String>,
    not_reviewed: usize,
    first_not_reviewed: Option<String>,
    /// Seconds of the declared interval [0, dur] covered by NO row. Row-ID
    /// audits alone cannot see this (a columns-2-3 edit moves a boundary
    /// without touching any ID); uncovered audio is unreviewed audio.
    uncovered_s: f64,
    first_uncovered: Option<(f64, f64)>,
}

/// Human-added rows use numeric IDs >= this (e.g. row-90, row-91) so they
/// can never collide with template rows (row-01..row-NN, N < 90).
const HUMAN_ROW_MIN: usize = 90;

/// Content-free row identifier for panic messages (PHI never reaches the
/// console: IDs and timings only, never text).
fn row_placeholder(parts: &[&str]) -> String {
    parts.first().unwrap_or(&"<no-id>").trim().to_string()
}

/// Parse ONE reference-format file (template or filled) into rows.
/// Format (review-v2), one line per row, 6 tab-separated fields:
///   <row-id> TAB <start-s> TAB <end-s> TAB <speaker> TAB <text> TAB <status>
/// The terminal field is never empty (??? / OK), so an editor stripping
/// trailing whitespace cannot change the field count.
fn parse_reference(path: &Path) -> Vec<RefRow> {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "cannot read reference file {:?}: {e} — (re)generate with FS_EVAL_MODE=template",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
        )
    });
    if !raw
        .lines()
        .any(|l| l.trim() == format!("# format: {REVIEW_FORMAT}"))
    {
        panic!(
            "reference file {:?} is not {REVIEW_FORMAT} (missing '# format:' header) — \
             regenerate with FS_EVAL_MODE=template; legacy two-line-per-span \
             files are not parseable and must not be re-filled",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
        );
    }
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() != 6 {
            panic!(
                "malformed reference row {:?}: {} tab fields, expected exactly 6 \
                 (id/start/end/speaker/text/status) — keep the row-ID and do not \
                 paste TABs into the text; use spaces",
                row_placeholder(&parts),
                parts.len()
            );
        }
        let id = parts[0].trim().to_string();
        let start: f64 = parts[1]
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("row {id}: bad start time {:?}: {e}", parts[1].trim()));
        let end: f64 = parts[2]
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("row {id}: bad end time {:?}: {e}", parts[2].trim()));
        assert!(
            end > start,
            "row {id}: end ({end}) must be > start ({start})"
        );
        let speaker = match parts[3].trim() {
            "S1" => Some(0),
            "S2" => Some(1),
            "-" => None,
            other => panic!("row {id}: speaker must be S1, S2, or '-', got {other:?}"),
        };
        let text = parts[4].trim().to_string();
        let has_text = !text.is_empty();
        // Status-token tolerance is EXPLICIT: plain ASCII spaces around the
        // token (a plain-text-editor save) are accepted; anything else is
        // junk and refused — a blanket .trim() here silently accepted
        // invisible Unicode whitespace (e.g. NBSP), undocumented.
        let status_field = parts[5];
        let status_tok = status_field.trim_matches(' ');
        let status = match status_tok {
            "???" => RowStatus::NotReviewed,
            "UNINTEL" => RowStatus::Unintelligible,
            "OK" => {
                if text.is_empty() {
                    RowStatus::ConfirmedSilence
                } else {
                    RowStatus::Speech(text)
                }
            }
            other => panic!(
                "row {id}: status must be ???, OK, or UNINTEL, got {other:?} — trailing \
                 junk after the status token is refused; the last field must be \
                 exactly one token"
            ),
        };
        if matches!(status, RowStatus::Unintelligible) && has_text {
            panic!(
                "row {id}: UNINTEL rows must have an EMPTY text field — text next to \
                 UNINTEL would be silently dropped; either transcribe it (OK) or \
                 clear the text (UNINTEL)"
            );
        }
        out.push(RefRow {
            id,
            start,
            end,
            speaker,
            status,
        });
    }
    out
}

/// DECLARED duration from the review file's '# Duration: 30.1s.' header.
/// This is the interval the template was generated to partition; the
/// completion gate holds the FILLED copy against it, so audio that falls
/// outside every row (possible only if columns 2-3 were edited) cannot
/// silently leave the review.
fn declared_dur_s(path: &Path) -> Option<f64> {
    let raw = std::fs::read_to_string(path).ok()?;
    for line in raw.lines() {
        if let Some(rest) = line.trim().strip_prefix("# Duration:") {
            let tok = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                // "30.1s." — strip trailing unit + sentence punctuation in
                // any order/combination.
                .trim_end_matches(['s', '.']);
            if !tok.is_empty()
                && let Ok(v) = tok.parse::<f64>()
            {
                return Some(v);
            }
        }
    }
    None
}

/// Load the filled reference and complete the not-reviewed audit against the
/// emitted review file (same directory, <clip>.review.txt): a row deleted
/// from the filled copy counts as not reviewed, exactly like a ??? row.
fn load_reviewed(ref_dir: &Path, clip_id: &str) -> ReviewedRef {
    let filled_path = ref_dir.join(format!("{clip_id}.filled.txt"));
    let template_path = ref_dir.join(format!("{clip_id}.review.txt"));
    let filled = parse_reference(&filled_path);

    // Duplicate row IDs would silently double-count that row's words in
    // ref_tokens (shifting the WER denominator) — refuse instead.
    let mut seen_ids = std::collections::BTreeSet::new();
    for r in &filled {
        assert!(
            seen_ids.insert(r.id.clone()),
            "duplicate row id {:?} in {} — a duplicated row silently double-counts \
             its words and shifts the WER denominator; each row ID must appear \
             exactly once",
            r.id,
            filled_path.display()
        );
    }

    // Expected row IDs from the emitted review file; every filled ID must be
    // either one of these or a human-added row (numeric ID >= HUMAN_ROW_MIN).
    let expected: std::collections::BTreeSet<String> = if template_path.exists() {
        parse_reference(&template_path)
            .into_iter()
            .map(|r| r.id)
            .collect()
    } else {
        panic!(
            "{clip_id}.review.txt missing under {} — regenerate with \
             FS_EVAL_MODE=template (deterministic; row-set audit needs it)",
            ref_dir.display()
        );
    };
    let human = |id: &str| {
        id.strip_prefix("row-")
            .and_then(|n| n.parse::<usize>().ok())
            .is_some_and(|n| n >= HUMAN_ROW_MIN)
    };
    for r in &filled {
        assert!(
            expected.contains(&r.id) || human(&r.id),
            "unknown row id {:?} (not in the template and not a human row-{}+) — \
             keep template IDs verbatim; add missed utterances as row-90+",
            r.id,
            HUMAN_ROW_MIN
        );
    }

    let mut not_reviewed = filled
        .iter()
        .filter(|r| r.status == RowStatus::NotReviewed)
        .count();
    let mut first_not_reviewed = filled
        .iter()
        .find(|r| r.status == RowStatus::NotReviewed)
        .map(|r| r.id.clone());
    let present: std::collections::BTreeSet<&String> = filled.iter().map(|r| &r.id).collect();
    let missing: Vec<&String> = expected
        .iter()
        .filter(|id| !present.contains(*id))
        .collect();
    if !missing.is_empty() {
        not_reviewed += missing.len();
        first_not_reviewed = first_not_reviewed.or_else(|| missing.first().map(|s| (*s).clone()));
    }

    // WHOLE-DECLARED-INTERVAL gate (repo-auditor 2026-09-13): a pyannote-
    // derived row set is NOT omission-proof — speech missed by both systems
    // has no proposed row, and an edited columns-2-3 row moves a boundary
    // without touching any row ID. So completion is measured over the
    // ENTIRE declared interval [0, dur]: seconds covered by no row are
    // unreviewed seconds, reported exactly like a ??? row.
    let declared_dur = declared_dur_s(&template_path)
        .unwrap_or_else(|| panic!("{clip_id}.review.txt: no '# Duration:' header"));
    let mut uncovered_s = 0.0f64;
    let mut first_uncovered: Option<(f64, f64)> = None;
    {
        // ORDER-INDEPENDENT (Codie re-gate of 77aa64b): coverage is a
        // property of the row SET, never of the file order. A human row
        // appended at the END of the file (the natural add-a-row edit)
        // walked the cursor in FILE order and produced FALSE uncovered
        // seconds — refusing a perfectly reviewed file. Walk TIME-sorted
        // rows instead, and while walking, hold the real invariant the
        // file order used to hide: rows must TILE — overlap would
        // double-count words and scorable seconds, so it refuses.
        let mut sorted: Vec<&RefRow> = filled.iter().collect();
        sorted.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
        let mut cursor = 0.0f64;
        for r in sorted {
            assert!(
                r.start >= cursor - 1e-9,
                "row {:?} [{:.2}, {:.2}) starts before the previous row ends \
                 ({:.2}) — rows must tile the declared interval with NO \
                 overlap (overlap would double-count words and seconds in the \
                 score); adjust the boundary of one of the two rows",
                r.id,
                r.start,
                r.end,
                cursor
            );
            if r.start > cursor + 1e-9 {
                uncovered_s += r.start - cursor;
                first_uncovered = first_uncovered.or(Some((cursor, r.start)));
            }
            cursor = cursor.max(r.end);
        }
        if declared_dur > cursor + 1e-9 {
            uncovered_s += declared_dur - cursor;
            first_uncovered = first_uncovered.or(Some((cursor, declared_dur)));
        }
    }

    let dur_s = filled.iter().map(|r| r.end).fold(0.0f64, f64::max);
    let mut rows = filled;
    rows.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());

    let ref_tokens: Vec<String> = rows
        .iter()
        .filter_map(|r| match &r.status {
            RowStatus::Speech(t) => Some(tokens(t)),
            _ => None,
        })
        .flatten()
        .collect();

    ReviewedRef {
        clip: clip_id.to_string(),
        dur_s,
        rows,
        ref_tokens,
        not_reviewed,
        first_not_reviewed,
        uncovered_s,
        first_uncovered,
    }
}

/// Overlap of [cs,ce) with [rs,re) in seconds (0 when disjoint).
fn overlap_s(cs: f64, ce: f64, rs: f64, re: f64) -> f64 {
    (ce.min(re) - cs.max(rs)).max(0.0)
}

/// Assign a cue to the reference row it overlaps MOST (plurality, not the
/// old >50%-of-the-CUE rule). Exact ties go to the earliest row in `rows`;
/// callers pass time-sorted rows so this is deterministic. Rows are a
/// partition of [0,dur], so every in-range cue lands somewhere.
fn assign_cue_to_row<'a>(cs: f64, ce: f64, rows: &'a [RefRow]) -> Option<&'a RefRow> {
    let mut best: Option<(&'a RefRow, f64)> = None;
    for r in rows {
        let ov = overlap_s(cs, ce, r.start, r.end);
        if ov <= 0.0 {
            continue;
        }
        // strict > : an exact tie keeps the EARLIER row (rows are
        // time-sorted, so this is deterministic).
        let better = best.map(|(_, b)| ov > b).unwrap_or(true);
        if better {
            best = Some((r, ov));
        }
    }
    best.map(|(r, _)| r)
}

/// INSERTION class: a nonempty cue whose overlap with CONFIRMED-SILENCE
/// rows STRICTLY exceeds its overlap with Speech rows. An exact tie
/// resolves toward speech (not an insertion) — hallucination is the
/// graver claim, so it needs the strict majority. UNINTEL/NotReviewed
/// overlap counts toward neither side.
fn cue_is_insertion_class(cs: f64, ce: f64, rows: &[RefRow]) -> bool {
    let mut sil = 0.0f64;
    let mut sp = 0.0f64;
    for r in rows {
        let ov = overlap_s(cs, ce, r.start, r.end);
        if ov <= 0.0 {
            continue;
        }
        match r.status {
            RowStatus::ConfirmedSilence => sil += ov,
            RowStatus::Speech(_) => sp += ov,
            RowStatus::Unintelligible | RowStatus::NotReviewed => {}
        }
    }
    sil > sp
}

/// Fraction of the ROW's duration covered by any cue (row-denominator —
/// a long cue fully covering a short row is 1.0 coverage, which the old
/// cue-denominator rule wrongly called a partial overlap).
fn row_covered_fraction(row: &RefRow, cues: &[(f64, f64)]) -> f64 {
    let dur = row.end - row.start;
    if dur <= 0.0 {
        return 0.0;
    }
    let mut cover = 0.0f64;
    for &(cs, ce) in cues {
        cover += overlap_s(cs, ce, row.start, row.end);
    }
    (cover / dur).min(1.0)
}

/// DELETION class: a Speech row covered by cue time strictly less than
/// half its own duration (>= 50% counts as covered; the exact boundary
/// is pinned by test).
fn row_uncovered_is_deletion(row: &RefRow, cues: &[(f64, f64)]) -> bool {
    row_covered_fraction(row, cues) < 0.5
}

/// Speaker errors under both attribution rules, using the same plurality
/// cue→row assignment as the insertion/deletion classes. Cues whose
/// plurality row is not a confirmed Speech row (silence = insertion class,
/// UNINTEL = speaker unconfirmed) are excluded from BOTH numerator and
/// denominator — the old rule counted hallucinations over silence as
/// speaker errors.
fn count_speaker_errors(
    spans: &[Span],
    seg_rule: &[Option<String>],
    wordwin_rule: &[Option<String>],
    rows: &[RefRow],
) -> (usize, usize) {
    let hyp_speaker_of = |sp: &Span, rule: &[Option<String>]| -> Option<usize> {
        let idx = spans.iter().position(|s| s.id == sp.id)?;
        rule.get(idx)?.as_ref().and_then(|l| {
            l.strip_prefix("Speaker ")
                .and_then(|n| n.parse::<usize>().ok())
                .map(|n| n - 1)
        })
    };
    let mut matches = 0usize;
    let mut errors = 0usize;
    for sp in spans {
        let span_dur = sp.end - sp.start;
        if span_dur <= 0.0 {
            continue;
        }
        if let Some(r) = assign_cue_to_row(sp.start, sp.end, rows)
            && matches!(r.status, RowStatus::Speech(_))
        {
            matches += 1;
            for rule in [seg_rule, wordwin_rule] {
                let hyp = hyp_speaker_of(sp, rule);
                match (hyp, r.speaker) {
                    (Some(h), Some(rr)) if h == rr => {}
                    (None, None) => {}
                    _ => errors += 1,
                }
            }
        }
    }
    (matches, errors)
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
    for (j, cell) in dp[0].iter_mut().enumerate().take(n + 1).skip(1) {
        *cell = (0, j, 0); // all insertions
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
    /// Hypothesis cue words inside CONFIRMED-SILENCE intervals (whole-clip
    /// insertions above already include them; this localizes them).
    ins_on_confirmed_silence: usize,
    /// Reference words (human rows with text) with NO hypothesis cue mapped:
    /// the dropped-utterance class a decode-keyed reference hid.
    del_in_speech_rows: usize,
    /// Ref words the WER denominator actually covers (Speech rows only).
    /// UNINTEL seconds are excluded from scoring BY DESIGN and are reported
    /// on the separate REVIEW COMPLETION line — never here, so the two can
    /// never be confused.
    ref_scorable_words: usize,
    /// Seconds excluded from scoring because the reviewer marked them
    /// UNINTEL. FIXED ACROSS VARIANTS (one reference, computed once): the
    /// same exclusion applies to every variant, so no variant can earn a
    /// better word error by being scored over a smaller interval.
    unintel_excluded_s: f64,
    /// REVIEW COMPLETION fields (review-level, identical on every variant
    /// line; carried so `report` can print the REVIEW row from
    /// scores.jsonl alone): total reviewed rows, UNINTEL rows, UNINTEL
    /// seconds, and scorable (non-UNINTEL) seconds.
    reviewed_rows: usize,
    unintel_rows: usize,
    unintel_s: f64,
    scorable_s: f64,
    /// SEPARATE metric — segmentation only, never folded into WER: hyp cues
    /// per reference speech row vs the row set's cue count, as boundary
    /// error. Gaming cue count cannot buy a word-error win.
    seg_boundary_error: f64,
    /// Hypothesis cue count (segmentation-invariance: identical words in
    /// identical order score identically regardless of this number).
    hyp_cues: usize,
    /// Reference cue count (human rows with text; includes row-90+ adds).
    ref_cues: usize,
    /// Speaker errors: hyp cues matched to reference rows by time overlap
    /// (>50% of the cue inside the row) whose speaker label differs.
    speaker_errors_matched: usize,
    speaker_matches: usize,
    /// Repetition regressions: cross-segment runs dropped (from run stage).
    crossseg_runs_dropped: usize,
    /// SECONDARY — labelled disagreement, NOT accuracy: ElevenLabs whole-
    /// clip word disagreement between the variant's stage-B text and the EL
    /// transcript. EL is a proxy with its own artifacts.
    el_disagreement_secondary: f64,
    /// Attribution-rule A/B on this clip/variant (from run stage).
    attribution_rule_flips: usize,
}

fn score(artifacts: &Path, samples_dir: &Path) {
    let run_dir = artifacts.join("run");
    let ref_dir = artifacts.join("reference");

    // Locate the filled reference (any *.filled.txt, review-v2 format).
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
    let reference = load_reviewed(&ref_dir, &clip_id);

    // LOUD FAILURE (kept from the 2026-09-13 incident): a reference with
    // unreviewed rows or zero tokens is never scoreable. Refusing is the
    // correct behavior; this must not regress.
    if reference.not_reviewed > 0 {
        panic!(
            "reference NOT fully reviewed: {} row(s) ??? or deleted (first: {:?}) — \
             every row must end in OK (empty text + OK = confirmed silence)",
            reference.not_reviewed, reference.first_not_reviewed
        );
    }
    if reference.uncovered_s > 1e-6 {
        // Uncovered audio is unreviewed audio, reported exactly like a ???
        // row. Content-free: interval bounds only, never text.
        panic!(
            "reference does not cover the DECLARED interval [0, {:.2}s]: \
             {:.2}s covered by no row (first gap: [{:.2}, {:.2})) — add a \
             row-{}+ spanning each gap (OK + empty text if it is silence)",
            reference.dur_s,
            reference.uncovered_s,
            reference.first_uncovered.map(|g| g.0).unwrap_or(0.0),
            reference.first_uncovered.map(|g| g.1).unwrap_or(0.0),
            HUMAN_ROW_MIN
        );
    }
    let ref_tokens = &reference.ref_tokens;
    let ref_words = ref_tokens.len();
    let ref_cues = reference
        .rows
        .iter()
        .filter(|r| matches!(r.status, RowStatus::Speech(_)))
        .count();
    assert!(
        ref_words > 0,
        "reference has 0 tokens — the filled file is empty or was not parsed; \
         refusing to score"
    );

    // ── REVIEW COMPLETION — reported separately from SCORABLE COVERAGE ──
    // (ui-consultant, 2026-09-13) A file can be fully reviewed without
    // supporting an accuracy score over all of its audio: UNINTEL rows are
    // excluded from the WER denominator BY DESIGN. Reporting only WER would
    // hide that exclusion; a variant cannot earn a better word error by
    // being scored over a smaller interval because the excluded duration is
    // FIXED BY THE REVIEW (same reference for every variant), computed once
    // here, and shown on its own row before any SCORE line.
    let reviewed_rows = reference.rows.len();
    let unintel_rows = reference
        .rows
        .iter()
        .filter(|r| matches!(r.status, RowStatus::Unintelligible))
        .count();
    let unintel_s: f64 = reference
        .rows
        .iter()
        .filter(|r| matches!(r.status, RowStatus::Unintelligible))
        .map(|r| r.end - r.start)
        .sum();
    let declared_s = reference.dur_s;
    let scorable_s: f64 = reference
        .rows
        .iter()
        .filter(|r| !matches!(r.status, RowStatus::Unintelligible))
        .map(|r| r.end - r.start)
        .sum();
    println!(
        "REVIEW {clip_id} COMPLETE rows={reviewed_rows} not_reviewed=0 uncovered_s={:.2} \
         unintel_rows={unintel_rows} unintel_s={unintel_s:.2} scorable_s={scorable_s:.2} \
         ({:.1}% of declared_s={declared_s:.2})",
        reference.uncovered_s,
        if declared_s > 0.0 {
            100.0 * scorable_s / declared_s
        } else {
            f64::NAN
        }
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
        // SEGMENTATION-INVARIANT: tokens are concatenated across cues, so
        // identical words in identical order score identically whether the
        // decoder emitted one cue or ten. Cue-count deltas are reported on
        // the SEPARATE seg_boundary_error row and can never touch WER.
        let hyp_tokens: Vec<String> = art.b_spans.iter().flat_map(|s| tokens(&s.text)).collect();
        let (sub, ins, del) = sid(ref_tokens, &hyp_tokens);
        let wer = if ref_words == 0 {
            f64::NAN
        } else {
            (sub + ins + del) as f64 / ref_words as f64
        };

        // Per-row localized counts (diagnostics with explicit semantics):
        // INSERTION class — nonempty hypothesis whose plurality row is a
        // confirmed-silence interval; DELETION class — reference speech
        // row covered by cue time < 50% of its own duration. Both use the
        // same plurality cue→row assignment (assign_cue_to_row); the old
        // >50%-of-the-CUE rule mis-binned cues straddling a speech/silence
        // boundary and cues longer than the row they fully contained.
        let cue_ivs: Vec<(f64, f64)> = art
            .b_spans
            .iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| (s.start, s.end))
            .collect();
        // Deliberate asymmetry (Codie review faf018b, informational):
        // cue_ivs (deletion coverage) keeps only NONEMPTY-text cues — a
        // wordless span decodes no words, so it cannot cover speech. The
        // speaker pass below uses ALL spans: attribution is about cue/speaker
        // alignment, and an empty-text span still carries a speaker label.
        let ins_on_confirmed_silence: usize = cue_ivs
            .iter()
            .filter(|&&(cs, ce)| cue_is_insertion_class(cs, ce, &reference.rows))
            .count();
        let del_in_speech_rows: usize = reference
            .rows
            .iter()
            .filter(|r| matches!(r.status, RowStatus::Speech(_)))
            .filter(|r| row_uncovered_is_deletion(r, &cue_ivs))
            .count();

        // Segmentation quality, SEPARATE row: per speech row, |mapped hyp
        // cues − 1| (a row's speech should be covered by exactly one cue),
        // averaged over speech rows. 0 = perfect cue alignment.
        let hyp_cues = art.b_spans.len();
        let seg_boundary_error = {
            let speech_rows = reference
                .rows
                .iter()
                .filter(|r| matches!(r.status, RowStatus::Speech(_)))
                .count() as f64;
            if speech_rows == 0.0 {
                0.0
            } else {
                let tot: f64 = reference
                    .rows
                    .iter()
                    .filter(|r| matches!(r.status, RowStatus::Speech(_)))
                    .map(|r| {
                        let n = art
                            .b_spans
                            .iter()
                            .filter(|s| {
                                (s.end.min(r.end) - s.start.max(r.start)).max(0.0)
                                    / (s.end - s.start)
                                    > 0.5
                            })
                            .count();
                        (n as f64 - 1.0).abs()
                    })
                    .sum();
                tot / speech_rows
            }
        };

        // Speaker errors: same plurality cue→row assignment as the
        // insertion/deletion classes; cues landing on silence (insertion
        // class) or UNINTEL rows are excluded from both numerator and
        // denominator. Compare speaker labels under BOTH attribution rules.
        let (speaker_matches, speaker_errors) = count_speaker_errors(
            &art.b_spans,
            &art.speakers_segment_rule,
            &art.speakers_wordwin_rule,
            &reference.rows,
        );

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
            ins_on_confirmed_silence,
            del_in_speech_rows,
            ref_scorable_words: ref_words,
            unintel_excluded_s: unintel_s,
            reviewed_rows,
            unintel_rows,
            unintel_s,
            scorable_s,
            seg_boundary_error,
            hyp_cues,
            ref_cues,
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
            "SCORE {} {} sub={} ins={} del={} wer={:.3} ref_scorable_w={} unintel_excl_s={:.2} ins_sil={} del_rows={} seg_err={:.3} cues={}/{} spk_err={}/{} (x2 rules) el_disagree_sec={:.3} rule_flips={}",
            line.clip,
            line.variant,
            line.sub,
            line.ins,
            line.del,
            line.wer,
            line.ref_scorable_words,
            line.unintel_excluded_s,
            line.ins_on_confirmed_silence,
            line.del_in_speech_rows,
            line.seg_boundary_error,
            line.hyp_cues,
            line.ref_cues,
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
        // `break` after the first parsed line (one REVIEW row per
        // reference, not per variant) trips clippy::never_loop (deny by
        // default) — write the take-first as a take-first.
        if let Some(line) = raw.lines().find(|l| !l.trim().is_empty()) {
            let s: serde_json::Value = serde_json::from_str(line).expect("score line");
            // REVIEW COMPLETION vs SCORABLE COVERAGE, visible here too: the
            // UNINTEL exclusion is fixed by the review and identical for
            // every variant — it can never be a per-variant scoring choice.
            println!(
                "REVIEW {} COMPLETE rows={} unintel_rows={} unintel_s={:.2} scorable_s={:.2}",
                s["clip"].as_str().unwrap_or("?"),
                s["reviewed_rows"].as_u64().unwrap_or(0),
                s["unintel_rows"].as_u64().unwrap_or(0),
                s["unintel_s"].as_f64().unwrap_or(f64::NAN),
                s["scorable_s"].as_f64().unwrap_or(f64::NAN),
            );
        }
        for line in raw.lines().filter(|l| !l.trim().is_empty()) {
            let s: serde_json::Value = serde_json::from_str(line).expect("score line");
            println!(
                "{:<8} {:<11} sub={:>3} ins={:>3} del={:>3} wer={:.3} unintel_excl_s={:.2} ins_sil={:>2} del_rows={:>2} seg_err={:.3} cues={:>3}/{} spk_err={:>3} (of {} matches) runs={:>2} EL-disagree={:.3} flips={:>3}",
                s["clip"].as_str().unwrap_or("?"),
                s["variant"].as_str().unwrap_or("?"),
                s["sub"].as_u64().unwrap_or(0),
                s["ins"].as_u64().unwrap_or(0),
                s["del"].as_u64().unwrap_or(0),
                s["wer"].as_f64().unwrap_or(f64::NAN),
                s["unintel_excluded_s"].as_f64().unwrap_or(f64::NAN),
                s["ins_on_confirmed_silence"].as_u64().unwrap_or(0),
                s["del_in_speech_rows"].as_u64().unwrap_or(0),
                s["seg_boundary_error"].as_f64().unwrap_or(f64::NAN),
                s["hyp_cues"].as_u64().unwrap_or(0),
                s["ref_cues"].as_u64().unwrap_or(0),
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

// ───────────────────────── tests ─────────────────────────
// Regressions for both 2026-09-13 incidents:
//   (1) FORMAT: the v1 two-line-per-span TSV with trailing-tab sensitivity
//       cost Andre a filled-in file. v2 is one line per row with a
//       never-empty terminal status field; these tests prove a plain text
//       editor save (trailing whitespace stripped, CRLF) cannot change the
//       parse.
//   (2) SPAN SET: rows derive from diarization over the AUDIO, never from a
//       decode; not-reviewed/missing rows block scoring; silence-vs-speech
//       rows are scored distinctly (insertion vs deletion classes);
//       segmentation invariance: identical words in identical order score
//       identically at 1 cue or 10 cues, with segmentation quality on a
//       SEPARATE row.

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

    const V2_HEADER: &str = "# Hand-review reference (PHI — local only, never commit)\n\
        # format: review-v2\n";

    fn v2_ref(statuses: &[(&str, &str, &str, &str, &str, &str)]) -> String {
        let mut s = String::from(V2_HEADER);
        for &(id, st, en, spk, text, status) in statuses {
            s.push_str(&format!("{id}\t{st}\t{en}\t{spk}\t{text}\t{status}\n"));
        }
        s
    }

    // ── format survival: save-and-reparse round trip ──

    #[test]
    fn round_trip_template_fill_and_editor_save_survives() {
        // The ACTUAL round trip that must never break: generate → human
        // edits in a plain text editor → editor strips trailing whitespace
        // → reparse.
        let rows = vec![ReviewRow {
            id: "row-01".into(),
            start: 0.0,
            end: 4.0,
            speaker: Some(0),
            speech: true,
        }];
        let reference_text = emit_reference(&rows, 4.0, &[]);
        let p = write_tmp("rt_template.txt", &reference_text);
        // Parser accepts its own unfilled output (template re-parse).
        let parsed = parse_reference(&p);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].status, RowStatus::NotReviewed);

        // Human fills the text field by hand; editor then strips trailing
        // whitespace on every line (the v1 killer).
        let edited = reference_text
            .lines()
            .map(|l| match l.strip_prefix("row-01\t") {
                // rest = "0.00\t4.00\tS1\t\t???" — insert text, set OK.
                Some(rest) => {
                    let cut = rest.trim_end_matches('?').trim_end();
                    format!("row-01\t{cut}\tTRUE WORDS HERE\tOK")
                }
                None => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let p2 = write_tmp("rt_filled.txt", &edited);
        let filled = parse_reference(&p2);
        assert_eq!(filled.len(), 1);
        assert_eq!(
            filled[0].status,
            RowStatus::Speech("TRUE WORDS HERE".into())
        );
    }

    #[test]
    fn editor_whitespace_mangling_cannot_change_field_count() {
        // v1 failure mode: trailing TAB eaten → field count changed. v2:
        // trailing whitespace stripped, CRLF line endings — all still parse
        // to the same rows.
        let raw = "# format: review-v2\r\n\
            row-01\t0.00\t4.00\tS1\tHELLO WORLD\tOK  \r\n\
            row-02\t4.00\t6.00\t-\t\tOK\r\n";
        let p = write_tmp("mangled.txt", raw);
        let rows = parse_reference(&p);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, RowStatus::Speech("HELLO WORLD".into()));
        assert_eq!(rows[1].status, RowStatus::ConfirmedSilence);
    }

    // ── unintelligible token: reviewed-but-unresolvable ≠ not-reviewed ──

    #[test]
    fn unintelligible_token_is_distinct_from_not_reviewed() {
        // Three states must never collapse into two: ??? = never reached,
        // OK = resolved, UNINTEL = reached but unresolvable by ear.
        let raw = "# format: review-v2\n\
            row-01\t0.00\t4.00\tS1\ttrue words\tOK\n\
            row-02\t4.00\t6.00\tS2\t\tUNINTEL\n\
            row-03\t6.00\t8.00\t-\t\t???\n\
            row-04\t8.00\t9.00\t-\t\tUNINTEL  \r\n";
        let p = write_tmp("unintel.txt", raw);
        let rows = parse_reference(&p);
        assert_eq!(rows[0].status, RowStatus::Speech("true words".into()));
        assert_eq!(rows[1].status, RowStatus::Unintelligible);
        assert_eq!(rows[2].status, RowStatus::NotReviewed);
        // Trailing spaces + CR after the token still parse (editor save).
        assert_eq!(rows[3].status, RowStatus::Unintelligible);
    }

    #[test]
    fn unintelligible_rows_do_not_block_scoring_and_yield_no_tokens() {
        // The reason UNINTEL exists: a row the reviewer TRIED and could not
        // resolve must be distinguishable from one he never reached — and
        // must not wedge the whole reference unreviewable.
        let dir = std::env::temp_dir().join("fs_local_eval_tests/unintel");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("clip-03.review.txt"),
            v2_ref(&[
                ("row-01", "0.00", "4.00", "S1", "", "???"),
                ("row-02", "4.00", "8.00", "S2", "", "???"),
            ]) + "# Duration: 8.0s.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("clip-03.filled.txt"),
            v2_ref(&[
                ("row-01", "0.00", "4.00", "S1", "true words here", "OK"),
                ("row-02", "4.00", "8.00", "S2", "", "UNINTEL"),
            ]),
        )
        .unwrap();
        let r = load_reviewed(&dir, "clip-03");
        assert_eq!(r.not_reviewed, 0, "UNINTEL is reviewed; nothing blocks");
        assert_eq!(r.ref_tokens, tokens("true words here"));
    }

    #[test]
    fn template_header_documents_unintelligible_token() {
        // Andre fills against the header text alone: the token must be
        // explained where he will read it.
        let rows = vec![ReviewRow {
            id: "row-01".into(),
            start: 0.0,
            end: 4.0,
            speaker: Some(0),
            speech: true,
        }];
        let text = emit_reference(&rows, 4.0, &[]);
        assert!(text.contains("UNINTEL"), "header must explain the token");
    }

    // ── cue→row binning: proportional overlap, exact boundaries ──
    // (Codie review 2026-09-13: the >50%-of-cue rule used everywhere
    // mis-bins cues that straddle a speech/silence boundary AND cues
    // longer than the row they fully contain — the ratio's denominator
    // was the CUE, so a 2x-long cue fully covering a short speech row
    // counted as BOTH an insertion and a deletion.)

    #[test]
    fn insertion_vs_deletion_classes_use_row_coverage_not_cue_fraction() {
        // Speech row [10,11) fully inside a mostly-silence cue [8,16):
        // silence overlap 7s vs speech 1s → INSERTION; but the row is
        // 100% covered → NOT a deletion. The old cue-denominator rule
        // binned it as both.
        let rows = vec![
            RefRow {
                id: "row-01".into(),
                start: 8.0,
                end: 10.0,
                speaker: None,
                status: RowStatus::ConfirmedSilence,
            },
            RefRow {
                id: "row-02".into(),
                start: 10.0,
                end: 11.0,
                speaker: Some(0),
                status: RowStatus::Speech("hi".into()),
            },
            RefRow {
                id: "row-03".into(),
                start: 11.0,
                end: 16.0,
                speaker: None,
                status: RowStatus::ConfirmedSilence,
            },
        ];
        let cue = (8.0f64, 16.0f64);
        assert!(
            cue_is_insertion_class(cue.0, cue.1, &rows),
            "7s silence vs 1s speech"
        );
        assert_eq!(
            row_covered_fraction(&rows[1], &[cue]),
            1.0,
            "row fully covered"
        );
        assert!(!row_uncovered_is_deletion(&rows[1], &[cue]));
    }

    #[test]
    fn exact_fifty_fifty_straddle_resolves_toward_speech() {
        // Cue exactly half over silence, half over speech: NOT an
        // insertion (exact tie resolves toward speech), and the speech
        // row is fully covered on its half.
        let rows = vec![
            RefRow {
                id: "row-01".into(),
                start: 0.0,
                end: 5.0,
                speaker: None,
                status: RowStatus::ConfirmedSilence,
            },
            RefRow {
                id: "row-02".into(),
                start: 5.0,
                end: 10.0,
                speaker: Some(1),
                status: RowStatus::Speech("words".into()),
            },
        ];
        let cue = (0.0f64, 10.0f64);
        assert!(
            !cue_is_insertion_class(cue.0, cue.1, &rows),
            "5s vs 5s tie → not insertion"
        );
        assert_eq!(row_covered_fraction(&rows[1], &[cue]), 1.0);
    }

    #[test]
    fn cue_straddling_with_majority_silence_is_insertion_only() {
        // 4s over silence vs 2s over speech → insertion; the speech row
        // is still 100% covered on its own interval → NOT a deletion.
        let rows = vec![
            RefRow {
                id: "row-01".into(),
                start: 0.0,
                end: 8.0,
                speaker: None,
                status: RowStatus::ConfirmedSilence,
            },
            RefRow {
                id: "row-02".into(),
                start: 8.0,
                end: 10.0,
                speaker: Some(0),
                status: RowStatus::Speech("ok".into()),
            },
        ];
        let cue = (4.0f64, 10.0f64);
        assert!(cue_is_insertion_class(cue.0, cue.1, &rows));
        assert!(!row_uncovered_is_deletion(&rows[1], &[cue]));
    }

    #[test]
    fn deletion_threshold_is_half_the_row_duration() {
        // Speech row [0,4): 2.0s of cue time = exactly 50% → covered;
        // 1.99s → below half → deletion.
        let row = RefRow {
            id: "row-01".into(),
            start: 0.0,
            end: 4.0,
            speaker: Some(0),
            status: RowStatus::Speech("x".into()),
        };
        assert!(
            !row_uncovered_is_deletion(&row, &[(0.0, 2.0)]),
            "exactly 50% = covered"
        );
        assert!(
            row_uncovered_is_deletion(&row, &[(0.0, 1.99)]),
            "49.75% = deletion"
        );
    }

    #[test]
    fn plurality_assignment_breaks_ties_toward_the_earlier_row() {
        // Rows partition time; a cue split across several rows belongs to
        // the one it overlaps most; an exact tie goes to the EARLIEST row
        // (rows are time-sorted, so this is deterministic).
        let rows = vec![
            RefRow {
                id: "row-01".into(),
                start: 0.0,
                end: 2.0,
                speaker: Some(0),
                status: RowStatus::Speech("a".into()),
            },
            RefRow {
                id: "row-02".into(),
                start: 2.0,
                end: 4.0,
                speaker: Some(1),
                status: RowStatus::Speech("b".into()),
            },
            RefRow {
                id: "row-03".into(),
                start: 4.0,
                end: 8.0,
                speaker: None,
                status: RowStatus::ConfirmedSilence,
            },
        ];
        // Plurality row by overlap; an EXACT tie goes to the earliest row
        // (rows are time-sorted, so this is deterministic).
        assert_eq!(assign_cue_to_row(0.0, 4.0, &rows).unwrap().id, "row-01"); // 2s vs 2s tie → row-01
        assert_eq!(assign_cue_to_row(1.0, 7.0, &rows).unwrap().id, "row-03"); // 1s/2s/3s
        assert_eq!(assign_cue_to_row(0.0, 10.0, &rows).unwrap().id, "row-03"); // 2s/2s/4s
    }

    #[test]
    fn hallucination_over_silence_is_not_a_speaker_error() {
        // A cue whose maximum-overlap row is CONFIRMED SILENCE is the
        // insertion class, not a speaker error — counting it as both was
        // the mis-bin. Same for UNINTEL rows (speaker there is an
        // unconfirmed suggestion).
        let rows = vec![
            RefRow {
                id: "row-01".into(),
                start: 0.0,
                end: 6.0,
                speaker: None,
                status: RowStatus::ConfirmedSilence,
            },
            RefRow {
                id: "row-02".into(),
                start: 6.0,
                end: 12.0,
                speaker: Some(0),
                status: RowStatus::Speech("real words".into()),
            },
        ];
        let spans = vec![
            Span {
                id: "span-A".into(),
                start: 0.5,
                end: 5.5,
                text: "HALLUCINATION".into(),
                words: vec![],
            },
            Span {
                id: "span-B".into(),
                start: 6.5,
                end: 11.5,
                text: "real words".into(),
                words: vec![],
            },
        ];
        let rule = vec![Some("Speaker 1".into()), Some("Speaker 1".into())];
        let (matches, errors) = count_speaker_errors(&spans, &rule, &rule, &rows);
        assert_eq!(matches, 1, "only the speech-row cue is in the denominator");
        assert_eq!(errors, 0, "cue over silence must not be a speaker error");
    }

    // ── duplicate row IDs and status-token junk (Codie probes 2+3) ──

    #[test]
    fn duplicate_row_ids_are_refused_not_scored() {
        // A duplicated ID would silently double-count its words and shift
        // the WER denominator; load_reviewed must refuse the file.
        let dir = std::env::temp_dir().join("fs_local_eval_tests/dupes");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("clip-03.review.txt"),
            v2_ref(&[("row-01", "0.00", "4.00", "S1", "", "???")]),
        )
        .unwrap();
        std::fs::write(
            dir.join("clip-03.filled.txt"),
            v2_ref(&[
                ("row-01", "0.00", "4.00", "S1", "some words", "OK"),
                ("row-01", "0.00", "4.00", "S1", "some words", "OK"),
            ]),
        )
        .unwrap();
        let dir = dir.clone();
        let result = std::panic::catch_unwind(move || load_reviewed(&dir, "clip-03"));
        assert!(result.is_err(), "duplicate row IDs must refuse, not score");
    }

    #[test]
    fn status_trailing_junk_is_rejected_not_trimmed() {
        // Documented tolerance: ASCII spaces around the token (editor
        // save) are fine. Anything else — including INVISIBLE non-ASCII
        // whitespace like NBSP, which today's blanket .trim() silently
        // accepts — is junk and must be refused with an actionable error.
        let bad = ["OKX", "OK\u{a0}", "\u{a0}OK", "O K", "OK;", "UNINTEL?"];
        for (i, status) in bad.iter().enumerate() {
            let raw = format!(
                "# format: review-v2\nrow-01\t0.00\t4.00\tS1\twords\t{}\n",
                status
            );
            let p = write_tmp(&format!("junk{i}.txt"), &raw);
            let result = std::panic::catch_unwind(move || parse_reference(&p));
            assert!(result.is_err(), "{status:?} must be refused");
        }
        // Documented acceptance: surrounding ASCII spaces survive an
        // editor save.
        let raw = "# format: review-v2\nrow-01\t0.00\t4.00\tS1\twords\t OK \nrow-02\t4.00\t6.00\tS1\t\tUNINTEL  \n";
        let p = write_tmp("junk_ok.txt", raw);
        let rows = parse_reference(&p);
        assert_eq!(rows[0].status, RowStatus::Speech("words".into()));
        assert_eq!(rows[1].status, RowStatus::Unintelligible);
    }

    #[test]
    fn legacy_v1_file_is_refused_not_misparsed() {
        // v1 shape (two lines per span, || separators). MUST hard-refuse:
        // re-filling a legacy file is how the first incident happened.
        let raw = "# Hand-correction template (PHI — local only, never commit)\n\
            span-A\t[Speaker 2]\t||\t\t||\t\n    You have reached the voicemail box of\n";
        let p = write_tmp("v1.txt", raw);
        let result = std::panic::catch_unwind(move || parse_reference(&p));
        assert!(result.is_err(), "v1 file must be refused, not parsed");
    }

    #[test]
    fn unfilled_rows_and_missing_rows_block_scoring() {
        // ??? row and a deleted row both count as not reviewed.
        let dir = std::env::temp_dir().join("fs_local_eval_tests/refaudit");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("clip-03.review.txt"),
            v2_ref(&[
                ("row-01", "0.00", "4.00", "S1", "", "???"),
                ("row-02", "4.00", "8.00", "S2", "", "???"),
            ]) + "# Duration: 8.0s.\n",
        )
        .unwrap();
        // Filled copy: row-01 left ??? (not reviewed), row-02 deleted
        // entirely — BOTH must count as not reviewed.
        std::fs::write(
            dir.join("clip-03.filled.txt"),
            v2_ref(&[("row-01", "0.00", "4.00", "S1", "", "???")]),
        )
        .unwrap();
        let r = load_reviewed(&dir, "clip-03");
        assert_eq!(r.not_reviewed, 2, "1 ??? + 1 missing row");
        assert_eq!(r.first_not_reviewed.as_deref(), Some("row-01"));
    }

    // ── scoring semantics: silence vs speech, distinct classes ──

    #[test]
    fn confirmed_silence_row_with_asr_output_is_insertion_class() {
        // Hypothesis cue sits inside a confirmed-silence interval: the
        // localized insertion count must see it.
        let rows = vec![RefRow {
            id: "row-01".into(),
            start: 0.0,
            end: 4.0,
            speaker: None,
            status: RowStatus::ConfirmedSilence,
        }];
        let n = cue_is_insertion_class(0.5, 3.5, &rows);
        assert!(n, "ASR output on a confirmed-silence row = insertion");
    }

    #[test]
    fn speech_row_with_no_asr_cue_is_deletion_class() {
        // Human transcribed speech; the decode emitted nothing there: the
        // localized deletion count must see it (invisible to a
        // decode-keyed reference).
        let row = RefRow {
            id: "row-01".into(),
            start: 10.0,
            end: 14.0,
            speaker: Some(0),
            status: RowStatus::Speech("DROPPED UTTERANCE WORDS".into()),
        };
        assert!(row_uncovered_is_deletion(&row, &[]), "no cue = deletion");
    }

    // ── whole-declared-interval completion gate (repo-auditor) ──
    // A 100%-row-reviewed file proves only that the PROPOSED rows were
    // reviewed. Completion must account for the ENTIRE declared interval.

    #[test]
    fn declared_interval_gap_blocks_scoring_as_unreviewed() {
        // Template partitions [0,8]. The filled copy edits columns 2-3 on
        // row-01 (ID untouched — the row-set audit passes) so [4,8) is
        // covered by NO row: those seconds are unreviewed and scoring must
        // refuse, exactly like a ??? row.
        let dir = std::env::temp_dir().join("fs_local_eval_tests/gapaudit");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("clip-03.review.txt"),
            v2_ref(&[
                ("row-01", "0.00", "4.00", "S1", "", "???"),
                ("row-02", "4.00", "8.00", "S1", "", "???"),
            ]) + "# Duration: 8.0s.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("clip-03.filled.txt"),
            v2_ref(&[("row-01", "0.00", "4.00", "S1", "true words", "OK")]),
        )
        .unwrap();
        let r = load_reviewed(&dir, "clip-03");
        // Missing row-02 already blocks; the gap audit must ALSO see the
        // uncovered seconds independent of the row-set audit.
        assert!(r.not_reviewed > 0);
        assert!(
            (r.uncovered_s - 4.0).abs() < 1e-9,
            "uncovered seconds are unreviewed seconds, got {}",
            r.uncovered_s
        );
        assert_eq!(r.first_uncovered, Some((4.0, 8.0)));
    }

    #[test]
    fn added_row_over_a_gap_completes_the_review() {
        // Andre hears speech in a gap neither system proposed and adds
        // row-90 with its interval and text: the reference now covers the
        // declared interval, the row is scored as speech, and nothing
        // blocks. This is exactly the instruction he was given.
        let dir = std::env::temp_dir().join("fs_local_eval_tests/gapadd");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("clip-03.review.txt"),
            v2_ref(&[
                ("row-01", "0.00", "4.00", "S1", "", "???"),
                ("row-02", "6.00", "8.00", "S1", "", "???"),
            ]) + "# Duration: 8.0s.\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("clip-03.filled.txt"),
            v2_ref(&[
                ("row-01", "0.00", "4.00", "S1", "known words", "OK"),
                (
                    "row-90",
                    "4.00",
                    "6.00",
                    "S1",
                    "missed by both systems",
                    "OK",
                ),
                ("row-02", "6.00", "8.00", "S1", "", "OK"),
            ]),
        )
        .unwrap();
        let r = load_reviewed(&dir, "clip-03");
        assert_eq!(r.not_reviewed, 0);
        assert!(r.uncovered_s.abs() < 1e-9, "gap closed by row-90");
        let joined = r.ref_tokens.join(" ");
        assert!(joined.contains("missed by both systems"));
        assert!(joined.contains("known words"));
    }

    #[test]
    fn rows_out_of_time_order_still_cover_the_declared_interval() {
        // Codie re-gate of 77aa64b: the coverage walk ran in FILE order,
        // so a human row appended at the END of the file (the natural
        // add-a-row edit) rather than inserted in time position produced
        // FALSE uncovered seconds and refused a perfectly reviewed file.
        // Coverage is a property of the row SET, never of the file order.
        let dir = std::env::temp_dir().join("fs_local_eval_tests/addroworder");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("clip-03.review.txt"),
            v2_ref(&[
                ("row-01", "0.00", "4.00", "S1", "", "???"),
                ("row-02", "6.00", "8.00", "S1", "", "???"),
            ]) + "# Duration: 8.0s.\n",
        )
        .unwrap();
        // Codie's exact repro: file order [row-90, row-01, row-02]. The
        // three rows together cover [0,8) exactly.
        std::fs::write(
            dir.join("clip-03.filled.txt"),
            v2_ref(&[
                ("row-90", "4.00", "6.00", "S1", "appended gap row", "OK"),
                ("row-01", "0.00", "4.00", "S1", "known words", "OK"),
                ("row-02", "6.00", "8.00", "S1", "", "OK"),
            ]),
        )
        .unwrap();
        let r = load_reviewed(&dir, "clip-03");
        assert_eq!(r.not_reviewed, 0);
        assert!(
            r.uncovered_s.abs() < 1e-9,
            "coverage must be order-independent, got {}s uncovered",
            r.uncovered_s
        );
        let joined = r.ref_tokens.join(" ");
        assert!(joined.contains("appended gap row"));
        assert!(joined.contains("known words"));
    }

    #[test]
    fn overlapping_rows_are_refused_in_any_file_order() {
        // Mirror invariant (Codie): non-overlap is REAL — two rows over
        // the same seconds would double-count them (WER denominator,
        // scorable_s) — and the refusal must not become order-sensitive
        // in the other direction now that the walk sorts by start time.
        let dir = std::env::temp_dir().join("fs_local_eval_tests/overlaprefuse");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("clip-03.review.txt"),
            v2_ref(&[("row-01", "0.00", "8.00", "S1", "", "???")]) + "# Duration: 8.0s.\n",
        )
        .unwrap();
        // row-90 overlaps row-01 on [2,4): must refuse.
        std::fs::write(
            dir.join("clip-03.filled.txt"),
            v2_ref(&[
                ("row-01", "0.00", "4.00", "S1", "words", "OK"),
                ("row-90", "2.00", "6.00", "S1", "overlapping add", "OK"),
            ]),
        )
        .unwrap();
        let dir_ref = dir.clone();
        let result = std::panic::catch_unwind(move || load_reviewed(&dir_ref, "clip-03"));
        assert!(
            result.is_err(),
            "overlapping rows must refuse, not score — each second may be covered by exactly one row"
        );
    }

    #[test]
    fn reference_speech_outside_every_detected_turn_stays_in_the_denominator() {
        // repo-auditor (c): a Speech row the decode has NO cue over must
        // remain in the WER denominator as deletions — it must never
        // disappear from the evaluation. Whole-clip sid over the reference
        // tokens counts every one of its words as deleted when the
        // hypothesis is empty; the localized deletion class must agree.
        let missed = "speech neither system proposed";
        let rows = vec![
            RefRow {
                id: "row-01".into(),
                start: 0.0,
                end: 4.0,
                speaker: Some(0),
                status: RowStatus::Speech("decoded words".into()),
            },
            RefRow {
                id: "row-90".into(), // human-added over a detected gap
                start: 4.0,
                end: 7.0,
                speaker: Some(0),
                status: RowStatus::Speech(missed.into()),
            },
        ];
        // Hypothesis covers ONLY row-01's audio; nothing over [4,7).
        let hyp: Vec<String> = tokens("decoded words");
        let ref_tokens: Vec<String> = rows
            .iter()
            .filter_map(|r| match &r.status {
                RowStatus::Speech(t) => Some(tokens(t)),
                _ => None,
            })
            .flatten()
            .collect();
        let (sub, ins, del) = sid(&ref_tokens, &hyp);
        assert_eq!((sub, ins, del), (0, 0, 4), "every missed word deleted");
        // And the localized deletion class sees the gap row.
        let cue_ivs = vec![(0.0f64, 4.0f64)];
        assert!(!row_uncovered_is_deletion(&rows[0], &cue_ivs));
        assert!(
            row_uncovered_is_deletion(&rows[1], &cue_ivs),
            "gap speech is a deletion, not an absence"
        );
        // SCORABLE vs REVIEWED stay separate: the missed row's words count
        // in the denominator precisely because the review CONFIRMED them.
        assert_eq!(ref_tokens.len(), 6);
    }

    // ── segmentation invariance ──

    #[test]
    fn identical_words_score_identically_at_one_or_ten_cues() {
        // Same words, same order: 1 cue vs 10 cues must give identical
        // S/I/D (tokens are concatenated; cue count never enters WER).
        let words = "the quick brown fox jumps over the lazy dog today";
        let one = vec![Span {
            id: "span-A".into(),
            start: 0.0,
            end: 10.0,
            text: words.into(),
            words: vec![],
        }];
        let ten: Vec<Span> = words
            .split_whitespace()
            .enumerate()
            .map(|(i, w)| Span {
                id: format!("span-{}", char::from(b'A' + i as u8)),
                start: i as f64,
                end: i as f64 + 1.0,
                text: w.into(),
                words: vec![],
            })
            .collect();
        assert_eq!(ten.len(), 10);
        let ref_tokens: Vec<String> = tokens(words);
        let h1: Vec<String> = one.iter().flat_map(|s| tokens(&s.text)).collect();
        let h10: Vec<String> = ten.iter().flat_map(|s| tokens(&s.text)).collect();
        let (s1, i1, d1) = sid(&ref_tokens, &h1);
        let (s10, i10, d10) = sid(&ref_tokens, &h10);
        assert_eq!((s1, i1, d1), (0, 0, 0));
        assert_eq!((s10, i10, d10), (0, 0, 0));
        assert_eq!(h1, h10, "token streams identical");
    }

    #[test]
    fn cue_count_difference_is_visible_only_in_seg_error_not_wer() {
        // 10 cues over 4 speech rows: WER stays 0 while the SEPARATE
        // segmentation row exposes the cue-count delta.
        let words_per_row = [
            "alpha bravo charlie",
            "delta echo foxtrot",
            "golf hotel",
            "india juliet",
        ];
        let rows: Vec<RefRow> = words_per_row
            .iter()
            .enumerate()
            .map(|(i, t)| RefRow {
                id: format!("row-{:02}", i + 1),
                start: i as f64 * 4.0,
                end: i as f64 * 4.0 + 4.0,
                speaker: Some(0),
                status: RowStatus::Speech((*t).into()),
            })
            .collect();
        let all_words: Vec<&str> = words_per_row.iter().flat_map(|s| s.split(' ')).collect();
        assert_eq!(all_words.len(), 10);
        let ten: Vec<Span> = all_words
            .iter()
            .enumerate()
            .map(|(i, w)| Span {
                id: format!("span-{}", char::from(b'A' + i as u8)),
                start: i as f64 * 1.6,
                end: i as f64 * 1.6 + 1.6,
                text: (*w).into(),
                words: vec![],
            })
            .collect();
        let ref_tokens: Vec<String> = rows
            .iter()
            .filter_map(|r| match &r.status {
                RowStatus::Speech(t) => Some(tokens(t)),
                _ => None,
            })
            .flatten()
            .collect();
        let hyp_tokens: Vec<String> = ten.iter().flat_map(|s| tokens(&s.text)).collect();
        let (s, i, d) = sid(&ref_tokens, &hyp_tokens);
        assert_eq!((s, i, d), (0, 0, 0), "WER must not see cue count");
        let seg: f64 = rows
            .iter()
            .map(|r| {
                let n = ten
                    .iter()
                    .filter(|sp| {
                        (sp.end.min(r.end) - sp.start.max(r.start)).max(0.0) / (sp.end - sp.start)
                            > 0.5
                    })
                    .count();
                (n as f64 - 1.0).abs()
            })
            .sum::<f64>()
            / rows.len() as f64;
        assert!(seg > 0.0, "seg error must expose the cue-count delta");
    }

    // ── whisper-independent row derivation ──

    #[test]
    fn review_rows_cover_whole_clip_and_partition() {
        let turns = vec![
            (0usize, 2.3f64, 4.0f64),
            (1, 4.3, 7.2),
            (1, 7.2, 10.0),
            (1, 10.0, 20.0),
            (0, 20.0, 21.2),
        ];
        let rows = review_rows_from_turns(&turns, 30.1);
        assert!((rows.first().unwrap().start - 0.0).abs() < 1e-9);
        for w in rows.windows(2) {
            assert!((w[0].end - w[1].start).abs() < 1e-9);
        }
        assert!((rows.last().unwrap().end - 30.1).abs() < 1e-9);
        assert!(!rows[0].speech, "leading silence row");
        assert!(rows[1].speech);
        assert!(rows.iter().all(|r| r.end - r.start <= MAX_ROW_S + 1e-9));
    }

    #[test]
    fn empty_diarization_still_yields_full_silence_coverage() {
        let rows = review_rows_from_turns(&[], 30.0);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|r| !r.speech));
        assert!((rows.last().unwrap().end - 30.0).abs() < 1e-9);
    }

    // ── sid kernel (kept from the v1 test set) ──

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

    #[test]
    fn el_cue_parser_extracts_timestamps_and_text() {
        let raw = "00:00:02,420 --> 00:00:04,460 [Speaker 0]\nYou have reached\n\n00:00:07,340 --> 00:00:08,860 [Speaker 1]\nSecond cue\n";
        let p = write_tmp("el.txt", raw);
        let cues = parse_el_cues(&p);
        assert_eq!(cues.len(), 2);
        assert!((cues[0].0 - 2.42).abs() < 1e-6);
        assert!((cues[0].1 - 4.46).abs() < 1e-6);
        assert_eq!(cues[0].2, "You have reached");
        assert_eq!(cues[1].2, "Second cue");
    }
}
