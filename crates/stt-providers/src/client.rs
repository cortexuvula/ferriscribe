//! HTTP client for Whisper STT API communication.

use reqwest::{
    Client, StatusCode,
    multipart::{Form, Part},
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
        // whisper.cpp's HTTP server rejects BCP-47 tags with a region/script
        // suffix (e.g. "en-US") — `whisper_lang_id` returns -1 and the server
        // crashes, which surfaces upstream as a 502/EOF. Local whisper-rs
        // tolerates these tags, so the bug only manifests in remote/office
        // mode. Normalize to the 2-letter ISO-639-1 code that both paths
        // accept. Fall back to the original string if the split yields nothing.
        let normalized = lang.split(['-', '_']).next().unwrap_or(lang);
        form = form.text("language", normalized.to_string());
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
        let reason = resp
            .headers()
            .get("x-auth-reason")
            .and_then(|v| v.to_str().ok());
        let msg = match reason {
            Some("unknown-token") => {
                "Office server no longer recognizes this client \u{2014} please re-pair (Settings \u{2192} Sharing \u{2192} Unpair, then scan a fresh code from the office machine)."
            }
            _ => {
                "Whisper server rejected authentication \u{2014} re-pair the client if the office server was reinstalled."
            }
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
        // The auth proxy (crates/sharing/src/auth_proxy.rs) tags its
        // backend-unreachable 502s with `x-proxy-reason: backend-unreachable`
        // and a plain-text body that tells the operator to restart Sharing on
        // the office machine. Surface that verbatim so the user gets an
        // actionable hint instead of a cryptic "502 Bad Gateway". The header
        // is a contract with the proxy; do not change without coordinating.
        if let Some(reason) = resp
            .headers()
            .get("x-proxy-reason")
            .and_then(|v| v.to_str().ok())
            && reason == "backend-unreachable"
        {
            let body = medical_core::http_error_body::read_error_body(resp, 200).await;
            let msg = if body.trim().is_empty() {
                "Office Whisper server is down \u{2014} restart Sharing on the office machine (Settings \u{2192} Sharing \u{2192} Stop, then Start).".to_string()
            } else {
                body
            };
            return Err(AppError::SttProvider(msg));
        }
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

    /// Regression: whisper.cpp's HTTP server crashes on BCP-47 tags like
    /// "en-US" (whisper_lang_id returns -1). We must normalize to the bare
    /// 2-letter ISO-639-1 code before sending. We assert this with wiremock's
    /// body-contains matcher on the raw multipart payload — the normalized
    /// "language=en" field must be present and the raw "en-US" tag must not.
    #[tokio::test]
    async fn bcp47_language_normalized_to_iso_639_1_for_remote() {
        use wiremock::matchers::body_string_contains;

        let server = MockServer::start().await;
        // Positive assertion: the multipart body contains the normalized
        // language field. The trailing CRLF is part of the multipart framing
        // so it discriminates "en\r\n" from "en-US\r\n".
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(body_string_contains("name=\"language\"\r\n\r\nen\r\n"))
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
            // The buggy code forwarded this verbatim; whisper.cpp then crashed.
            Some("en-US"),
            &CancellationToken::new(),
        )
        .await;

        assert!(
            result.is_ok(),
            "expected request to match normalized 'language=en' field; \
             wiremock did not see it — got: {result:?}",
        );

        // Negative assertion via a second server: if the buggy 'en-US' value
        // were sent, this mock (which only matches the un-normalized tag)
        // would fire. wiremock returns 404 by default, so a non-2xx response
        // here is the success condition.
        let neg_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(body_string_contains("name=\"language\"\r\n\r\nen-US\r\n"))
            .respond_with(ResponseTemplate::new(200).set_body_json(verbose_body()))
            .mount(&neg_server)
            .await;

        let neg_result = post_audio(
            &client,
            &neg_server.uri(),
            "whisper-1",
            None,
            vec![0u8; 100],
            Some("en-US"),
            &CancellationToken::new(),
        )
        .await;

        assert!(
            neg_result.is_err(),
            "BCP-47 tag 'en-US' leaked through to the remote server (would crash whisper.cpp)",
        );
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
                ResponseTemplate::new(401).insert_header("x-auth-reason", "unknown-token"),
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
        assert!(
            err_msg.contains("rejected authentication"),
            "got: {}",
            err_msg
        );
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
        assert!(
            err_msg.contains("model load failed"),
            "body missing: {}",
            err_msg
        );
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
