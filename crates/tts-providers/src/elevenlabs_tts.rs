use async_trait::async_trait;
use medical_core::error::{AppError, AppResult};
use medical_core::traits::TtsProvider;
use medical_core::types::tts::{TtsConfig, VoiceInfo};
use reqwest::Client;
use serde::Serialize;

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
    pub fn new(api_key: &str) -> AppResult<Self> {
        let client = Client::builder()
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert("xi-api-key", api_key.parse().unwrap());
                h.insert("Content-Type", "application/json".parse().unwrap());
                h
            })
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| AppError::TtsProvider(e.to_string()))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl TtsProvider for ElevenLabsTtsProvider {
    fn name(&self) -> &str {
        "elevenlabs"
    }

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
}
