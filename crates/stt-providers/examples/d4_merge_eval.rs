//! D4 merge-path eval: WhisperSegments + the REAL diarizer output, scored
//! through the REAL D4 merge (`merge::merge_segments_with_speakers`),
//! comparing WORD-WINDOW attribution (token-level DTW windows, the
//! refinement under test) against SEGMENT-WINDOW attribution (the previous
//! rule) on the SAME eval corpus and the SAME diarizer turns.
//!
//! Two segment sources:
//!
//! - `FERRISCRIBE_STT_MODEL=<ggml>` set → REAL whisper.cpp decode of the
//!   fixture audio through the production transcriber (segments carry
//!   token-level DTW word windows). Content-free: no transcript text is
//!   printed; segments are consumed for timings/counts only.
//! - unset → SYNTHETIC segments derived from the GT turns (option b, the
//!   original method): the fixture is hard-concatenated with NO silence at
//!   handoffs, so whisper.cpp's segmenter may return one giant smeared
//!   segment; synthetic segments isolate the merge quantity the D-series
//!   tracks. Synthetic segments carry uniform word slots (declared
//!   assumption).
//!
//! Scoring (both attribution modes, identical procedure): a word window is
//! CORRECT iff its segment is labeled and the label maps (cluster->GT) to
//! the GT speaker covering the majority of the window. Attribution =
//! correct word-time / total word-time. Segment granularity: correct iff
//! labeled and >50% of its word-time is GT-correct.
//!
//! Run:
//!   FERRISCRIBE_DIAR_EVAL=<dir with mix_two_speaker_nosilence.wav + pyannote/> \
//!   [FERRISCRIBE_STT_MODEL=<ggml bin>] \
//!   cargo run -p medical-stt-providers --example d4_merge_eval --release

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use medical_core::types::TranscriptSegment;
use medical_stt_providers::diarization::SpeakerDiarizer;
use medical_stt_providers::merge;
use medical_stt_providers::whisper::WordTiming;
use medical_stt_providers::whisper::{WhisperContextCache, WhisperSegment, WhisperTranscriber};

/// Words per synthetic turn (uniform word slots — declared assumption).
const WORDS_PER_TURN: usize = 10;

