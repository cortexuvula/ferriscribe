//! Shared transcript attribution markers (single source of truth).
//!
//! [`SPEAKER_UNASSIGNED_MARKER`] is written by the transcript formatter
//! (`src-tauri` transcription pipeline) for spans the diarizer could not
//! attribute, parsed back by the TranscriptView text fallback, and explained
//! to the model by the SOAP / peer-discussion prompt builders. Keeping the
//! bytes in one crate-level constant means the writer, the reader, and the
//! prompt legend can never drift apart.
//!
//! The marker is deliberately NOT mis-parseable as a real speaker label:
//! TranscriptView's text parsers accept only `[Speaker N]` (decimal digits)
//! and `Speaker N:` — `unassigned` matches neither, so the marker can never
//! collide with a real label, and a real speaker can never be numbered
//! "unassigned".

/// Span-level marker for text the diarizer could not attribute to any
/// speaker. Replaces the old behavior of silently inheriting the previous
/// speaker's label (fabricated attribution reaching stored text, Copy, and
/// generated clinical documents).
pub const SPEAKER_UNASSIGNED_MARKER: &str = "[Speaker unassigned]";

/// True when the text contains the unassigned marker. Prompt builders use
/// this to decide whether the attribution legend is needed — a transcript
/// without markers carries no unattributed spans and gets no legend.
pub fn contains_unassigned_marker(text: &str) -> bool {
    text.contains(SPEAKER_UNASSIGNED_MARKER)
}

/// The legend prompt builders append after a transcript that carries the
/// unassigned marker, so the model reads the marker as uncertainty instead
/// of "resolving" it to the nearest speaker (fabricated attribution in a
/// generated clinical document — review contract line B,
/// docs/reviews/transcript-render-2026-09-13).
pub const ATTRIBUTION_LEGEND: &str = "Note on speaker labels: `[Speaker unassigned]` marks a passage that automatic speaker labelling could not attribute to any speaker. Do NOT assign these passages to a nearby speaker. If a passage matters clinically, describe it without speaker attribution and flag the attribution as unverified.";

/// Assemble the prompt-side transcript block: datetime prefix (shared
/// convention of both prompt builders) plus the attribution legend when —
/// and only when — the sanitized transcript carries the unassigned marker.
/// Single-sourced here so the SOAP and peer-discussion builders cannot
/// drift apart (the 2026-09-04 sanitizer drift started exactly that way).
pub fn datetime_prefix_and_legend(clean_transcript: &str) -> String {
    use chrono::Local;
    let now = Local::now();
    let time_date = now.format("Time %H:%M Date %d %b %Y").to_string();
    if contains_unassigned_marker(clean_transcript) {
        format!("{time_date}\n\n{clean_transcript}\n\n{ATTRIBUTION_LEGEND}")
    } else {
        format!("{time_date}\n\n{clean_transcript}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legend_added_only_when_transcript_carries_the_marker() {
        let marked = datetime_prefix_and_legend(
            "00:00:01,000 --> 00:00:02,000 [Speaker 1] \nSynthetic alpha.\n\n00:00:03,000 --> 00:00:04,000 [Speaker unassigned] \nSynthetic gap.",
        );
        assert!(marked.contains(SPEAKER_UNASSIGNED_MARKER));
        assert!(marked.contains(ATTRIBUTION_LEGEND));
        // The legend must tell the model NOT to attribute: its core rule.
        assert!(ATTRIBUTION_LEGEND.contains("Do NOT assign"));

        let unmarked = datetime_prefix_and_legend(
            "00:00:01,000 --> 00:00:02,000 [Speaker 1] \nSynthetic only.",
        );
        assert!(
            !unmarked.contains(ATTRIBUTION_LEGEND),
            "no marker, no legend — a marker-free transcript has no unattributed spans"
        );
    }

    #[test]
    fn marker_is_not_a_valid_speaker_number() {
        // The TranscriptView parsers accept only decimal digits after
        // "Speaker ". If someone ever changes the marker to a numeric form,
        // this pin fails before it can collide with real labels.
        let inner = SPEAKER_UNASSIGNED_MARKER
            .trim_start_matches('[')
            .trim_end_matches(']');
        assert_eq!(inner, "Speaker unassigned");
        let suffix = inner.strip_prefix("Speaker ").expect("Speaker prefix");
        assert!(
            suffix.parse::<u32>().is_err(),
            "marker suffix must never be numeric (got {suffix})"
        );
    }

    #[test]
    fn contains_detector_matches_only_the_marker() {
        assert!(contains_unassigned_marker(
            "00:00:01,000 --> 00:00:02,000 [Speaker unassigned]\nSome words."
        ));
        assert!(!contains_unassigned_marker(
            "00:00:01,000 --> 00:00:02,000 [Speaker 1]\nSome words."
        ));
        assert!(!contains_unassigned_marker(""));
    }
}
