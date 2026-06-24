//! Speech-to-text types — audio data, transcription config, and results.

use serde::{Deserialize, Serialize};

/// Raw PCM audio data ready for transcription.
///
/// Samples are 32-bit floats in the range `[-1.0, 1.0]`. The
/// [`duration_seconds`](AudioData::duration_seconds) method computes
/// duration from sample count, rate, and channel count.
#[derive(Debug, Clone)]
pub struct AudioData {
    /// Interleaved PCM samples (32-bit float, normalized).
    pub samples: Vec<f32>,
    /// Sample rate in Hz (e.g. 16000, 44100).
    pub sample_rate: u32,
    /// Number of audio channels (1 = mono, 2 = stereo).
    pub channels: u16,
}

impl AudioData {
    /// Returns the duration of the audio in seconds.
    ///
    /// Computed as `samples.len() / (sample_rate * channels)`. Returns
    /// `0.0` if `sample_rate` or `channels` is zero.
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 || self.channels == 0 {
            return 0.0;
        }
        self.samples.len() as f64 / (self.sample_rate as f64 * self.channels as f64)
    }
}

/// Configuration for a speech-to-text request.
///
/// Passed to [`SttProvider::transcribe`](crate::traits::SttProvider::transcribe)
/// alongside the audio data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttConfig {
    /// BCP-47 language hint (e.g. `"en-US"`). `None` for auto-detect.
    pub language: Option<String>,
    /// Whether to enable speaker diarization.
    pub diarize: bool,
    /// Expected number of speakers (hint for diarization).
    pub num_speakers: Option<u32>,
    /// Model name override (provider default if `None`).
    pub model: Option<String>,
    /// Enable smart formatting (punctuation, capitalization).
    pub smart_formatting: bool,
    /// Enable profanity filtering.
    pub profanity_filter: bool,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            language: None,
            diarize: false,
            num_speakers: None,
            model: None,
            smart_formatting: true,
            profanity_filter: false,
        }
    }
}

/// A completed transcription result.
///
/// Returned by [`SttProvider::transcribe`](crate::traits::SttProvider::transcribe).
/// Contains the full text, timed segments (with optional speaker labels),
/// and provider metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// The full transcribed text.
    pub text: String,
    /// Timed segments with optional speaker attribution.
    pub segments: Vec<TranscriptSegment>,
    /// Detected or configured language.
    pub language: Option<String>,
    /// Duration of the source audio.
    pub duration_seconds: Option<f64>,
    /// Which provider produced this transcript.
    pub provider: String,
    /// Provider-specific metadata (freeform JSON).
    pub metadata: serde_json::Value,
}

/// A timed segment within a transcript, optionally attributed to a speaker.
///
/// Used by the UI to display transcripts with time-aligned highlighting
/// and speaker labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// The segment text.
    pub text: String,
    /// Start time in seconds from the beginning of the audio.
    pub start: f64,
    /// End time in seconds from the beginning of the audio.
    pub end: f64,
    /// Speaker label (e.g. `"Speaker 1"`) if diarization was enabled.
    pub speaker: Option<String>,
    /// Confidence score in `[0.0, 1.0]` if the provider reports it.
    pub confidence: Option<f32>,
}

/// A streaming chunk from a real-time transcription session.
///
/// Yielded by [`SttProvider::transcribe_stream`](crate::traits::SttProvider::transcribe_stream).
/// Chunks with `is_final = true` represent the provider's final decision
/// for that segment; earlier chunks are interim hypotheses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptChunk {
    /// The chunk text.
    pub text: String,
    /// Whether this is the final version of this segment.
    pub is_final: bool,
    /// Speaker label if diarization is active.
    pub speaker: Option<String>,
}

/// A stream of raw PCM audio frames.
///
/// Used by [`SttProvider::transcribe_stream`](crate::traits::SttProvider::transcribe_stream)
/// to receive live audio from the audio capture subsystem.
pub type AudioStream = tokio::sync::mpsc::Receiver<Vec<f32>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_seconds_calculated_correctly() {
        let audio = AudioData {
            samples: vec![0.0f32; 44100],
            sample_rate: 44100,
            channels: 1,
        };
        let duration = audio.duration_seconds();
        assert!(
            (duration - 1.0).abs() < 1e-6,
            "expected ~1.0s, got {duration}"
        );
    }

    #[test]
    fn duration_seconds_stereo() {
        let audio = AudioData {
            samples: vec![0.0f32; 88200],
            sample_rate: 44100,
            channels: 2,
        };
        let duration = audio.duration_seconds();
        assert!(
            (duration - 1.0).abs() < 1e-6,
            "expected ~1.0s, got {duration}"
        );
    }

    #[test]
    fn duration_zero_guard() {
        let audio = AudioData {
            samples: vec![],
            sample_rate: 0,
            channels: 0,
        };
        assert_eq!(audio.duration_seconds(), 0.0);
    }

    #[test]
    fn stt_config_defaults() {
        let config = SttConfig::default();
        assert!(config.language.is_none());
        assert!(!config.diarize);
        assert!(config.num_speakers.is_none());
        assert!(config.model.is_none());
        assert!(config.smart_formatting);
        assert!(!config.profanity_filter);
    }
}
