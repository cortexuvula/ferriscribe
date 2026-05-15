//! RemoteSttProvider — OpenAI-compatible Whisper server client.
//!
//! Sends a 16 kHz mono PCM WAV to `POST {base}/v1/audio/transcriptions` and
//! parses `verbose_json` back into `TranscriptSegment[]`. Local pyannote
//! diarization runs on the same audio buffer (paralleling `LocalSttProvider`)
//! so speaker labels still work even when Whisper is remote.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use futures_core::Stream;
use reqwest::{
    Client,
    multipart::{Form, Part},
};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use medical_core::error::{AppError, AppResult};
use medical_core::traits::SttProvider;
use medical_core::types::{
    http_url, AudioData, AudioStream, RemoteEndpoint, SttConfig, Transcript, TranscriptChunk,
    TranscriptSegment,
};

use crate::audio_prep;
use crate::diarization::SpeakerDiarizer;
use crate::merge;
use crate::whisper::WhisperSegment;

const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(600);
const TARGET_SAMPLE_RATE: u32 = 16_000;

// ──────────────────────────────────────────────────────────────────────────────
// 30-second resolved-URL cache for RemoteEndpoint resolution
// ──────────────────────────────────────────────────────────────────────────────

struct ResolvedCache {
    url: String,
    resolved_at: std::time::Instant,
}

const CACHE_TTL: Duration = Duration::from_secs(30);

// ──────────────────────────────────────────────────────────────────────────────

pub struct RemoteSttProvider {
    client: Client,
    /// Fallback static base URL used when no `endpoint` is configured.
    base_url: String,
    model: String,
    /// Bearer token sent as `Authorization: Bearer <token>`. Semantically
    /// this is a bearer credential (for the auth proxy in paired mode), not
    /// a whisper.cpp `--api-key`. The field name `api_key` is preserved to
    /// avoid churn at existing call sites.
    api_key: RwLock<Option<String>>,
    segmentation_model_path: PathBuf,
    embedding_model_path: PathBuf,
    /// Optional LAN/Tailscale endpoint. When set, `current_base_url()` resolves
    /// the first reachable address with a 30-second cache.
    endpoint: RwLock<Option<RemoteEndpoint>>,
    url_cache: Mutex<Option<ResolvedCache>>,
}

