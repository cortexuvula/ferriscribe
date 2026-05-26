//! HTTP client for Whisper STT API communication.

use reqwest::{
    multipart::{Form, Part},
    Client, StatusCode,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use medical_core::error::{AppError, AppResult, ServiceKind};

/// Response structure from Whisper API verbose_json format.
#[derive(Debug, Deserialize)]
pub struct VerboseJson {
    #[serde(default)]
    pub segments: Vec<VerboseSegment>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

/// Individual segment from Whisper API response.
#[derive(Debug, Deserialize)]
pub struct VerboseSegment {
    pub start: f32,
    pub end: f32,
    #[serde(default)]
    pub text: Option<String>,
}

/// Post audio to Whisper API and return parsed transcription.
///
/// Handles multipart form upload, authentication, error responses, and cancellation.
pub async fn post_audio(
    client: &Client,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    wav_bytes: Vec<u8>,
    language: Option<&str>,
    cancel: &CancellationToken,
) -> AppResult<VerboseJson> {
    let url = format!("{}/v1/audio/transcriptions", base_url);

    // Build multipart form
    let mut form = Form::new()
        .part(
            "file",
            Part::bytes(wav_bytes)
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .map_err(|e| AppError::SttProvider(format!("multipart error: {}", e)))?,
        )
        .text("model", model.to_string())
        .text("response_format", "verbose_json");

    if let Some(lang) = language.filter(|l| !l.is_empty()) {
        form = form.text("language", lang.to_string());
    }

    // Build request with optional auth
    let mut req = client.post(&url).multipart(form);
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        req = req.header("Authorization", format!("Bearer {}", key));
    }

    // Send request with cancellation
    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            return Err(AppError::Cancelled);
        }
        result = req.send() => {
            result.map_err(|e| {
                use medical_core::preflight::classify_reqwest_error;
                match classify_reqwest_error(&e) {
                    Some(reason) => AppError::EndpointOffline {
                        service: ServiceKind::RemoteStt,
                        endpoint: base_url.to_string(),
                        reason,
                        provider_name: "Whisper STT".into(),
                    },
                    None => AppError::SttProvider(format!("Whisper request failed: {}", e)),
                }
            })?
        }
    };

    // Handle HTTP errors
    let status = resp.status();

    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let reason = resp.headers().get("x-auth-reason").and_then(|v| v.to_str().ok());
        let msg = match reason {
            Some("unknown-token") => {
                "Office server no longer recognizes this client \u{2014} please re-pair (Settings \u{2192} Sharing \u{2192} Unpair, then scan a fresh code from the office machine)."
            }
            _ => "Whisper server rejected authentication \u{2014} re-pair the client if the office server was reinstalled.",
        };
        return Err(AppError::SttProvider(msg.to_string()));
    }

    if status.is_client_error() {
        let body = medical_core::http_error_body::read_error_body(resp, 200).await;
        return Err(AppError::SttProvider(format!(
            "Whisper server rejected request: {} {}",
            status, body
        )));
    }

    if status.is_server_error() {
        let body = medical_core::http_error_body::read_error_body(resp, 200).await;
        return Err(AppError::SttProvider(format!(
            "Whisper server internal error: {} {}",
            status, body
        )));
    }

    // Parse response with cancellation
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AppError::Cancelled),
        result = resp.json::<VerboseJson>() => result.map_err(|e| {
            AppError::SttProvider(format!("Unexpected response from Whisper server: {}", e))
        }),
    }
}