fn main() {
    let Some(dir) = std::env::var_os("FERRISCRIBE_DIAR_EVAL") else {
        eprintln!("skipping: set FERRISCRIBE_DIAR_EVAL=<staging dir>");
        std::process::exit(2);
    };
    let root = PathBuf::from(&dir);
    let mix = root.join("mix_two_speaker_nosilence.wav");
    let models = root.join("pyannote");
    for (what, p) in [("fixture", &mix), ("pyannote models", &models)] {
        assert!(p.exists(), "missing {what}: {}", p.display());
    }

    // ---- Decode fixture WAV (16 kHz mono i16) — same parser as the eval test.
    let bytes = std::fs::read(&mix).expect("read mix wav");
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
    let samples: Vec<i16> = bytes[doff..doff + dsz]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c))
        .collect();
    let duration_s = samples.len() as f64 / 16000.0;

    // ---- GT: spk_A/spk_B alternating, boundaries 5.0 / 9.7 / 14.0 / 18.0.
    let gt_boundaries = [5.0_f64, 9.7, 14.0, 18.0];
    let mut gt_turns: Vec<(usize, f64, f64)> = Vec::new();
    let mut speaker = 0usize;
    let mut prev = 0.0;
    for &b in gt_boundaries.iter().chain(std::iter::once(&duration_s)) {
        gt_turns.push((speaker, prev, b));
        prev = b;
        speaker = 1 - speaker;
    }

    // ---- (1) Segments: real whisper decode (with word windows) or synthetic.
    let (segments, seg_source) = if let Some(model) = std::env::var_os("FERRISCRIBE_STT_MODEL") {
        let cache = Arc::new(WhisperContextCache::new(Arc::new(
            move |path: &std::path::Path| {
                medical_stt_providers::whisper::load_whisper_context(path)
            },
        )));
        let transcriber = WhisperTranscriber::new(PathBuf::from(&model), cache);
        let f32_samples: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
        let segs = transcriber
            .transcribe(&f32_samples, Some("en"))
            .expect("whisper decode");
        eprintln!(
            "whisper decode: {} segments, {} timed word windows (text not read)",
            segs.len(),
            segs.iter().map(|s| s.words.len()).sum::<usize>()
        );
        (segs, "real-whisper")
    } else {
        eprintln!("FERRISCRIBE_STT_MODEL unset — synthetic option-b segments");
        let synth: Vec<WhisperSegment> = gt_turns
            .iter()
            .enumerate()
            .map(|(i, &(_, s, e))| WhisperSegment {
                text: format!("synthetic turn {i}"),
                start: s,
                end: e,
                words: Vec::new(),
            })
            .collect();
        (synth, "option-b-synthetic-segments")
    };

    // Word windows per segment: DTW windows when present, else uniform slots.
    let seg_words: Vec<Vec<WordTiming>> = segments
        .iter()
        .map(|ws| {
            if ws.words.is_empty() {
                let n = WORDS_PER_TURN as f64;
                let dur = (ws.end - ws.start) / n;
                (0..WORDS_PER_TURN)
                    .map(|k| WordTiming {
                        start: ws.start + k as f64 * dur,
                        end: ws.start + (k + 1) as f64 * dur,
                    })
                    .collect()
            } else {
                ws.words.clone()
            }
        })
        .collect();

    // ---- (2) Real diarizer on the fixture audio (prod path).
    let diarizer = SpeakerDiarizer::new(
        models.join("segmentation-3.0.onnx"),
        models.join("wespeaker_en_voxceleb_CAM++.onnx"),
    );
    let turns = diarizer.diarize(&samples, 16000, None).expect("diarize");
    eprintln!(
        "diarize: {} turns: {:?}",
        turns.len(),
        turns
            .iter()
            .map(|t| (t.speaker_id, (t.start, t.end)))
            .collect::<Vec<_>>()
    );

    // ---- (3) THE MERGE UNDER TEST, both attribution modes.
    // (3a) word-window: production merge with segments as decoded.
    let merged_word = merge::merge_segments_with_speakers(&segments, &turns);
    // (3b) segment-window: identical segments with word windows stripped,
    // forcing the pre-refinement rule on the SAME corpus + turns.
    let stripped: Vec<WhisperSegment> = segments
        .iter()
        .map(|ws| WhisperSegment {
            text: ws.text.clone(),
            start: ws.start,
            end: ws.end,
            words: Vec::new(),
        })
        .collect();
    let merged_seg = merge::merge_segments_with_speakers(&stripped, &turns);

    // ---- (4) cluster-id -> GT speaker by majority turn overlap.
    let mut overlap: HashMap<usize, HashMap<usize, f64>> = HashMap::new();
    for t in &turns {
        let entry = overlap.entry(t.speaker_id).or_default();
        for &(gsp, gs, ge) in &gt_turns {
            let o = (t.end.min(ge) - t.start.max(gs)).max(0.0);
            if o > 0.0 {
                *entry.entry(gsp).or_insert(0.0) += o;
            }
        }
    }
    let mut cluster_to_gt: HashMap<usize, usize> = HashMap::new();
    for (&cid, by_gt) in &overlap {
        let mut best: Option<(usize, f64)> = None;
        for (&g, &o) in by_gt {
            if best.is_none() || o > best.unwrap().1 {
                best = Some((g, o));
            }
        }
        cluster_to_gt.insert(cid, best.unwrap().0);
    }
    eprintln!("cluster->GT speaker map: {:?}", cluster_to_gt);

    // ---- (5) Score both merges at word and segment granularity.
    let score = |merged: &[TranscriptSegment]| -> (f64, f64, usize, usize) {
        let mut total_word_time = 0.0f64;
        let mut credited_word_time = 0.0f64;
        let mut seg_correct = 0usize;
        let mut n_none = 0usize;
        for (si, s) in merged.iter().enumerate() {
            let words = seg_words.get(si).map(|w| w.as_slice()).unwrap_or(&[]);
            let seg_total: f64 = words.iter().map(|w| (w.end - w.start).max(0.0)).sum();
            total_word_time += seg_total;
            let assigned_gt = s.speaker.as_ref().and_then(|lbl| {
                lbl.rsplit(' ')
                    .next()
                    .and_then(|tok| tok.parse::<usize>().ok())
                    .and_then(|spk| cluster_to_gt.get(&(spk - 1)).copied())
            });
            if assigned_gt.is_none() {
                n_none += 1;
            }
            let mut seg_credit = 0.0f64;
            for w in words {
                // Word window's GT speaker = majority overlap of the window.
                let mut best_gt: Option<(usize, f64)> = None;
                for &(gsp, gs, ge) in &gt_turns {
                    let o = (w.end.min(ge) - w.start.max(gs)).max(0.0);
                    if best_gt.is_none() || o > best_gt.unwrap().1 {
                        best_gt = Some((gsp, o));
                    }
                }
                if let Some((_, o)) = best_gt.filter(|(g, _)| Some(*g) == assigned_gt) {
                    seg_credit += o;
                }
            }
            credited_word_time += seg_credit;
            let frac = if seg_total > 0.0 {
                seg_credit / seg_total
            } else {
                0.0
            };
            if assigned_gt.is_some() && frac > 0.5 {
                seg_correct += 1;
            }
        }
        (credited_word_time, total_word_time, seg_correct, n_none)
    };

    let (cred_w, tot_w, segc_w, none_w) = score(&merged_word);
    let (cred_s, tot_s, segc_s, none_s) = score(&merged_seg);
    let acc_w = if tot_w > 0.0 { cred_w / tot_w } else { 0.0 };
    let acc_s = if tot_s > 0.0 { cred_s / tot_s } else { 0.0 };

    // ---- (6) DER from the diarizer turns (unchanged by the merge).
    let der = der_with_collar(&turns, &gt_turns, duration_s, 0.25);

    eprintln!(
        "word-window attribution:  {:.2}s / {:.2}s = {:.1}% ({} seg correct, {} none)",
        cred_w,
        tot_w,
        acc_w * 100.0,
        segc_w,
        none_w
    );
    eprintln!(
        "segment-window attribution:{:.2}s / {:.2}s = {:.1}% ({} seg correct, {} none)",
        cred_s,
        tot_s,
        acc_s * 100.0,
        segc_s,
        none_s
    );

    println!(
        "RESULT: method={} attribution_word_window_pct={:.1} attribution_segment_window_pct={:.1} word_time={:.2} credited_word={:.2} credited_seg={:.2} seg_correct_word={} seg_correct_seg={} seg_total={} none_word={} none_seg={} der_pct={:.1}",
        seg_source,
        acc_w * 100.0,
        acc_s * 100.0,
        tot_w,
        cred_w,
        cred_s,
        segc_w,
        segc_s,
        merged_word.len(),
        none_w,
        none_s,
        der * 100.0
    );
}

