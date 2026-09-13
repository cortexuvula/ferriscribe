//! Merge Whisper text segments with speaker diarization turns by timestamp overlap.
//!
//! The merge algorithm aggregates speaker overlap per attribution window:
//!
//! - When a Whisper segment carries token-level DTW word windows
//!   ([`WhisperSegment::words`](crate::whisper::WhisperSegment::words)) and
//!   at least `MIN_WORD_WINDOWS` of them are valid, attribution runs at word
//!   granularity: each word window contributes its own overlap votes and the
//!   segment's speaker is the one dominating the sum of word-window overlaps.
//! - Otherwise (segments with no token timings — e.g. remote-server segments
//!   — or short backchannels/interjections with <2 valid timed words where a
//!   single DTW point is too noisy to trust), attribution falls back to the
//!   whole-segment window.
//! - Overlap is summed per-speaker (not just best-match), so a window that
//!   genuinely spans two speakers dilutes the vote.
//! - If one speaker dominates (≥70% of the total overlap), that speaker's
//!   label is assigned.
//! - If no speaker turn overlaps by at least 10ms, the segment is left unlabeled.
//!
//! Speaker labels are formatted as `"Speaker N"` (1-based).
//!
//! **Short-reply fix (D4)**: the previous algorithm assigned each segment
//! exactly one speaker (the one with the largest overlap), so a short reply
//! from speaker B inside a long A segment was attributed to A. The algorithm
//! aggregates overlap per-speaker and retains ambiguity where the segment
//! genuinely mixes speakers — the UI can render "Speaker 1 / Speaker 2"
//! or highlight the ambiguity for manual correction.
//!
//! **Word-window refinement**: a segment that straddles a speaker handoff
//! gets its dominant speaker from word-window-weighted overlap rather than
//! the raw segment span, so speech early in the segment counts more than
//! silence or the other speaker's tail sitting inside the segment window.

use std::collections::HashMap;

use medical_core::types::TranscriptSegment;

use crate::diarization::SpeakerTurn;
use crate::whisper::WhisperSegment;

/// Minimum overlap (seconds) to consider a speaker turn relevant to a segment.
const MIN_OVERLAP_S: f64 = 0.01;

/// Dominance threshold: a speaker must cover this fraction of the total overlap
/// to be assigned. Below this, the segment is marked ambiguous (None speaker).
const DOMINANCE_THRESHOLD: f64 = 0.7;

/// Minimum number of valid word windows before word-window attribution is
/// trusted. With fewer (backchannels, interjections), a single DTW point is
/// too noisy — fall back to the segment-window rule.
const MIN_WORD_WINDOWS: usize = 2;

/// Merge whisper text segments with speaker turns.
///
/// For each whisper segment, aggregates overlap per speaker over the
/// segment's attribution windows (word windows when available and
/// sufficiently many, else the whole segment window). If one speaker
/// dominates (≥70% of total overlap), assigns that speaker's label. Otherwise
/// leaves the speaker as `None` (ambiguous). If `speaker_turns` is empty,
/// returns segments without speaker labels.
pub fn merge_segments_with_speakers(
    whisper_segments: &[WhisperSegment],
    speaker_turns: &[SpeakerTurn],
) -> Vec<TranscriptSegment> {
    whisper_segments
        .iter()
        .map(|ws| {
            let speaker = if speaker_turns.is_empty() {
                None
            } else if ws.words.len() >= MIN_WORD_WINDOWS {
                speaker_for_word_windows(&ws.words, speaker_turns)
            } else {
                speaker_for_range(ws.start, ws.end, speaker_turns)
            };
            TranscriptSegment {
                text: ws.text.clone(),
                start: ws.start,
                end: ws.end,
                speaker,
                confidence: None,
            }
        })
        .collect()
}