#[derive(Debug, Deserialize)]
struct VerboseJson {
    #[serde(default)]
    segments: Vec<VerboseSegment>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VerboseSegment {
    start: f32,
    end: f32,
    #[serde(default)]
    text: Option<String>,
}

impl RemoteSttProvider {
    pub fn new(
        host: &str,
        port: u16,
        model: &str,
        allow_public: bool,
        api_key: Option<String>,
        segmentation_model_path: PathBuf,
        embedding_model_path: PathBuf,
    ) -> AppResult<Self> {
        if !host.is_empty() {
            medical_core::endpoint_policy::validate_local_endpoint(host, allow_public)
                .map_err(|e| AppError::invalid_endpoint_for(e, "stt_remote_host"))?;
        }
        let host = if host.is_empty() { "localhost" } else { host };
        let base_url = http_url(host, port);

        let client = Client::builder()
            .pool_max_idle_per_host(4)
            .connect_timeout(Duration::from_secs(10))
            .timeout(TRANSCRIBE_TIMEOUT)
            .build()
            .map_err(|e| AppError::SttProvider(format!("Failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            base_url,
            model: model.to_string(),
            api_key: RwLock::new(api_key),
            segmentation_model_path,
            embedding_model_path,
            endpoint: RwLock::new(None),
            url_cache: Mutex::new(None),
        })
    }

    /// Create a new RemoteSttProvider with a `RemoteEndpoint` pre-configured.
    ///
    /// Usable in synchronous initialization code (no running async runtime required).
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_endpoint(
        host: &str,
        port: u16,
        model: &str,
        allow_public: bool,
        api_key: Option<String>,
        segmentation_model_path: PathBuf,
        embedding_model_path: PathBuf,
        ep: Option<RemoteEndpoint>,
    ) -> AppResult<Self> {
        if !host.is_empty() {
            medical_core::endpoint_policy::validate_local_endpoint(host, allow_public)
                .map_err(|e| AppError::invalid_endpoint_for(e, "stt_remote_host"))?;
        }
        let host = if host.is_empty() { "localhost" } else { host };
        let base_url = http_url(host, port);
        let client = Client::builder()
            .pool_max_idle_per_host(4)
            .connect_timeout(Duration::from_secs(10))
            .timeout(TRANSCRIBE_TIMEOUT)
            .build()
            .map_err(|e| AppError::SttProvider(format!("Failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            base_url,
            model: model.to_string(),
            api_key: RwLock::new(api_key),
            segmentation_model_path,
            embedding_model_path,
            endpoint: RwLock::new(ep),
            url_cache: Mutex::new(None),
        })
    }

    /// Override the remote endpoint used for LAN/Tailscale resolution.
    /// Invalidates the URL cache, replaces the endpoint, and propagates the
    /// endpoint's bearer into `api_key` so subsequent transcribe requests
    /// authenticate with the current token. Without the last step, an
    /// in-session Unpair → Pair leaves a stale bearer baked in at
    /// construction time — a 401 source if the office admin revoked the
    /// previous client entry.
    pub async fn set_endpoint(
        &self,
        ep: Option<RemoteEndpoint>,
        allow_public: bool,
    ) -> AppResult<()> {
        if let Some(ref e) = ep {
            for (label, opt_host) in [
                ("lan", e.lan.as_deref()),
                ("tailscale", e.tailscale.as_deref()),
            ] {
                if let Some(h) = opt_host {
                    medical_core::endpoint_policy::validate_local_endpoint(h, allow_public)
                        .map_err(|err| AppError::invalid_endpoint_for(
                            err,
                            format!("stt_remote_host.{label}"),
                        ))?;
                }
            }
        }
        let new_bearer = ep.as_ref().and_then(|e| e.bearer.clone());
        *self.url_cache.lock().await = None;
        *self.endpoint.write().await = ep;
        *self.api_key.write().await = new_bearer;
        Ok(())
    }

    /// Resolve the current base URL (no trailing path).
    /// If a RemoteEndpoint is configured, probe LAN then Tailscale with a 30s
    /// cache.  Falls back to `self.base_url` when no endpoint is set.
    async fn current_base_url(&self) -> AppResult<String> {
        let ep_guard = self.endpoint.read().await;
        if let Some(ep) = ep_guard.as_ref() {
            let mut cache = self.url_cache.lock().await;
            if let Some(c) = cache.as_ref() {
                if c.resolved_at.elapsed() < CACHE_TTL {
                    return Ok(c.url.clone());
                }
            }
            let url = ep
                .resolve_base_url()
                .await
                .ok_or_else(|| {
                    use medical_core::error::{OfflineReason, ServiceKind};
                    // RemoteEndpoint probed LAN then Tailscale and both failed. Pick
                    // the LAN URL as the representative endpoint; if LAN isn't set,
                    // fall back to Tailscale; if neither is set, "(unresolved)"
                    // surfaces clearly in the dialog.
                    let endpoint = ep
                        .lan
                        .as_deref()
                        .map(|h| http_url(h, ep.port))
                        .or_else(|| ep.tailscale.as_deref().map(|h| http_url(h, ep.port)))
                        .unwrap_or_else(|| "(unresolved)".into());
                    AppError::EndpointOffline {
                        service: ServiceKind::RemoteStt,
                        endpoint,
                        reason: OfflineReason::Timeout,
                        provider_name: "Whisper STT".into(),
                    }
                })?;
            *cache = Some(ResolvedCache {
                url: url.clone(),
                resolved_at: std::time::Instant::now(),
            });
            return Ok(url);
        }
        Ok(self.base_url.clone())
    }

    fn diarization_available(&self) -> bool {
        self.segmentation_model_path.exists() && self.embedding_model_path.exists()
    }

    async fn post_audio(
        &self,
        wav_bytes: Vec<u8>,
        language: Option<&str>,
        cancel: &CancellationToken,
    ) -> AppResult<VerboseJson> {
        let base = self.current_base_url().await?;
        let url = format!("{base}/v1/audio/transcriptions");

        let mut form = Form::new()
            .part(
                "file",
                Part::bytes(wav_bytes)
                    .file_name("audio.wav")
                    .mime_str("audio/wav")
                    .map_err(|e| AppError::SttProvider(format!("multipart error: {e}")))?,
            )
            .text("model", self.model.clone())
            .text("response_format", "verbose_json");
        if let Some(lang) = language.filter(|l| !l.is_empty()) {
            form = form.text("language", lang.to_string());
        }

        let mut req = self.client.post(&url).multipart(form);
        let api_key_snapshot = self.api_key.read().await.clone();
        if let Some(key) = api_key_snapshot.as_deref().filter(|k| !k.is_empty()) {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        // Drive the HTTP send concurrently with the cancellation token. With
        // `biased;`, the cancel branch is checked first on each poll so a
        // mid-flight cancellation is observed promptly. Dropping the request
        // future tears down the underlying reqwest connection at the TCP layer.
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return Err(AppError::Cancelled);
            }
            result = req.send() => {
                result.map_err(|e| {
                    use medical_core::error::ServiceKind;
                    use medical_core::preflight::classify_reqwest_error;
                    match classify_reqwest_error(&e) {
                        Some(reason) => AppError::EndpointOffline {
                            service: ServiceKind::RemoteStt,
                            endpoint: base.clone(),
                            reason,
                            provider_name: "Whisper STT".into(),
                        },
                        None => AppError::SttProvider(format!("Whisper request failed: {e}")),
                    }
                })?
            }
        };

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
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
                    "Office server no longer recognizes this client \
                     \u{2014} please re-pair (Settings \u{2192} Sharing \u{2192} Unpair, \
                     then scan a fresh code from the office machine)."
                        .to_string()
                }
                _ => "Whisper server rejected authentication \u{2014} \
                      re-pair the client if the office server was reinstalled."
                    .to_string(),
            };
            return Err(AppError::SttProvider(msg));
        }
        if status.is_client_error() {
            let body = medical_core::http_error_body::read_error_body(resp, 200).await;
            return Err(AppError::SttProvider(format!(
                "Whisper server rejected request: {status} {body}"
            )));
        }
        if status.is_server_error() {
            let body = medical_core::http_error_body::read_error_body(resp, 200).await;
            return Err(AppError::SttProvider(format!(
                "Whisper server internal error: {status} {body}"
            )));
        }

