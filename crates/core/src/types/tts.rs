//! Text-to-speech types — synthesis config and voice metadata.

use serde::{Deserialize, Serialize};

/// Configuration for a text-to-speech synthesis request.
///
/// Passed to [`TtsProvider::synthesize`](crate::traits::TtsProvider::synthesize).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    /// Voice identifier (from [`VoiceInfo::id`]).
    pub voice: Option<String>,
    /// BCP-47 language code (e.g. `"en-US"`).
    pub language: Option<String>,
    /// Playback speed multiplier (1.0 = normal).
    pub speed: f32,
    /// Volume multiplier (1.0 = normal).
    pub volume: f32,
    /// Model name override (provider default if `None`).
    pub model: Option<String>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            voice: None,
            language: None,
            speed: 1.0,
            volume: 1.0,
            model: None,
        }
    }
}

/// Metadata about an available TTS voice.
///
/// Returned by [`TtsProvider::available_voices`](crate::traits::TtsProvider::available_voices).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceInfo {
    /// Provider-specific voice identifier.
    pub id: String,
    /// Human-readable voice name.
    pub name: String,
    /// BCP-47 language the voice speaks.
    pub language: Option<String>,
    /// Voice gender label (provider-defined).
    pub gender: Option<String>,
    /// URL to a short audio preview.
    pub preview_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_config_defaults() {
        let config = TtsConfig::default();
        assert!(config.voice.is_none());
        assert!(config.language.is_none());
        assert!((config.speed - 1.0).abs() < f32::EPSILON);
        assert!((config.volume - 1.0).abs() < f32::EPSILON);
        assert!(config.model.is_none());
    }

    #[test]
    fn tts_config_round_trip() {
        let config = TtsConfig {
            voice: Some("nova".into()),
            language: Some("en-US".into()),
            speed: 1.25,
            volume: 0.8,
            model: Some("eleven_monolingual_v1".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: TtsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.voice.as_deref(), Some("nova"));
        assert!((back.speed - 1.25).abs() < f32::EPSILON);
    }
}
