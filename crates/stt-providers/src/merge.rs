//! Merge Whisper text segments with speaker diarization turns by timestamp overlap.
//!
//! The merge algorithm assigns each Whisper segment to the speaker turn that
//! covers the most of its time range. If no speaker turn overlaps by at least
//! 10ms, the segment is left unlabeled. Speaker labels are formatted as
//! `"Speaker N"` (1-based).

use medical_core::types::TranscriptSegment;

use crate::diarization::SpeakerTurn;
use crate::whisper::WhisperSegment;

/// Merge whisper text segments with speaker turns.
///
/// For each whisper segment, finds the speaker turn with the greatest
/// timestamp overlap and assigns that speaker's ID as "Speaker N".
/// If `speaker_turns` is empty, returns segments without speaker labels.
pub fn merge_segments_with_speakers(
    whisper_segments: &[WhisperSegment],
    speaker_turns: &[SpeakerTurn],
) -> Vec<TranscriptSegment> {
    whisper_segments
        .iter()
        .map(|ws| {
            let speaker = if speaker_turns.is_empty() {
                None
            } else {
                best_speaker_for_range(ws.start, ws.end, speaker_turns)
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

fn best_speaker_for_range(start: f64, end: f64, turns: &[SpeakerTurn]) -> Option<String> {
    let mut best_id: Option<usize> = None;
    let mut best_overlap: f64 = 0.0;
    for turn in turns {
        let overlap_start = start.max(turn.start);
        let overlap_end = end.min(turn.end);
        let overlap = (overlap_end - overlap_start).max(0.0);
        if overlap > best_overlap {
            best_overlap = overlap;
            best_id = Some(turn.speaker_id);
        }
    }
    if best_overlap > 0.01 {
        best_id.map(|id| format!("Speaker {}", id + 1))
    } else {
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
            },
            WhisperSegment {
                text: "World".to_string(),
                start: 1.0,
                end: 2.0,
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
            },
            WhisperSegment {
                text: "World".to_string(),
                start: 1.0,
                end: 2.0,
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
            },
            WhisperSegment {
                text: "World".to_string(),
                start: 1.0,
                end: 2.0,
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
    fn partial_overlap_picks_best_match() {
        // Segment spans 0.0–1.0; speaker 0 covers 0.0–0.7, speaker 1 covers 0.7–2.0
        // Overlap with speaker 0 = 0.7, overlap with speaker 1 = 0.3 → picks Speaker 1
        let segments = vec![WhisperSegment {
            text: "Overlap".to_string(),
            start: 0.0,
            end: 1.0,
        }];
        let turns = vec![
            SpeakerTurn {
                speaker_id: 0,
                start: 0.0,
                end: 0.7,
            },
            SpeakerTurn {
                speaker_id: 1,
                start: 0.7,
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
}
