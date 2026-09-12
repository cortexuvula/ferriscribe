//! D4 merge-path eval, option (b): SYNTHETIC WhisperSegments derived from the
//! GT turns + the REAL diarizer output, scored through the REAL D4 merge
//! (`merge::merge_segments_with_speakers`: per-speaker overlap aggregation,
//! >=70% dominance assigns, else None/ambiguous).
//!
//! Why synthetic segments (option b): the fixture is hard-concatenated with
//! NO silence at handoffs, and on such audio whisper.cpp's segmenter can
//! return ONE giant segment smeared over the whole clip — the merge then
//! correctly returns None (ambiguity rule) and a 0% number measures the
//! segmenter fixture artifact, not the merge. Deriving segments from the GT
//! turns isolates the quantity the D-series tracks: given segmentable turns,
//! how much speaking time does the diarize->merge path attribute correctly?
//!
//! Method:
//!   1. Synthetic segments = the 5 GT turns (start/end from ground truth).
//!      Words per segment: the segment is divided into equal-duration
//!      word slots (declared uniform assumption — synthetic segments carry
//!      no token timings). 10 words per turn.
//!   2. Real diarizer (pyannote segmentation + CAM++ embeddings, prod path)
//!      runs on the fixture audio.
//!   3. Real D4 merge labels each synthetic segment.
//!   4. cluster-id -> GT speaker resolved by majority turn overlap
//!      (permutation-safe).
//!   5. Word granularity: a word slot is CORRECT iff its segment is labeled
//!      and the label maps to the GT speaker that covers the majority of
//!      the slot's interval. Attribution = correct word-time / total
//!      word-time. Segment granularity: a segment is CORRECT iff labeled
//!      and >50% of its word-time is in GT turns of the assigned speaker.
//!   6. DER (250 ms collar, permutation-invariant, same algorithm as the
//!      gated eval's der_with_collar) computed from the diarizer turns —
//!      D4 does not touch the diarizer, so DER is unchanged by the merge.
//!
//! Run: FERRISCRIBE_DIAR_EVAL=<dir with mix_two_speaker_nosilence.wav + pyannote/> \
//!      cargo run -p medical-stt-providers --example d4_merge_eval --release

use std::collections::HashMap;
use std::path::PathBuf;

use medical_stt_providers::diarization::SpeakerDiarizer;
use medical_stt_providers::merge;
use medical_stt_providers::whisper::WhisperSegment;

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

    // ---- (1) Synthetic WhisperSegments = GT turns (option b).
    let synth: Vec<WhisperSegment> = gt_turns
        .iter()
        .enumerate()
        .map(|(i, &(_, s, e))| WhisperSegment {
            text: format!("synthetic turn {i}"),
            start: s,
            end: e,
        })
        .collect();
    // Uniform word slots per segment.
    let seg_words: Vec<Vec<(f64, f64)>> = gt_turns
        .iter()
        .map(|&(_, s, e)| {
            let dur = (e - s) / WORDS_PER_TURN as f64;
            (0..WORDS_PER_TURN)
                .map(|k| (s + k as f64 * dur, s + (k + 1) as f64 * dur))
                .collect()
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

    // ---- (3) THE MERGE UNDER TEST (real D4 code, unchanged).
    let merged = merge::merge_segments_with_speakers(&synth, &turns);
    let n_none = merged.iter().filter(|s| s.speaker.is_none()).count();
    eprintln!(
        "merge: {} segments, {} None-speaker (ambiguous)",
        merged.len(),
        n_none
    );
    for s in &merged {
        eprintln!(
            "  [ {:.3} .. {:.3} ] {:?} {}",
            s.start, s.end, s.speaker, s.text
        );
    }

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

    // ---- (5) Score at word and segment granularity.
    let mut total_word_time = 0.0f64;
    let mut credited_word_time = 0.0f64;
    let mut seg_correct = 0usize;
    for (si, s) in merged.iter().enumerate() {
        let words = seg_words.get(si).map(|w| w.as_slice()).unwrap_or(&[]);
        let seg_total: f64 = words.iter().map(|&(t0, t1)| (t1 - t0).max(0.0)).sum();
        total_word_time += seg_total;
        let assigned_gt = s.speaker.as_ref().and_then(|lbl| {
            lbl.rsplit(' ')
                .next()
                .and_then(|tok| tok.parse::<usize>().ok())
                .and_then(|spk| cluster_to_gt.get(&(spk - 1)).copied())
        });
        let mut seg_credit = 0.0f64;
        for &(t0, t1) in words {
            // Word slot's GT speaker = majority overlap of the slot interval.
            let mut best_gt: Option<(usize, f64)> = None;
            for &(gsp, gs, ge) in &gt_turns {
                let o = (t1.min(ge) - t0.max(gs)).max(0.0);
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
        let correct = assigned_gt.is_some() && frac > 0.5;
        if correct {
            seg_correct += 1;
        }
        eprintln!(
            "score seg {si}: label={:?} word_time={:.2}s credit={:.2}s frac={:.1}% correct={}",
            s.speaker,
            seg_total,
            seg_credit,
            frac * 100.0,
            correct
        );
    }

    let accuracy = if total_word_time > 0.0 {
        credited_word_time / total_word_time
    } else {
        0.0
    };

    // ---- (6) DER from the diarizer turns (same algorithm as gated eval).
    let der = der_with_collar(&turns, &gt_turns, duration_s, 0.25);

    eprintln!(
        "word-granularity attribution: {:.2}s / {:.2}s = {:.1}%",
        credited_word_time,
        total_word_time,
        accuracy * 100.0
    );
    eprintln!(
        "segment granularity: {}/{} correct, {} None-labeled",
        seg_correct,
        merged.len(),
        n_none
    );
    eprintln!("DER (250ms collar): {:.1}%", der * 100.0);

    println!(
        "RESULT: method=option-b-synthetic-segments attribution_pct={:.1} word_time={:.2} credited={:.2} seg_correct={} seg_total={} none_segs={} der_pct={:.1}",
        accuracy * 100.0,
        total_word_time,
        credited_word_time,
        seg_correct,
        merged.len(),
        n_none,
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