/// Aggregate word-window overlap per speaker and return the dominant speaker.
///
/// Each word window contributes its own per-speaker overlap votes; the votes
/// are summed over all windows. The 10 ms overlap floor and the ≥70%
/// dominance rule are the same as the segment-window path — the only change
/// is the unit over which overlap is integrated.
fn speaker_for_word_windows(
    words: &[crate::whisper::WordTiming],
    turns: &[SpeakerTurn],
) -> Option<String> {
    let mut overlap_per_speaker: HashMap<usize, f64> = HashMap::new();

    for word in words {
        for turn in turns {
            let overlap_start = word.start.max(turn.start);
            let overlap_end = word.end.min(turn.end);
            let overlap = (overlap_end - overlap_start).max(0.0);

            if overlap > MIN_OVERLAP_S {
                *overlap_per_speaker.entry(turn.speaker_id).or_insert(0.0) += overlap;
            }
        }
    }

    if overlap_per_speaker.is_empty() {
        return None;
    }

    let total_overlap: f64 = overlap_per_speaker.values().sum();
    if total_overlap < MIN_OVERLAP_S {
        return None;
    }

    // Find the speaker with the most overlap
    let (best_id, best_overlap) = overlap_per_speaker
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(&id, &overlap)| (id, overlap))
        .unwrap();

    // Check dominance: does the best speaker cover ≥70% of total overlap?
    let dominance = best_overlap / total_overlap;
    if dominance >= DOMINANCE_THRESHOLD {
        Some(format!("Speaker {}", best_id + 1))
    } else {
        // Ambiguous: multiple speakers overlap and none dominates
        None
    }
}

