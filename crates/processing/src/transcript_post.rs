//! Post-decode transcript transforms shared by the app pipeline and the
//! local evaluation harness.
//!
//! These functions were moved verbatim from
//! `src-tauri/src/commands/transcription/helpers.rs` so the local-eval
//! harness (stage B/C of the 3-stage comparison) can apply the EXACT
//! production transforms to raw decoder output without duplicating logic
//! (a copy would drift; this is the single source of truth). The app
//! re-exports them from the same names, so behavior is unchanged.
//!
//! Stage model (see docs/eval/local-eval-accuracy-diarization.md):
//! - Stage A: raw whisper decoder output (segments from `whisper.rs`)
//! - Stage B: saved/stored transcript (after repetition filters)
//! - Stage C: displayed/copied text (after speaker formatting)

use medical_core::types::stt::Transcript;

/// Detect the repeated-short-phrase pattern Whisper produces when fed silence
/// (classic: "Thank you. Thank you. Thank you. ...").
///
/// Conservative by design: requires at least 3 sentence-like segments that are
/// all identical (case-insensitive, whitespace-normalised) and short. Callers
/// should gate this on a known-silent source so legitimate short transcripts
/// aren't rejected.
pub fn is_repeated_phrase_hallucination(text: &str) -> bool {
    let segments: Vec<String> = text
        .split(['.', '!', '?', '\n'])
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() < 3 {
        return false;
    }
    let first = &segments[0];
    // Long segments are almost never hallucinations — real speech is varied.
    if first.chars().count() > 80 {
        return false;
    }
    segments.iter().all(|s| s == first)
}

/// Collapse whisper.cpp repetition loops within a transcript segment.
///
/// Whisper sometimes produces loops like "okay okay okay all right all right
/// all right all right" within a single segment — a decoding pathology, not
/// real speech. This function detects when the segment's tokens repeat a
/// short sub-sequence (1-4 words) 3+ times and collapses the repetition to
/// a single instance.
///
/// Conservative: only collapses when the *entire* segment is dominated by
/// the repeated pattern (the repeated block covers > 60% of the segment).
/// A real sentence that happens to repeat a word ("the patient said the
/// patient said...") won't be touched because the surrounding words break
/// the pattern.
pub fn collapse_repetition_loops(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 6 {
        return text.to_string(); // too short to have a meaningful loop
    }

    // Try pattern lengths 1..=4 words.
    for pattern_len in 1..=4usize {
        if let Some(collapsed) = try_collapse_pattern(&tokens, pattern_len) {
            return collapsed;
        }
    }
    text.to_string()
}

/// If the tokens consist of a repeating `pattern_len`-word block (3+ times),
/// collapse to a single instance of that block.
fn try_collapse_pattern(tokens: &[&str], pattern_len: usize) -> Option<String> {
    if tokens.len() < pattern_len * 3 {
        return None;
    }
    let pattern: Vec<&str> = tokens[..pattern_len].to_vec();
    let pattern_lower: Vec<String> = pattern.iter().map(|t| t.to_lowercase()).collect();

    // Count how many consecutive repetitions of the pattern exist from the start.
    let mut reps = 1;
    let mut i = pattern_len;
    while i + pattern_len <= tokens.len() {
        let chunk_lower: Vec<String> = tokens[i..i + pattern_len]
            .iter()
            .map(|t| t.to_lowercase())
            .collect();
        if chunk_lower == pattern_lower {
            reps += 1;
            i += pattern_len;
        } else {
            break;
        }
    }

    if reps >= 3 && i == tokens.len() {
        // The entire segment is the pattern repeated — collapse to one.
        return Some(pattern.join(" "));
    }

    if reps >= 3 {
        // The repeated block covers the start; check if the remaining tokens
        // are short enough that the repetition dominates (> 60% of segment).
        let repeated_count = reps * pattern_len;
        if repeated_count as f64 / tokens.len() as f64 > 0.6 {
            let remaining = tokens[i..].join(" ");
            return Some(format!("{} {}", pattern.join(" "), remaining));
        }
    }

    None
}