        // Body parsing is also awaited under cancellation — large/slow responses
        // shouldn't pin the caller after they've asked to bail out.
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(AppError::Cancelled),
            result = resp.json::<VerboseJson>() => result.map_err(|e| {
                AppError::SttProvider(format!("Unexpected response from Whisper server: {e}"))
            }),
        }
    }
}

#[async_trait]
impl SttProvider for RemoteSttProvider {
    fn name(&self) -> &str {
        "remote"
    }

    fn supports_streaming(&self) -> bool {
        false
    }

    fn supports_diarization(&self) -> bool {
        self.diarization_available()
    }

    async fn transcribe(
        &self,
        audio: AudioData,
        config: SttConfig,
        cancel: CancellationToken,
    ) -> AppResult<Transcript> {
        let duration = audio.duration_seconds();

        // Resolve the server URL once for this transcription (cached 30s).
        let resolved_server = self.current_base_url().await?;

        // Stage 1: resample to 16 kHz mono f32, then convert to i16 for upload.
        let audio_16k = audio_prep::to_16k_mono_f32(&audio);
        let samples_i16 = audio_prep::f32_to_i16(&audio_16k);
        let wav_bytes = audio_prep::write_pcm16_wav_bytes(&samples_i16, TARGET_SAMPLE_RATE);

        // Stage 2: POST to the Whisper server (cancellable via tokio::select!).
        let parsed = self
            .post_audio(wav_bytes, config.language.as_deref(), &cancel)
            .await?;

        // Capture the server's full-text field (if any) before consuming `parsed.segments`.
        let server_text = parsed.text.clone();
        let server_language = parsed.language.clone();

        // Convert the server's segments into `WhisperSegment`s so they can be
        // handed to the existing `merge_segments_with_speakers` helper, which
        // outputs `TranscriptSegment`s with a `speaker` field filled in when
        // diarization turns are available.
        let whisper_segments: Vec<WhisperSegment> = parsed
            .segments
            .into_iter()
            .filter_map(|s| {
                let text = s.text?;
                if text.trim().is_empty() {
                    return None;
                }
                Some(WhisperSegment {
                    start: s.start as f64,
                    end: s.end as f64,
                    text,
                })
            })
            .collect();

        // Stage 3: local diarization if requested and models present.
        let speaker_turns = if config.diarize && self.diarization_available() {
            let seg_path = self.segmentation_model_path.clone();
            let emb_path = self.embedding_model_path.clone();
            let audio_for_diarize = samples_i16;
            match tokio::task::spawn_blocking(move || {
                let diarizer = SpeakerDiarizer::new(seg_path, emb_path);
                diarizer.diarize(&audio_for_diarize, TARGET_SAMPLE_RATE)
            })
            .await
            {
                Ok(Ok(turns)) => turns,
                Ok(Err(e)) => {
                    warn!(error = %e, "Diarization failed — proceeding without speaker labels");
                    Vec::new()
                }
                Err(e) => {
                    warn!(error = %e, "Diarization task panicked — proceeding without speaker labels");
                    Vec::new()
                }
            }
        } else {
            if config.diarize && !self.diarization_available() {
                warn!("Diarization requested but pyannote models not found — skipping");
            }
            Vec::new()
        };

        // Stage 4: merge speaker turns with whisper segments.
        let merged: Vec<TranscriptSegment> =
            merge::merge_segments_with_speakers(&whisper_segments, &speaker_turns);

        let full_text = server_text.unwrap_or_else(|| {
            merged
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        });

        info!(
            segments = merged.len(),
            text_len = full_text.len(),
            "Remote transcription complete"
        );

        Ok(Transcript {
            text: full_text,
            segments: merged,
            language: server_language.or(config.language),
            duration_seconds: Some(duration),
            provider: "remote".to_owned(),
            metadata: serde_json::json!({
                "server": resolved_server,
                "model": self.model,
            }),
        })
    }

