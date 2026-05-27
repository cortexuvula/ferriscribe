//! HTTP client for Whisper STT API communication.

use reqwest::{
    multipart::{Form, Part},
    Client, StatusCode,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use medical_core::error::{AppError, AppResult, ServiceKind};

/// Response structure from Whisper API `verbose_json` format.
///
/// Contains timestamped segments, optional detected language, and optional
/// full-text transcript. Server implementations may omit `language` or `text`.
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
///
/// Timestamps are in seconds (f32 from the server, used as-is by the merge layer).
#[derive(Debug, Deserialize)]
pub struct VerboseSegment {
    /// Segment start time in seconds.
    pub start: f32,
    /// Segment end time in seconds.
    pub end: f32,
    /// Transcribed text for this segment. May be `None` for silent segments.
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

    // Drive the HTTP send concurrently with the cancellation token.
    // With `biased;`, the cancel branch is checked first on each poll so a
    // mid-flight cancellation is observed promptly. Dropping the request future
    // tears down the underlying reqwest connection at the TCP layer.
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
        // The auth proxy at crates/sharing/src/auth_proxy.rs tags its 401s
        // with `x-auth-reason: unknown-token` when the bearer doesn't match
        // any non-revoked row — the orphaned-pairing case (office server
        // rebuilt after pair). Surface a specific re-pair instruction in
        // that case; fall back to a generic auth-failure message otherwise.
        // The header values are a contract with the proxy; do not change
        // without coordinating the producer side.
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

    // Body parsing is also awaited under cancellation — large/slow responses
    // shouldn't pin the caller after they've asked to bail out.
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AppError::Cancelled),
        result = resp.json::<VerboseJson>() => result.map_err(|e| {
            AppError::SttProvider(format!("Unexpected response from Whisper server: {}", e))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn verbose_body() -> serde_json::Value {
        serde_json::json!({
            "text": "Hello patient.",
            "segments": [
                { "start": 0.0, "end": 1.0, "text": "Hello patient." }
            ],
            "language": "en",
            "duration": 1.0
        })
    }

    #[tokio::test]
    async fn authorization_header_sent_when_api_key_present() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(verbose_body()))
            .mount(&server)
            .await;

        let client = Client::new();
        let result = post_audio(
            &client,
            &server.uri(),
            "whisper-1",
            Some("sk-test"),
            vec![0u8; 100],
            None,
            &CancellationToken::new(),
        )
        .await;

        assert!(result.is_ok(), "expected ok, got: {:?}", result);
    }

    #[tokio::test]
    async fn no_authorization_header_when_api_key_absent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(verbose_body()))
            .mount(&server)
            .await;

        let client = Client::new();
        let result = post_audio(
            &client,
            &server.uri(),
            "whisper-1",
            None,
            vec![0u8; 100],
            None,
            &CancellationToken::new(),
        )
        .await;

        assert!(result.is_ok(), "expected ok, got: {:?}", result);
    }

    #[tokio::test]
    async fn http_401_with_unknown_token_reason_maps_to_repair_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header("x-auth-reason", "unknown-token"),
            )
            .mount(&server)
            .await;

        let client = Client::new();
        let result = post_audio(
            &client,
            &server.uri(),
            "whisper-1",
            Some("bad-key"),
            vec![0u8; 100],
            None,
            &CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("no longer recognizes"), "got: {}", err_msg);
    }

    #[tokio::test]
    async fn http_401_without_reason_header_maps_to_generic_auth_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = Client::new();
        let result = post_audio(
            &client,
            &server.uri(),
            "whisper-1",
            Some("bad-key"),
            vec![0u8; 100],
            None,
            &CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("rejected authentication"), "got: {}", err_msg);
    }

    #[tokio::test]
    async fn http_503_maps_to_server_internal_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
            .mount(&server)
            .await;

        let client = Client::new();
        let result = post_audio(
            &client,
            &server.uri(),
            "whisper-1",
            None,
            vec![0u8; 100],
            None,
            &CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("internal error"), "got: {}", err_msg);
    }

    #[tokio::test]
    async fn http_500_with_partial_body_includes_diagnostic_marker() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("model load failed"))
            .mount(&server)
            .await;

        let client = Client::new();
        let result = post_audio(
            &client,
            &server.uri(),
            "whisper-1",
            None,
            vec![0u8; 100],
            None,
            &CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("500"), "status code missing: {}", err_msg);
        assert!(err_msg.contains("model load failed"), "body missing: {}", err_msg);
    }

    #[tokio::test]
    async fn malformed_json_maps_to_parse_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = Client::new();
        let result = post_audio(
            &client,
            &server.uri(),
            "whisper-1",
            None,
            vec![0u8; 100],
            None,
            &CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("Unexpected response"), "got: {}", err_msg);
    }
}