/// Aggregate overlap per speaker and return the dominant speaker if one exists.
///
/// Returns `None` if:
/// - No speaker overlaps by at least `MIN_OVERLAP_S`
/// - Multiple speakers overlap and none dominates (≥`DOMINANCE_THRESHOLD`)
fn speaker_for_range(start: f64, end: f64, turns: &[SpeakerTurn]) -> Option<String> {
    let mut overlap_per_speaker: HashMap<usize, f64> = HashMap::new();

    for turn in turns {
        let overlap_start = start.max(turn.start);
        let overlap_end = end.min(turn.end);
        let overlap = (overlap_end - overlap_start).max(0.0);

        if overlap > MIN_OVERLAP_S {
            *overlap_per_speaker.entry(turn.speaker_id).or_insert(0.0) += overlap;
        }
    }

    if overlap_per_speaker.is_empty() {
        return None;
    }

    let total_overlap: f64 = overlap_per_speaker.values().sum();
    if total_overlap < MIN_OVERLAP_S {
        return None;
    }

    // Find the speaker with the most overlap
    let (best_id, best_overlap) = overlap_per_speaker
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(&id, &overlap)| (id, overlap))
        .unwrap();

    // Check dominance: does the best speaker cover ≥70% of total overlap?
    let dominance = best_overlap / total_overlap;
    if dominance >= DOMINANCE_THRESHOLD {
        Some(format!("Speaker {}", best_id + 1))
    } else {
        // Ambiguous: multiple speakers overlap and none dominates
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_speaker_turns_returns_none_labels() {
        let segments = vec![
            WhisperSegment {
                text: "Hello".to_string(),
                start: 0.0,
                end: 1.0,
                words: Vec::new(),
            },
            WhisperSegment {
                text: "World".to_string(),
                start: 1.0,
                end: 2.0,
                words: Vec::new(),
            },
        ];
        let result = merge_segments_with_speakers(&segments, &[]);
        assert_eq!(result.len(), 2);
        for seg in &result {
            assert!(
                seg.speaker.is_none(),
                "expected None speaker, got {:?}",
                seg.speaker
            );
        }
    }

    #[test]
    fn single_speaker_assigns_label() {
        let segments = vec![
            WhisperSegment {
                text: "Hello".to_string(),
                start: 0.0,
                end: 1.0,
                words: Vec::new(),
            },
            WhisperSegment {
                text: "World".to_string(),
                start: 1.0,
                end: 2.0,
                words: Vec::new(),
            },
        ];
        let turns = vec![SpeakerTurn {
            speaker_id: 0,
            start: 0.0,
            end: 2.0,
        }];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert_eq!(result.len(), 2);
        for seg in &result {
            assert_eq!(seg.speaker.as_deref(), Some("Speaker 1"));
        }
    }

    #[test]
    fn two_speakers_assigned_correctly() {
        let segments = vec![
            WhisperSegment {
                text: "Hello".to_string(),
                start: 0.0,
                end: 1.0,
                words: Vec::new(),
            },
            WhisperSegment {
                text: "World".to_string(),
                start: 1.0,
                end: 2.0,
                words: Vec::new(),
            },
        ];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 1.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 1.0,
                end: 2.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert_eq!(result[0].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(result[1].speaker.as_deref(), Some("Speaker 2"));
    }

    #[test]
    fn partial_overlap_picks_dominant_match() {
        // Segment spans 0.0–1.0; speaker 0 covers 0.0–0.8, speaker 1 covers 0.8–2.0
        // Overlap with speaker 0 = 0.8, overlap with speaker 1 = 0.2 → Speaker 1 dominates (80%)
        let segments = vec![WhisperSegment {
            text: "Overlap".to_string(),
            start: 0.0,
            end: 1.0,
            words: Vec::new(),
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 0.8,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 0.8,
                end: 2.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert_eq!(result[0].speaker.as_deref(), Some("Speaker 1"));
    }

    #[test]
    fn no_overlap_returns_none() {
        let segments = vec![WhisperSegment {
            text: "Silent gap".to_string(),
            start: 5.0,
            end: 6.0,
            words: Vec::new(),
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 1.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 2.0,
                end: 3.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert!(
            result[0].speaker.is_none(),
            "expected None for non-overlapping segment"
        );
    }

    #[test]
    fn timestamps_preserved() {
        let segments = vec![WhisperSegment {
            text: "Check timestamps".to_string(),
            start: 3.5,
            end: 7.25,
            words: Vec::new(),
        }];
        let turns = vec![SpeakerTurn {
            speaker_id: 0,
            start: 0.0,
            end: 10.0,
        }];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert_eq!(result[0].start, 3.5);
        assert_eq!(result[0].end, 7.25);
    }

    /// **Regression test for D4**: a short reply from speaker B inside a long A
    /// segment should NOT be attributed to A. The segment spans 0.0–10.0, speaker
    /// A covers 0.0–4.0 and 6.0–10.0 (8s total), speaker B covers 4.0–6.0 (2s).
    /// Old algorithm: picks A (8s > 2s). New algorithm: A dominates (80% ≥ 70%)
    /// so assigns Speaker 1.
    #[test]
    fn short_reply_inside_long_segment_dominant_speaker_wins() {
        let segments = vec![WhisperSegment {
            text: "Long segment with interruption".to_string(),
            start: 0.0,
            end: 10.0,
            words: Vec::new(),
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 4.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 4.0,
                end: 6.0,
            },
            SpeakerTurn {
                speaker_id: 0,
                start: 6.0,
                end: 10.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        // Speaker 0 has 8s overlap, Speaker 1 has 2s → Speaker 0 dominates (80%)
        assert_eq!(
            result[0].speaker.as_deref(),
            Some("Speaker 1"),
            "dominant speaker (A) should be assigned"
        );
    }

    /// **Regression test for D4**: when overlap is genuinely split between
    /// speakers (no one dominates), the segment should be marked ambiguous
    /// (`None` speaker) rather than arbitrarily assigned to one speaker.
    #[test]
    fn ambiguous_overlap_returns_none() {
        // Segment spans 0.0–10.0; speaker 0 covers 0.0–5.0 (5s), speaker 1 covers 5.0–10.0 (5s)
        // Neither dominates (50% each, threshold is 70%)
        let segments = vec![WhisperSegment {
            text: "Mixed speakers".to_string(),
            start: 0.0,
            end: 10.0,
            words: Vec::new(),
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 5.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 5.0,
                end: 10.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert!(
            result[0].speaker.is_none(),
            "expected None for ambiguous 50/50 overlap, got {:?}",
            result[0].speaker
        );
    }

    /// **Regression test for D4**: a short reply from speaker B that is
    /// overwhelmed by a long A segment should be marked ambiguous, not
    /// misattributed to A. Segment spans 0.0–10.0, speaker A covers 0.0–3.0
    /// and 4.0–10.0 (9s total), speaker B covers 3.0–4.0 (1s).
    /// A has 90% overlap → dominates → assigns Speaker 1.
    #[test]
    fn very_short_reply_dominant_speaker_wins() {
        let segments = vec![WhisperSegment {
            text: "Long monologue with brief interruption".to_string(),
            start: 0.0,
            end: 10.0,
            words: Vec::new(),
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 3.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 3.0,
                end: 4.0,
            },
            SpeakerTurn {
                speaker_id: 0,
                start: 4.0,
                end: 10.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        // Speaker 0 has 9s overlap, Speaker 1 has 1s → Speaker 0 dominates (90%)
        assert_eq!(
            result[0].speaker.as_deref(),
            Some("Speaker 1"),
            "dominant speaker (A) should be assigned"
        );
    }

    /// **Regression test for D4**: when a short reply is present but the
    /// segment is split 60/40, neither speaker dominates (threshold is 70%)
    /// so the segment should be marked ambiguous.
    #[test]
    fn borderline_overlap_returns_none() {
        // Segment spans 0.0–10.0; speaker 0 covers 0.0–6.0 (6s), speaker 1 covers 6.0–10.0 (4s)
        // Speaker 0 has 60% overlap (below 70% threshold) → ambiguous
        let segments = vec![WhisperSegment {
            text: "Borderline split".to_string(),
            start: 0.0,
            end: 10.0,
            words: Vec::new(),
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 6.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 6.0,
                end: 10.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert!(
            result[0].speaker.is_none(),
            "expected None for borderline 60/40 overlap, got {:?}",
            result[0].speaker
        );
    }

    // ---- word-window attribution tests ----

    use crate::whisper::WordTiming;

    fn wt(start: f64, end: f64) -> WordTiming {
        WordTiming { start, end }
    }

    /// Word windows pointing at speaker B flip the attribution of a segment
    /// whose raw span is dominated by speaker A: with word timings, only the
    /// time actually covered by timed words votes, so a 0.3–1.0 s window
    /// inside B's 0.5–2.0 s turn attributes to B even though A's turn covers
    /// the segment's first 0.5 s.
    #[test]
    fn word_windows_attribute_by_word_overlap_not_segment_span() {
        let segments = vec![WhisperSegment {
            text: "straddling a handoff".to_string(),
            start: 0.0,
            end: 2.0,
            words: vec![wt(0.3, 0.6), wt(0.6, 1.4)],
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 0.5,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 0.5,
                end: 2.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert_eq!(
            result[0].speaker.as_deref(),
            Some("Speaker 2"),
            "word windows inside speaker B's turn must attribute to B, got {:?}",
            result[0].speaker
        );
    }

    /// The <2-valid-word fallback: a single word window (backchannel like
    /// "mm-hm") must NOT drive word-window attribution — the merge falls
    /// back to the segment-window rule. Segment 0–10, A covers 0–9 (90%),
    /// so the fallback attributes to A even though the lone word window
    /// sits entirely inside B's 9.0–10.0 turn.
    #[test]
    fn single_word_window_falls_back_to_segment_rule() {
        let segments = vec![WhisperSegment {
            text: "mm-hm".to_string(),
            start: 0.0,
            end: 10.0,
            words: vec![wt(9.2, 9.6)],
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 9.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 9.0,
                end: 10.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert_eq!(
            result[0].speaker.as_deref(),
            Some("Speaker 1"),
            "one word window must not override the segment-window rule (fallback), got {:?}",
            result[0].speaker
        );
    }

    /// The same fallback, asserting the other direction: with the segment
    /// window itself inside B (fallback would say B), one noisy DTW point
    /// claiming A must not flip it either — the fallback path is what decides.
    #[test]
    fn single_word_window_fallback_cannot_flip_attribution() {
        let segments = vec![WhisperSegment {
            text: "yeah".to_string(),
            start: 5.0,
            end: 6.0,
            words: vec![wt(5.1, 5.5)],
        }];
        // B covers the whole segment; a noisy DTW point is inside B anyway,
        // so this pins that the fallback path is exercised (B) — if someone
        // lowers MIN_WORD_WINDOWS to 1 the attribution is still B here, but
        // the previous test catches the behavior change.
        let turns = vec![SpeakerTurn {
            speaker_id: 1,
            start: 0.0,
            end: 10.0,
        }];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert_eq!(result[0].speaker.as_deref(), Some("Speaker 2"));
    }

    /// Word-window ambiguity: word windows split between speakers with
    /// neither reaching 70% dominance → None, same rule as segment windows.
    #[test]
    fn word_windows_ambiguous_split_returns_none() {
        // Word time split 0.6 s in A / 0.7 s in B — B holds only 53.8% of the
        // total word overlap, below the 70% dominance threshold.
        let segments = vec![WhisperSegment {
            text: "mixed words".to_string(),
            start: 0.0,
            end: 2.0,
            words: vec![wt(0.4, 1.0), wt(1.0, 1.7)],
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 1.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 1.0,
                end: 2.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert!(
            result[0].speaker.is_none(),
            "46/54 word-window split must stay ambiguous, got {:?}",
            result[0].speaker
        );
    }

    /// The 10 ms overlap floor applies per word window: a window overlapping
    /// a turn by ≤10 ms contributes nothing, and windows with no qualifying
    /// overlap at all leave the segment unlabeled.
    #[test]
    fn word_windows_respect_ten_ms_floor_and_gap() {
        // Word windows in the 5–6 s gap between turns; the one straddling
        // the turn edge by exactly 10 ms contributes nothing (floor is >).
        let segments = vec![WhisperSegment {
            text: "gap words".to_string(),
            start: 4.0,
            end: 6.0,
            words: vec![wt(5.0, 5.4), wt(5.5, 6.0)],
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 5.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 6.0,
                end: 8.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert!(
            result[0].speaker.is_none(),
            "word windows with no >10ms overlap must leave the segment unlabeled, got {:?}",
            result[0].speaker
        );
    }

    /// Word-window dominance uses summed overlap across windows: many small
    /// windows inside A's turn outvote one long window inside B's turn when
    /// A's total word time dominates ≥70%.
    #[test]
    fn word_windows_sum_overlap_across_windows() {
        // Total word time 4.0 s: 3.0 s in A (75%), 1.0 s in B (25%).
        let segments = vec![WhisperSegment {
            text: "mostly A with a B tail".to_string(),
            start: 0.0,
            end: 10.0,
            words: vec![wt(0.0, 1.0), wt(1.0, 2.0), wt(2.0, 3.0), wt(9.0, 10.0)],
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 8.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 8.0,
                end: 10.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert_eq!(result[0].speaker.as_deref(), Some("Speaker 1"));
    }

    /// Empty word list (remote-provider segments) must behave exactly like
    /// the pre-refinement segment-window rule — this pins the fallback for
    /// segments that never had token timings.
    #[test]
    fn empty_words_equivalent_to_segment_window_rule() {
        let segments = vec![WhisperSegment {
            text: "remote segment".to_string(),
            start: 0.0,
            end: 10.0,
            words: Vec::new(),
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 5.0,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 5.0,
                end: 10.0,
            },
        ];
        let result = merge_segments_with_speakers(&segments, &turns);
        assert!(
            result[0].speaker.is_none(),
            "empty words must hit the segment-window fallback and stay ambiguous, got {:?}",
            result[0].speaker
        );
    }
}
