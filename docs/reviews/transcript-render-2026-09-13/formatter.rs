mod medical_core { pub mod types { pub mod stt {
pub struct Segment { pub speaker:Option<String>, pub text:String,pub start:f64,pub end:f64 }
pub struct Transcript {pub text:String,pub segments:Vec<Segment>}
}}}
fn format_transcript_with_speakers(transcript: &medical_core::types::stt::Transcript) -> String {
    let any_speakers = transcript.segments.iter().any(|s| s.speaker.is_some());
    if !any_speakers {
        return transcript.text.clone();
    }

    let mut result = String::new();
    let mut last_speaker: Option<&str> = None;

    for seg in &transcript.segments {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }
        // Fold unlabeled segments into the last known speaker so no text
        // is dropped (diarization may not cover every whisper segment).
        let speaker = seg.speaker.as_deref().or(last_speaker);
        if speaker.is_some() {
            last_speaker = speaker;
        }

        if !result.is_empty() {
            result.push_str("\n\n");
        }

        // SRT-style timestamp: HH:MM:SS,mmm --> HH:MM:SS,mmm
        result.push_str(&format_srt_timestamp(seg.start, seg.end));
        result.push(' ');

        // Bracketed speaker label: [Speaker 1]
        //
        // NUMBERING CONVENTION (B3): "Speaker N" is 1-based everywhere —
        // merge.rs already emits `format!("Speaker {}", id + 1)`, the rich
        // view badges render the metadata label verbatim, and this stored /
        // copied text must match. The old `n - 1` here produced a 0-based
        // off-by-one against every other surface.
        // Labels are neutral speaker ordinals with NO clinical role
        // inference — the diarizer cannot know who is the doctor and who
        // is the patient, and must never label as such (see
        // docs/design/speaker-labelling.md).
        if let Some(label) = speaker {
            result.push_str(&format!("[{label}] "));
        }

        result.push('\n');
        result.push_str(text);
    }

    result
}

/// Format a start/end timestamp pair (in seconds) as an SRT-style range:
/// `00:00:01,340 --> 00:00:03,750`
fn format_srt_timestamp(start: f64, end: f64) -> String {
    format!("{} --> {}", format_srt_time(start), format_srt_time(end))
}

/// Format a single timestamp (in seconds) as `HH:MM:SS,mmm`.
fn format_srt_time(t: f64) -> String {
    let total_ms = (t * 1000.0).round() as u64;
    let hours = total_ms / 3_600_000;
    let minutes = (total_ms % 3_600_000) / 60_000;
    let seconds = (total_ms % 60_000) / 1_000;
    let millis = total_ms % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

fn main(){use medical_core::types::stt::*; let t=Transcript{text:"Alpha Gap Beta".into(),segments:vec![
Segment{speaker:Some("Speaker 1".into()),text:"Alpha".into(),start:0.,end:1.},
Segment{speaker:None,text:"Gap".into(),start:1.,end:2.},
Segment{speaker:Some("Speaker 2".into()),text:"Beta".into(),start:2.,end:3.}]}; println!("{}",format_transcript_with_speakers(&t));}