/// Optimal-map DER on a 10 ms grid with a 250 ms collar around every internal
/// GT boundary and permutation-invariant speaker matching. Copied verbatim in
/// spirit from diarization.rs's gated eval (test-module private, not exported).
fn der_with_collar(
    turns: &[medical_stt_providers::diarization::SpeakerTurn],
    gt_turns: &[(usize, f64, f64)],
    duration_s: f64,
    collar_s: f64,
) -> f64 {
    const STEP: f64 = 0.01;
    let hyp_ids: Vec<usize> = {
        let mut v: Vec<usize> = turns.iter().map(|t| t.speaker_id).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let gt_speakers: Vec<usize> = {
        let mut v: Vec<usize> = gt_turns.iter().map(|&(s, _, _)| s).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    if hyp_ids.is_empty() {
        return 1.0;
    }
    let hyp_index = |id: usize| hyp_ids.iter().position(|&h| h == id).unwrap_or(0);

    let boundaries: Vec<f64> = gt_turns
        .iter()
        .flat_map(|&(_, s, e)| [s, e])
        .filter(|&b| b > 0.0 && b < duration_s)
        .collect();

    let mut choice = vec![0usize; hyp_ids.len()];
    let mut best_der = f64::INFINITY;
    loop {
        let mut confusion = 0.0_f64;
        let mut missed = 0.0_f64;
        let mut false_alarm = 0.0_f64;
        let mut total = 0.0_f64;
        let mut t = 0.0;
        while t < duration_s {
            let collared = boundaries.iter().any(|&b| (t - b).abs() <= collar_s);
            if !collared {
                let gt = gt_turns
                    .iter()
                    .find(|&&(_, s, e)| t >= s && t < e)
                    .map(|&(s, _, _)| s);
                let hyp = turns
                    .iter()
                    .find(|tr| t >= tr.start && t < tr.end)
                    .map(|tr| tr.speaker_id);
                match (gt, hyp) {
                    (Some(g), Some(h)) => {
                        total += STEP;
                        if gt_speakers[choice[hyp_index(h)]] != g {
                            confusion += STEP;
                        }
                    }
                    (Some(_), None) => {
                        total += STEP;
                        missed += STEP;
                    }
                    (None, Some(_)) => false_alarm += STEP,
                    (None, None) => {}
                }
            }
            t += STEP;
        }
        if total > 0.0 {
            let der = (confusion + missed + false_alarm) / total;
            if der < best_der {
                best_der = der;
            }
        }
        let mut i = choice.len();
        loop {
            if i == 0 {
                return best_der;
            }
            i -= 1;
            choice[i] += 1;
            if choice[i] < gt_speakers.len() {
                break;
            }
            choice[i] = 0;
        }
    }
}