/// Apply repetition-loop collapse to each segment of a transcript, returning
/// a new transcript text. Called after whisper transcription but before
/// formatting/storage.
pub fn filter_segment_repetitions(transcript: &mut Transcript) {
    let mut changed = false;
    for seg in &mut transcript.segments {
        let original = &seg.text;
        let collapsed = collapse_repetition_loops(original);
        if collapsed != *original {
            tracing::warn!(
                original_len = original.len(),
                collapsed_len = collapsed.len(),
                "collapsed whisper repetition loop in segment"
            );
            seg.text = collapsed;
            changed = true;
        }
    }
    // Rebuild transcript.text if any segment was modified, so downstream
    // code (hallucination guard, formatting) sees consistent content.
    if changed {
        transcript.text = transcript
            .segments
            .iter()
            .map(|s| s.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
    }
}

/// Drop runs of 3+ consecutive identical segments — whisper.cpp
/// repetition hallucinations. These occur when whisper's decoder gets
/// stuck in a high-probability loop during low-energy/noisy audio.
///
/// The original `MAX_WORDS = 5` limit was too restrictive — the real-world
/// patterns include long phrases ("I don't know if I was going to go
/// through it" = 10 words, "I see a counsellor once every two weeks"
/// = 9 words). Any segment text repeated 3+ times consecutively is a
/// hallucination, regardless of length.
///
/// After dropping, `transcript.text` is rebuilt from the surviving
/// segments so downstream code (hallucination guard, formatting) sees
/// accurate content.
pub fn filter_cross_segment_repetitions(transcript: &mut Transcript) {
    const MIN_RUN_LEN: usize = 3;

    let segments = &transcript.segments;
    if segments.len() < MIN_RUN_LEN {
        return;
    }

    // Identify runs of consecutive identical segments.
    let mut drop_indices: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < segments.len() {
        let current_key = segments[i].text.trim().to_lowercase();
        if current_key.is_empty() {
            i += 1;
            continue;
        }
        // Count how many consecutive segments match (case-insensitive, trimmed).
        let run_end = i
            + 1
            + segments[i + 1..]
                .iter()
                .take_while(|s| s.text.trim().to_lowercase() == current_key)
                .count();
        let run_len = run_end - i;
        if run_len >= MIN_RUN_LEN {
            // PHI guard: log the length only — segment text is transcript
            // content and must never reach the persistent log.
            tracing::warn!(
                text_len = current_key.len(),
                run_len,
                "dropping cross-segment repetition (whisper hallucination)"
            );
            drop_indices.extend(i..run_end);
        }
        i = run_end;
    }

    if drop_indices.is_empty() {
        return;
    }

    // Retain only segments not in the drop set.
    let drop_set: std::collections::HashSet<usize> = drop_indices.into_iter().collect();
    transcript.segments = transcript
        .segments
        .iter()
        .enumerate()
        .filter(|(idx, _)| !drop_set.contains(idx))
        .map(|(_, seg)| seg.clone())
        .collect();

    // Rebuild the joined text so downstream guards see the filtered result.
    transcript.text = transcript
        .segments
        .iter()
        .map(|s| s.text.trim())
        .collect::<Vec<_>>()
        .join(" ");
}

/// Build the speaker-attributed stored/copied text for a transcript.
///
/// Moved verbatim from
/// `src-tauri/src/commands/transcription/inner.rs::format_transcript_with_speakers`
/// so the harness's stage C matches production byte-for-byte.
///
/// - No speaker labels at all → the flat joined text.
/// - SRT-style `HH:MM:SS,mmm --> HH:MM:SS,mmm` timestamp, then `[Speaker N]`
///   (1-based, matching merge.rs) or the shared `[Speaker unassigned]` marker
///   for spans diarization could not attribute — an unlabeled span must NEVER
///   inherit the previous speaker's label (DATA INTEGRITY: the stored text is
///   pasted into clinical notes and fed to the SOAP prompt verbatim).
pub fn format_transcript_with_speakers(transcript: &Transcript) -> String {
    let any_speakers = transcript.segments.iter().any(|s| s.speaker.is_some());
    if !any_speakers {
        return transcript.text.clone();
    }

    let mut result = String::new();

    for seg in &transcript.segments {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }
        let speaker = seg.speaker.as_deref();

        if !result.is_empty() {
            result.push_str("\n\n");
        }

        // SRT-style timestamp: HH:MM:SS,mmm --> HH:MM:SS,mmm
        result.push_str(&format_srt_timestamp(seg.start, seg.end));
        result.push(' ');

        match speaker {
            Some(label) => result.push_str(&format!("[{label}] ")),
            None => {
                result.push_str(crate::transcript_markers::SPEAKER_UNASSIGNED_MARKER);
                result.push(' ');
            }
        }

        result.push('\n');
        result.push_str(text);
    }

    result
}

/// Format a start/end timestamp pair (in seconds) as an SRT-style range:
/// `00:00:01,340 --> 00:00:03,750`
pub fn format_srt_timestamp(start: f64, end: f64) -> String {
    format!("{} --> {}", format_srt_time(start), format_srt_time(end))
}