    async fn transcribe_stream(
        &self,
        _stream: AudioStream,
        _config: SttConfig,
    ) -> AppResult<Box<dyn Stream<Item = AppResult<TranscriptChunk>> + Send + Unpin>> {
        Err(AppError::SttProvider(
            "Remote provider does not support streaming transcription".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::{AudioData, SttConfig};
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn dummy_audio() -> AudioData {
        // 1 second of silent 16 kHz mono f32.
        AudioData {
            samples: vec![0.0_f32; 16_000],
            sample_rate: 16_000,
            channels: 1,
        }
    }

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

    fn provider_at(base: &str, api_key: Option<String>) -> RemoteSttProvider {
        // Strip the http:// prefix to feed RemoteSttProvider::new which re-adds it.
        let stripped = base.trim_start_matches("http://");
        let (host, port) = stripped
            .split_once(':')
            .map(|(h, p)| (h.to_string(), p.parse::<u16>().unwrap()))
            .unwrap();
        RemoteSttProvider::new(
            &host,
            port,
            "whisper-1",
            /* allow_public */ false,
            api_key,
            PathBuf::from("/nonexistent-seg.onnx"),
            PathBuf::from("/nonexistent-emb.onnx"),
        )
        .expect("build provider")
    }

    #[tokio::test]
    async fn happy_path_returns_segments_without_diarization() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(verbose_body()))
            .mount(&server)
            .await;

        let provider = provider_at(&server.uri(), None);
        let transcript = provider
            .transcribe(
                dummy_audio(),
                SttConfig { language: Some("en".into()), diarize: false, ..SttConfig::default() },
                CancellationToken::new(),
            )
            .await
            .expect("transcribe");

        assert_eq!(transcript.provider, "remote");
        assert_eq!(transcript.segments.len(), 1);
        assert_eq!(transcript.segments[0].text, "Hello patient.");
        assert!(transcript.segments[0].speaker.is_none());
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

        let provider = provider_at(&server.uri(), Some("sk-test".into()));
        let res = provider
            .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
            .await;
        assert!(res.is_ok(), "expected ok, got: {res:?}");
    }

    #[tokio::test]
    async fn no_authorization_header_when_api_key_absent() {
        let server = MockServer::start().await;
        // Match requests that DO have Authorization — they should be zero.
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(header_exists("Authorization"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        // Requests WITHOUT Authorization get a 200.
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(verbose_body()))
            .mount(&server)
            .await;

        let provider = provider_at(&server.uri(), None);
        let res = provider
            .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
            .await;
        assert!(res.is_ok(), "should not send Authorization without key");
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

        let provider = provider_at(&server.uri(), Some("stale".into()));
        let err = provider
            .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no longer recognizes"),
            "expected orphaned-pairing specific message, got: {err}"
        );
    }

    #[tokio::test]
    async fn http_401_without_reason_header_maps_to_generic_auth_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let provider = provider_at(&server.uri(), Some("bad".into()));
        let err = provider
            .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("authentication"),
            "expected generic auth error, got: {err}"
        );
    }

    #[tokio::test]
    async fn http_503_maps_to_server_internal_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let provider = provider_at(&server.uri(), None);
        let err = provider
            .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("internal error"),
            "expected 5xx error, got: {err}"
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

        let provider = provider_at(&server.uri(), None);
        let err = provider
            .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Unexpected response"),
            "expected parse error, got: {err}"
        );
    }

    #[test]
    fn diarization_available_is_false_without_models() {
        let p = RemoteSttProvider::new(
            "localhost",
            8080,
            "whisper-1",
            /* allow_public */ false,
            None,
            PathBuf::from("/nowhere/seg.onnx"),
            PathBuf::from("/nowhere/emb.onnx"),
        )
        .expect("build");
        assert!(!p.diarization_available());
    }

    #[tokio::test]
    async fn transcribe_returns_promptly_when_cancelled_mid_request() {
        use std::time::{Duration, Instant};
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Mock server that delays the response by 5 seconds — far longer
        // than the test should tolerate if cancellation works.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"text": "should not arrive"}))
                    .set_delay(Duration::from_secs(5)),
            )
            .mount(&server)
            .await;

        let provider = provider_at(&server.uri(), None);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        // Cancel after 100ms.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_clone.cancel();
        });

        let started = Instant::now();
        let result = provider
            .transcribe(dummy_audio(), SttConfig::default(), cancel)
            .await;
        let elapsed = started.elapsed();

        // Should have returned an error (cancelled) well under the 5s mock delay.
        assert!(result.is_err(), "expected Err on cancellation, got {:?}", result);
        assert!(
            elapsed < Duration::from_secs(2),
            "transcribe should return promptly on cancel, took {:?}",
            elapsed
        );
        // The error should mention cancellation.
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.to_lowercase().contains("cancel"),
            "expected error to mention cancel, got: {msg}"
        );
    }

    #[tokio::test]
    async fn segments_without_text_are_skipped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "Hello.",
                "segments": [
                    { "start": 0.0, "end": 0.5 },
                    { "start": 0.5, "end": 1.0, "text": "" },
                    { "start": 1.0, "end": 2.0, "text": "Hello." }
                ]
            })))
            .mount(&server)
            .await;

        let provider = provider_at(&server.uri(), None);
        let transcript = provider
            .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
            .await
            .expect("transcribe");
        assert_eq!(transcript.segments.len(), 1, "empty/missing text segments must be filtered");
        assert_eq!(transcript.segments[0].text, "Hello.");
    }

    #[tokio::test]
    async fn set_endpoint_clears_url_cache() {
        let p = RemoteSttProvider::new(
            "localhost",
            8080,
            "whisper-1",
            /* allow_public */ false,
            None,
            PathBuf::from("/no/seg.onnx"),
            PathBuf::from("/no/emb.onnx"),
        )
        .expect("build");

        // Seed the cache manually.
        *p.url_cache.lock().await = Some(ResolvedCache {
            url: "http://stale:9999".to_string(),
            resolved_at: std::time::Instant::now(),
        });

        p.set_endpoint(None, false).await.expect("clear endpoint");
        assert!(p.url_cache.lock().await.is_none(), "cache must be cleared on set_endpoint");
    }

    #[tokio::test]
    async fn current_base_url_returns_static_when_no_endpoint() {
        // allow_public=true so the test can use an arbitrary hostname to verify
        // that URL construction round-trips the host as-is.
        let p = RemoteSttProvider::new(
            "myhost",
            8080,
            "whisper-1",
            /* allow_public */ true,
            None,
            PathBuf::from("/no/seg.onnx"),
            PathBuf::from("/no/emb.onnx"),
        )
        .expect("build");

        let url = p.current_base_url().await.expect("url");
        assert_eq!(url, "http://myhost:8080");
    }

    #[tokio::test]
    async fn http_500_with_partial_body_includes_diagnostic_marker() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("model load failed"))
            .mount(&server)
            .await;

        let provider = provider_at(&server.uri(), None);
        let err = provider
            .transcribe(dummy_audio(), SttConfig::default(), CancellationToken::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("500"), "expected status code in error: {err}");
        assert!(err.contains("model load failed"), "expected body content in error: {err}");
    }

    #[tokio::test]
    async fn current_base_url_caches_for_30s() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let p = RemoteSttProvider::new(
            "localhost",
            9999,
            "whisper-1",
            /* allow_public */ false,
            None,
            PathBuf::from("/no/seg.onnx"),
            PathBuf::from("/no/emb.onnx"),
        )
        .expect("build");

        p.set_endpoint(Some(RemoteEndpoint {
            lan: Some("127.0.0.1".to_string()),
            tailscale: None,
            port,
            bearer: None,
        }), false)
        .await
        .expect("set endpoint");

        // First call: port is open — should resolve.
        let url1 = p.current_base_url().await.expect("first resolve");
        assert!(url1.contains(&port.to_string()));

        // Drop listener so port is closed.
        drop(listener);

        // Second call immediately: cache should return the same URL.
        let url2 = p.current_base_url().await.expect("cached resolve");
        assert_eq!(url1, url2, "should return cached URL without re-probing");
    }

    #[test]
    fn new_blocks_public_host_by_default() {
        let result = RemoteSttProvider::new(
            "api.openai.com",
            8080,
            "whisper-1",
            /* allow_public */ false,
            None,
            std::path::PathBuf::from("/dev/null"),
            std::path::PathBuf::from("/dev/null"),
        );
        assert!(matches!(
            result,
            Err(medical_core::error::AppError::InvalidEndpoint {
                field, ..
            }) if field == "stt_remote_host"
        ));
    }

    #[test]
    fn new_accepts_public_host_when_allow_public() {
        let result = RemoteSttProvider::new(
            "api.openai.com",
            8080,
            "whisper-1",
            /* allow_public */ true,
            None,
            std::path::PathBuf::from("/dev/null"),
            std::path::PathBuf::from("/dev/null"),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn new_accepts_local_hosts_with_default_allow_public() {
        for host in ["localhost", "192.168.1.42", "100.64.0.1", "clinic.local"] {
            let r = RemoteSttProvider::new(
                host,
                8080,
                "whisper-1",
                /* allow_public */ false,
                None,
                std::path::PathBuf::from("/dev/null"),
                std::path::PathBuf::from("/dev/null"),
            );
            assert!(r.is_ok(), "expected Ok for {host}");
        }
    }

    #[test]
    fn new_accepts_empty_host() {
        // Empty host means "use default" — provider-level no-op; Settings save
        // layer enforces the stricter empty-vs-non-empty + mode policy.
        let r = RemoteSttProvider::new(
            "",
            8080,
            "whisper-1",
            /* allow_public */ false,
            None,
            std::path::PathBuf::from("/dev/null"),
            std::path::PathBuf::from("/dev/null"),
        );
        assert!(r.is_ok());
    }

    #[tokio::test]
    async fn set_endpoint_rejects_public_lan_address() {
        let p = RemoteSttProvider::new(
            "localhost",
            8080,
            "whisper-1",
            /* allow_public */ false,
            None,
            std::path::PathBuf::from("/dev/null"),
            std::path::PathBuf::from("/dev/null"),
        )
        .expect("build");
        let bad = medical_core::types::RemoteEndpoint {
            lan: Some("api.openai.com".into()),
            tailscale: None,
            port: 8080,
            bearer: None,
        };
        let r = p.set_endpoint(Some(bad), false).await;
        assert!(matches!(
            r,
            Err(medical_core::error::AppError::InvalidEndpoint { .. })
        ));
    }

    #[tokio::test]
    async fn set_endpoint_accepts_lan_and_tailscale_addresses() {
        let p = RemoteSttProvider::new(
            "localhost",
            8080,
            "whisper-1",
            /* allow_public */ false,
            None,
            std::path::PathBuf::from("/dev/null"),
            std::path::PathBuf::from("/dev/null"),
        )
        .expect("build");
        let good = medical_core::types::RemoteEndpoint {
            lan: Some("192.168.1.42".into()),
            tailscale: Some("100.64.0.1".into()),
            port: 8080,
            bearer: None,
        };
        assert!(p.set_endpoint(Some(good), false).await.is_ok());
    }
}

#[cfg(test)]
mod offline_tests {
    use super::*;
    use medical_core::error::{AppError, OfflineReason, ServiceKind};

    #[tokio::test]
    async fn transcribe_returns_endpoint_offline_when_remote_unreachable() {
        // Bind then immediately drop a TCP listener to get a port that will
        // refuse connections. The OS reclaims the port but new connections to
        // it yield ECONNREFUSED — the canonical "server is down" signal.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // Build the provider pointing at the now-closed port.
        let provider = RemoteSttProvider::new(
            "127.0.0.1",
            port,
            "whisper-1",
            /* allow_public */ false,
            None,
            PathBuf::from("/nonexistent-seg.onnx"),
            PathBuf::from("/nonexistent-emb.onnx"),
        )
        .expect("build provider");

        // Use 1 second of silent 16 kHz mono f32 audio — same shape as the
        // happy-path tests above.
        let audio = AudioData {
            samples: vec![0.0_f32; 16_000],
            sample_rate: 16_000,
            channels: 1,
        };

        let err = provider
            .transcribe(audio, SttConfig::default(), CancellationToken::new())
            .await
            .unwrap_err();

        match err {
            AppError::EndpointOffline {
                service,
                endpoint,
                reason,
                provider_name,
            } => {
                assert_eq!(service, ServiceKind::RemoteStt);
                assert_eq!(
                    reason,
                    OfflineReason::ConnectionRefused,
                    "refused loopback port must yield ConnectionRefused, not {reason:?}"
                );
                assert_eq!(provider_name, "Whisper STT");
                assert!(
                    endpoint.contains("127.0.0.1"),
                    "endpoint should carry the target host; got {endpoint:?}"
                );
            }
            other => panic!("expected EndpointOffline, got {other:?}"),
        }
    }
}
