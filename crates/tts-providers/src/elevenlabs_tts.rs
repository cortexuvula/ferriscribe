//! ElevenLabs cloud TTS provider.
//!
//! Synthesises speech by POSTing to the ElevenLabs REST API. Returns encoded
//! audio bytes (MP3) from [`synthesize`](ElevenLabsTtsProvider::synthesize).
//!
//! The provider requires an API key at construction time, which is baked into
//! the HTTP client's default headers (`xi-api-key`). A 60-second request
//! timeout is applied to all calls.

use async_trait::async_trait;
use medical_core::error::{AppError, AppResult};
use medical_core::traits::TtsProvider;
use medical_core::types::tts::{TtsConfig, VoiceInfo};
use reqwest::Client;
use serde::Serialize;

use crate::TtsError;

/// Cloud TTS provider backed by the ElevenLabs API.
///
/// Holds a pre-configured [`reqwest::Client`] with the API key set as a
/// default header. Cheap to clone (the client uses connection pooling
/// internally), though cloning is not expected in normal use.
#[derive(Debug)]
pub struct ElevenLabsTtsProvider {
    client: Client,
}

#[derive(Serialize)]
struct TtsRequest {
    text: String,
    model_id: String,
    voice_settings: VoiceSettings,
}

#[derive(Serialize)]
struct VoiceSettings {
    stability: f32,
    similarity_boost: f32,
    style: f32,
    use_speaker_boost: bool,
}

impl ElevenLabsTtsProvider {
    /// Create a new ElevenLabs TTS provider.
    ///
    /// The `api_key` is parsed into an HTTP header value and set as a default
    /// `xi-api-key` header on every request. Returns [`TtsError::InvalidHeader`]
    /// if the key contains characters that are not valid in an HTTP header
    /// (non-ASCII, newlines, control characters).
    ///
    /// # Errors
    ///
    /// - [`TtsError::InvalidHeader`] — `api_key` is not a valid header value.
    /// - [`TtsError::Http`] — the underlying `reqwest` client could not be built.
    pub fn new(api_key: &str) -> Result<Self, TtsError> {
        let api_key_header = api_key
            .parse()
            .map_err(|e| TtsError::InvalidHeader(format!("xi-api-key header: {e}")))?;
        let content_type_header = "application/json"
            .parse()
            .map_err(|e| TtsError::InvalidHeader(format!("Content-Type header: {e}")))?;

        let client = Client::builder()
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert("xi-api-key", api_key_header);
                h.insert("Content-Type", content_type_header);
                h
            })
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| TtsError::Http(format!("failed to build HTTP client: {e}")))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl TtsProvider for ElevenLabsTtsProvider {
    fn name(&self) -> &str {
        "elevenlabs"
    }

    /// Returns a hard-coded list of five popular English voices.
    ///
    /// This avoids an extra API round-trip. To use a voice not in this list,
    /// pass its ID via [`TtsConfig::voice`] — the ID is used directly in the
    /// API URL regardless of whether it appears here.
    async fn available_voices(&self) -> AppResult<Vec<VoiceInfo>> {
        Ok(vec![
            VoiceInfo {
                id: "21m00Tcm4TlvDq8ikWAM".into(),
                name: "Rachel".into(),
                language: Some("en".into()),
                gender: Some("female".into()),
                preview_url: None,
            },
            VoiceInfo {
                id: "AZnzlk1XvdvUeBnXmlld".into(),
                name: "Domi".into(),
                language: Some("en".into()),
                gender: Some("female".into()),
                preview_url: None,
            },
            VoiceInfo {
                id: "EXAVITQu4vr4xnSDxMaL".into(),
                name: "Bella".into(),
                language: Some("en".into()),
                gender: Some("female".into()),
                preview_url: None,
            },
            VoiceInfo {
                id: "ErXwobaYiN019PkySvjV".into(),
                name: "Antoni".into(),
                language: Some("en".into()),
                gender: Some("male".into()),
                preview_url: None,
            },
            VoiceInfo {
                id: "VR6AewLTigWG4xSOukaG".into(),
                name: "Arnold".into(),
                language: Some("en".into()),
                gender: Some("male".into()),
                preview_url: None,
            },
        ])
    }

    /// Synthesize text to audio via the ElevenLabs API.
    ///
    /// POSTs to `https://api.elevenlabs.io/v1/text-to-speech/{voice_id}` and
    /// returns the response body as a byte vector (typically MP3-encoded).
    ///
    /// Defaults when `config` fields are `None`:
    /// - **voice:** `"21m00Tcm4TlvDq8ikWAM"` (Rachel)
    /// - **model:** `"eleven_flash_v2_5"`
    ///
    /// `speed` and `volume` from [`TtsConfig`] are **not** forwarded —
    /// ElevenLabs uses its own voice-settings (stability, similarity boost)
    /// which are hard-coded for now.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::TtsProvider`] on network failure, non-2xx HTTP
    /// status, or failure to read the response body.
    async fn synthesize(&self, text: &str, config: TtsConfig) -> AppResult<Vec<u8>> {
        let voice_id = config
            .voice
            .as_deref()
            .unwrap_or("21m00Tcm4TlvDq8ikWAM");
        let model = config.model.as_deref().unwrap_or("eleven_flash_v2_5");
        let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{voice_id}");

        let body = TtsRequest {
            text: text.to_string(),
            model_id: model.to_string(),
            voice_settings: VoiceSettings {
                stability: 0.5,
                similarity_boost: 0.75,
                style: 0.0,
                use_speaker_boost: true,
            },
        };

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::TtsProvider(format!("ElevenLabs TTS failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let err_text = medical_core::http_error_body::read_error_body(response, 200).await;
            return Err(AppError::TtsProvider(format!(
                "ElevenLabs TTS HTTP {status}: {err_text}"
            )));
        }

        let audio_bytes = response
            .bytes()
            .await
            .map_err(|e| AppError::TtsProvider(format!("Failed to read audio: {e}")))?;

        Ok(audio_bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn voice_list_not_empty() {
        let provider = ElevenLabsTtsProvider {
            client: reqwest::Client::new(),
        };
        let voices = provider.available_voices().await.unwrap();
        assert!(!voices.is_empty());
        assert!(voices.len() >= 5);
    }

    #[test]
    fn provider_name() {
        let provider = ElevenLabsTtsProvider {
            client: reqwest::Client::new(),
        };
        assert_eq!(provider.name(), "elevenlabs");
    }

    #[test]
    fn invalid_api_key_header_returns_error() {
        // HTTP header values cannot contain newlines or non-ASCII characters
        let invalid_api_key = "invalid\nkey\rwith\0control";
        let result = ElevenLabsTtsProvider::new(invalid_api_key);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TtsError::InvalidHeader(_)));

        let err_msg = err.to_string();
        assert!(
            err_msg.contains("xi-api-key header"),
            "error message should mention which header failed, got: {err_msg}"
        );
    }

    #[test]
    fn valid_api_key_creates_provider() {
        // Valid ASCII string should work
        let valid_api_key = "test-api-key-12345_ABCDE";
        let result = ElevenLabsTtsProvider::new(valid_api_key);
        assert!(result.is_ok(), "valid API key should create provider");
    }
}