/// Format a single timestamp (in seconds) as `HH:MM:SS,mmm`.
pub fn format_srt_time(t: f64) -> String {
    let total_ms = (t * 1000.0).round() as u64;
    let hours = total_ms / 3_600_000;
    let minutes = (total_ms % 3_600_000) / 60_000;
    let seconds = (total_ms % 60_000) / 1_000;
    let millis = total_ms % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, start: f64) -> TranscriptSegment {
        TranscriptSegment {
            text: text.to_string(),
            start,
            end: start + 2.0,
            speaker: None,
            confidence: None,
        }
    }

    fn mk_transcript(segments: Vec<TranscriptSegment>) -> Transcript {
        let text = segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Transcript {
            text,
            segments,
            language: None,
            duration_seconds: None,
            provider: "test".to_string(),
            metadata: serde_json::json!({}),
        }
    }

    // ---- is_repeated_phrase_hallucination ----

    #[test]
    fn detects_thank_you_hallucination() {
        assert!(is_repeated_phrase_hallucination(
            "Thank you. Thank you. Thank you. Thank you."
        ));
    }

    #[test]
    fn detects_case_insensitive_repetition() {
        assert!(is_repeated_phrase_hallucination("Okay. okay. OKAY. Okay."));
    }

    #[test]
    fn rejects_varied_speech() {
        assert!(!is_repeated_phrase_hallucination(
            "Hello there. How are you today? I am fine."
        ));
    }

    #[test]
    fn rejects_short_transcript() {
        assert!(!is_repeated_phrase_hallucination("Thank you. Thank you."));
    }

    #[test]
    fn rejects_empty_transcript() {
        assert!(!is_repeated_phrase_hallucination(""));
    }

    #[test]
    fn rejects_long_repeated_segments() {
        let long = "word ".repeat(30);
        let text = format!("{long}. {long}. {long}.");
        assert!(!is_repeated_phrase_hallucination(&text));
    }

    // ---- collapse_repetition_loops / filter_segment_repetitions ----

    #[test]
    fn collapse_single_word_loop() {
        let collapsed = collapse_repetition_loops("okay okay okay okay okay okay");
        assert_eq!(collapsed, "okay");
    }

    #[test]
    fn collapse_multi_word_loop() {
        let collapsed = collapse_repetition_loops(
            "all right all right all right all right all right all right",
        );
        assert_eq!(collapsed, "all right");
    }

    #[test]
    fn keeps_short_text() {
        assert_eq!(collapse_repetition_loops("okay okay"), "okay okay");
    }

    #[test]
    fn keeps_normal_text() {
        let text = "the patient reports chest pain on exertion";
        assert_eq!(collapse_repetition_loops(text), text);
    }

    // ---- filter_cross_segment_repetitions ----

    #[test]
    fn filter_cross_segment_repetitions_strips_trailing_so_so_so() {
        let mut t = mk_transcript(vec![
            seg("first sentence here", 0.0),
            seg("so", 2.0),
            seg("so", 4.0),
            seg("so", 6.0),
        ]);
        filter_cross_segment_repetitions(&mut t);
        assert_eq!(t.segments.len(), 1);
        assert_eq!(t.segments[0].text, "first sentence here");
        assert_eq!(t.text, "first sentence here");
    }

    #[test]
    fn filter_cross_segment_repetitions_keeps_distinct_segments() {
        let mut t = mk_transcript(vec![
            seg("first", 0.0),
            seg("second", 2.0),
            seg("third", 4.0),
        ]);
        filter_cross_segment_repetitions(&mut t);
        assert_eq!(t.segments.len(), 3);
    }

    #[test]
    fn filter_cross_segment_repetitions_keeps_short_runs() {
        let mut t = mk_transcript(vec![seg("so", 0.0), seg("so", 2.0)]);
        filter_cross_segment_repetitions(&mut t);
        assert_eq!(t.segments.len(), 2);
    }

    #[test]
    fn filter_cross_segment_repetitions_drops_long_repeated_phrases() {
        let phrase = "I see a counsellor once every two weeks";
        let mut t = mk_transcript(vec![
            seg("intro", 0.0),
            seg(phrase, 2.0),
            seg(phrase, 4.0),
            seg(phrase, 6.0),
            seg("outro", 8.0),
        ]);
        filter_cross_segment_repetitions(&mut t);
        assert_eq!(t.segments.len(), 2);
        assert_eq!(t.segments[0].text, "intro");
        assert_eq!(t.segments[1].text, "outro");
    }

    // ---- format_transcript_with_speakers ----

    #[test]
    fn format_without_speakers_returns_flat_text() {
        let t = mk_transcript(vec![seg("hello world", 0.0)]);
        assert_eq!(format_transcript_with_speakers(&t), "hello world");
    }

    #[test]
    fn format_with_speakers_uses_srt_timestamps_and_one_based_labels() {
        let mut s1 = seg("hello", 1.34);
        s1.speaker = Some("Speaker 1".to_string());
        let t = mk_transcript(vec![s1]);
        let out = format_transcript_with_speakers(&t);
        assert!(
            out.contains("00:00:01,340 --> 00:00:03,340 [Speaker 1]"),
            "{out}"
        );
        assert!(out.contains("\nhello"));
    }

    #[test]
    fn format_unlabeled_span_uses_shared_marker_not_inherited_label() {
        let mut s1 = seg("first", 0.0);
        s1.speaker = Some("Speaker 1".to_string());
        let s2 = seg("second", 2.0); // no label — must NOT inherit Speaker 1
        let t = mk_transcript(vec![s1, s2]);
        let out = format_transcript_with_speakers(&t);
        assert!(!out.contains("[Speaker 1]\nsecond"), "{out}");
        assert!(
            out.contains(crate::transcript_markers::SPEAKER_UNASSIGNED_MARKER),
            "{out}"
        );
    }

    // ---- srt time formatting ----

    #[test]
    fn srt_time_formats() {
        assert_eq!(format_srt_time(0.0), "00:00:00,000");
        assert_eq!(format_srt_time(1.34), "00:00:01,340");
        assert_eq!(format_srt_time(3661.5), "01:01:01,500");
    }
}
