//! Pre-flight connectivity probes for remote AI / STT endpoints.
//!
//! `probe_endpoint` issues a single short-timeout GET. `classify_reqwest_error`
//! maps a `reqwest::Error` into an `OfflineReason`. `preflight_for_command`
//! (added in Task 3) composes these with settings.

use std::error::Error as StdError;
use std::time::Duration;

use reqwest::Client;
use tracing::{debug, warn};

use crate::error::{AppError, OfflineReason, ServiceKind};
use crate::types::settings::AppConfig;

/// Cap on a single probe's wall time.
///
/// Chosen to be long enough for a healthy LAN round-trip plus TLS
/// handshake (~200ms typical) but short enough that an offline server
/// doesn't visibly stall the UI.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Pinned, boxed, Send future returned by each preflight probe. Factored into
/// a type alias so the `Vec` holding the pending probes stays readable.
pub type PreflightFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AppError>> + Send>>;

/// Classify a failed reqwest send into the user-facing [`OfflineReason`].
///
/// Returns `None` if the error is something other than a connection /
/// timeout / DNS / TLS failure (e.g. URL parse error, body decode) —
/// caller should treat that as a genuine bug rather than an offline
/// endpoint.
///
/// Walks the [`std::error::Error::source`] chain looking for hyper /
/// `std::io` signals that distinguish DNS failures from plain connection
/// refusals (both report `is_connect() == true`).
pub fn classify_reqwest_error(err: &reqwest::Error) -> Option<OfflineReason> {
    if err.is_timeout() {
        return Some(OfflineReason::Timeout);
    }
    if err.is_connect() {
        // Walk the error source chain looking for hyper / std::io
        // signals that distinguish DNS failures from plain connection
        // refusals. `is_connect()` is true for both.
        let mut source: Option<&(dyn StdError + 'static)> = StdError::source(err);
        while let Some(s) = source {
            let s_str: String = s.to_string().to_lowercase();
            if s_str.contains("dns") || s_str.contains("failed to lookup") {
                return Some(OfflineReason::DnsFailure);
            }
            if s_str.contains("tls") || s_str.contains("handshake") || s_str.contains("certificate")
            {
                return Some(OfflineReason::TlsFailure);
            }
            source = StdError::source(s);
        }
        return Some(OfflineReason::ConnectionRefused);
    }
    None
}

/// Probe a single endpoint for connectivity.
///
/// Returns `Ok(())` if the server responded with *any* HTTP status
/// (including 4xx / 5xx) — auth / API errors are not connectivity errors
/// and are handled by the real call. Returns
/// [`AppError::EndpointOffline`] if the connection fails, with the
/// reason classified via [`classify_reqwest_error`].
///
/// If a `bearer` token is provided, it is sent as
/// `Authorization: Bearer <token>` in the probe request.
///
/// # Errors
///
/// - [`AppError::Config`] if the reqwest client cannot be built.
/// - [`AppError::EndpointOffline`] if the endpoint is unreachable.
/// - [`AppError::Other`] if the error is not a connectivity failure.
pub async fn probe_endpoint(
    service: ServiceKind,
    provider_name: &str,
    base_url: &str,
    probe_path: &str,
    bearer: Option<&str>,
) -> Result<(), AppError> {
    let client = Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|e| AppError::Config(format!("preflight client build failed: {e}")))?;

    let url = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        probe_path.trim_start_matches('/'),
    );
    let mut req = client.get(&url);
    if let Some(b) = bearer {
        req = req.header("Authorization", format!("Bearer {b}"));
    }

    let start = std::time::Instant::now();
    let result = req.send().await;

    match result {
        Ok(_response) => {
            let elapsed_ms = start.elapsed().as_millis();
            // Any HTTP status counts as "reachable" for our purposes.
            debug!(
                provider = provider_name,
                url = %url,
                elapsed_ms,
                "preflight probe reachable"
            );
            Ok(())
        }
        Err(e) => {
            let elapsed_ms = start.elapsed().as_millis();
            match classify_reqwest_error(&e) {
                Some(reason) => {
                    warn!(
                        provider = provider_name,
                        url = %url,
                        elapsed_ms,
                        reason = ?reason,
                        "preflight probe failed"
                    );
                    Err(AppError::EndpointOffline {
                        service,
                        endpoint: base_url.to_string(),
                        reason,
                        provider_name: provider_name.to_string(),
                    })
                }
                None => {
                    tracing::error!(
                        provider = provider_name,
                        url = %url,
                        elapsed_ms,
                        error = %e,
                        "preflight probe failed with non-connectivity error"
                    );
                    Err(AppError::Other(format!(
                        "Unexpected probe error against {provider_name} at {base_url}: {e}"
                    )))
                }
            }
        }
    }
}

/// Which Tauri command is about to run.
///
/// Drives which endpoint(s) are probed by
/// [`preflight_for_command`]. Each variant maps to either the AI
/// provider endpoint, the STT endpoint, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// Speech-to-text — probes the remote STT endpoint (if configured).
    Transcribe,
    /// SOAP note generation — probes the AI provider endpoint.
    GenerateSoap,
    /// Referral letter generation — probes the AI provider endpoint.
    GenerateReferral,
    /// Patient letter generation — probes the AI provider endpoint.
    GenerateLetter,
    /// Synopsis generation — probes the AI provider endpoint.
    GenerateSynopsis,
    /// Peer discussion note generation — probes the AI provider endpoint.
    GeneratePeerDiscussion,
    /// Interactive chat — probes the AI provider endpoint.
    Chat,
}

/// Inspect settings, decide which remote endpoints this command needs,
/// probe each in parallel with a 3 s timeout, and return `Ok(())` if all
/// are reachable (or skipped).
///
/// Endpoints whose host is loopback (`127.0.0.1`, `::1`, `localhost`,
/// `""`) are skipped entirely — failures from local servers surface via
/// the real call's error mapper using the same `EndpointOffline` variant.
///
/// # Errors
///
/// Returns the first [`AppError::EndpointOffline`] if any probed endpoint
/// is unreachable.
pub async fn preflight_for_command(
    kind: CommandKind,
    settings: &AppConfig,
) -> Result<(), AppError> {
    let mut futs: Vec<PreflightFuture> = Vec::new();

    let ai_needed = matches!(
        kind,
        CommandKind::GenerateSoap
            | CommandKind::GenerateReferral
            | CommandKind::GenerateLetter
            | CommandKind::GenerateSynopsis
            | CommandKind::GeneratePeerDiscussion
            | CommandKind::Chat,
    );
    let stt_needed = matches!(kind, CommandKind::Transcribe);

    if ai_needed && let Some(probe) = build_ai_probe(settings) {
        futs.push(Box::pin(probe));
    }
    if stt_needed && let Some(probe) = build_stt_probe(settings) {
        futs.push(Box::pin(probe));
    }

    if futs.is_empty() {
        return Ok(());
    }

    let results = futures_util::future::join_all(futs).await;
    for r in results {
        r?;
    }
    Ok(())
}

/// Returns `Some(future)` if the active AI provider has a non-loopback
/// host worth probing; `None` if it's local or empty.
fn build_ai_probe(
    settings: &AppConfig,
) -> Option<impl std::future::Future<Output = Result<(), AppError>> + Send + 'static> {
    let (provider_name, host, port, probe_path) = match settings.ai_provider.as_str() {
        "ollama" => (
            "Ollama",
            settings.ollama_host.clone(),
            settings.ollama_port,
            "/api/tags",
        ),
        "lmstudio" => (
            "LM Studio",
            settings.lmstudio_host.clone(),
            settings.lmstudio_port,
            "/v1/models",
        ),
        "omlx" => (
            "oMLX",
            settings.omlx_host.clone(),
            settings.omlx_port,
            "/v1/models",
        ),
        _ => return None, // unknown provider: skip; caller will surface a config error
    };
    if is_loopback_host(&host) {
        return None;
    }
    let base_url = format!("http://{host}:{port}");
    Some(async move {
        probe_endpoint(
            ServiceKind::AiProvider,
            provider_name,
            &base_url,
            probe_path,
            None,
        )
        .await
    })
}

/// Returns `Some(future)` only if the user has configured a remote STT
/// endpoint (non-empty host); `None` if STT is fully local (default).
fn build_stt_probe(
    settings: &AppConfig,
) -> Option<impl std::future::Future<Output = Result<(), AppError>> + Send + 'static> {
    let host = settings.stt_remote_host.clone();
    if host.is_empty() || is_loopback_host(&host) {
        return None;
    }
    let port = settings.stt_remote_port;
    let base_url = format!("http://{host}:{port}");
    Some(async move {
        probe_endpoint(
            ServiceKind::RemoteStt,
            "Whisper STT",
            &base_url,
            "/v1/models",
            None,
        )
        .await
    })
}

/// True for loopback / empty hosts that should bypass preflight.
fn is_loopback_host(host: &str) -> bool {
    if host.is_empty() {
        return true;
    }
    let h = host.trim().to_ascii_lowercase();
    let h_stripped = h.trim_matches(|c| c == '[' || c == ']');
    if h_stripped == "localhost" || h_stripped == "::1" {
        return true;
    }
    h_stripped
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::settings::AppConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn settings_pointing_at(ai_provider: &str, host: &str, port: u16) -> AppConfig {
        let mut cfg = AppConfig::default();
        cfg.ai_provider = ai_provider.into();
        match ai_provider {
            "ollama" => {
                cfg.ollama_host = host.into();
                cfg.ollama_port = port;
            }
            "lmstudio" => {
                cfg.lmstudio_host = host.into();
                cfg.lmstudio_port = port;
            }
            "omlx" => {
                cfg.omlx_host = host.into();
                cfg.omlx_port = port;
            }
            _ => panic!("unknown ai_provider: {ai_provider}"),
        }
        cfg
    }

    #[tokio::test]
    async fn preflight_skips_loopback_ollama() {
        // 127.0.0.1:1 is definitely unreachable; if preflight tried to probe it
        // we'd see EndpointOffline. The skip rule means it never tries, so we
        // get Ok.
        let cfg = settings_pointing_at("ollama", "127.0.0.1", 1);
        let result = preflight_for_command(CommandKind::GenerateSoap, &cfg).await;
        assert!(result.is_ok(), "loopback should be skipped; got {result:?}");
    }

    #[tokio::test]
    async fn preflight_skips_localhost_lmstudio() {
        let cfg = settings_pointing_at("lmstudio", "localhost", 1);
        let result = preflight_for_command(CommandKind::GenerateSoap, &cfg).await;
        assert!(
            result.is_ok(),
            "localhost should be skipped; got {result:?}"
        );
    }

    #[tokio::test]
    async fn preflight_skips_empty_host_lmstudio() {
        // The Settings UI uses empty-host to mean "use the default (localhost)".
        // We treat empty host as loopback for skip purposes.
        let cfg = settings_pointing_at("lmstudio", "", 1);
        let result = preflight_for_command(CommandKind::GenerateSoap, &cfg).await;
        assert!(
            result.is_ok(),
            "empty host should be skipped; got {result:?}"
        );
    }

    #[tokio::test]
    async fn preflight_returns_endpoint_offline_for_unreachable_remote_ollama() {
        let mut cfg = AppConfig::default();
        cfg.ai_provider = "ollama".into();
        cfg.ollama_host = "192.0.2.1".into(); // RFC 5737 TEST-NET-1 — guaranteed unroutable
        cfg.ollama_port = 11434;

        let result = preflight_for_command(CommandKind::GenerateSoap, &cfg).await;
        let err = result.expect_err("unrouteable host must fail preflight");
        assert!(matches!(err, AppError::EndpointOffline { .. }));
    }

    #[tokio::test]
    async fn preflight_transcribe_skips_when_no_stt_remote_configured() {
        let mut cfg = AppConfig::default();
        cfg.stt_remote_host = "".into(); // not configured → use local whisper
        cfg.stt_remote_port = 8080;
        let result = preflight_for_command(CommandKind::Transcribe, &cfg).await;
        assert!(
            result.is_ok(),
            "transcribe with no remote STT configured should skip preflight; got {result:?}"
        );
    }

    #[test]
    fn is_loopback_host_recognizes_common_forms() {
        assert!(is_loopback_host(""));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LOCALHOST"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.42.0.1")); // 127/8
        assert!(is_loopback_host("::1"));
        assert!(!is_loopback_host("192.168.1.10"));
        assert!(!is_loopback_host("ollama.local"));
        assert!(!is_loopback_host("10.0.0.1"));
        assert!(
            is_loopback_host("[::1]"),
            "bracketed IPv6 loopback must be skipped"
        );
        assert!(
            is_loopback_host("[127.0.0.1]"),
            "bracketed IPv4 loopback also handled"
        );
        assert!(
            !is_loopback_host("[::ffff:192.168.1.10]"),
            "bracketed non-loopback IPv6 stays a probe"
        );
    }

    #[tokio::test]
    async fn probe_returns_ok_on_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"models\":[]}"))
            .mount(&server)
            .await;

        let result = probe_endpoint(
            ServiceKind::AiProvider,
            "Ollama",
            &server.uri(),
            "/api/tags",
            None,
        )
        .await;

        assert!(result.is_ok(), "200 response should be Ok; got {result:?}");
    }

    #[tokio::test]
    async fn probe_returns_ok_on_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let result = probe_endpoint(
            ServiceKind::AiProvider,
            "Ollama",
            &server.uri(),
            "/api/tags",
            None,
        )
        .await;

        assert!(
            result.is_ok(),
            "5xx response means server is reachable; got {result:?}"
        );
    }

    #[tokio::test]
    async fn probe_returns_connection_refused_when_no_server() {
        // Bind a TcpListener to get a free port, then drop it. The OS will
        // refuse any subsequent connection on that port (until something
        // else binds it). This is the canonical wiremock-free "connection
        // refused" pattern.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let base = format!("http://127.0.0.1:{port}");

        let result =
            probe_endpoint(ServiceKind::AiProvider, "Ollama", &base, "/api/tags", None).await;

        let err = result.expect_err("must error when port is closed");
        match err {
            AppError::EndpointOffline {
                service,
                reason,
                provider_name,
                ..
            } => {
                assert_eq!(service, ServiceKind::AiProvider);
                assert_eq!(reason, OfflineReason::ConnectionRefused);
                assert_eq!(provider_name, "Ollama");
            }
            other => panic!("expected EndpointOffline, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_returns_timeout_when_server_hangs() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
            .mount(&server)
            .await;

        let result = probe_endpoint(
            ServiceKind::AiProvider,
            "Ollama",
            &server.uri(),
            "/api/tags",
            None,
        )
        .await;

        let err = result.expect_err("must error when server exceeds timeout");
        match err {
            AppError::EndpointOffline { reason, .. } => {
                assert_eq!(reason, OfflineReason::Timeout);
            }
            other => panic!("expected EndpointOffline, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_returns_dns_failure_for_nonexistent_host() {
        // .invalid is reserved by RFC 2606 to always fail DNS.
        let result = probe_endpoint(
            ServiceKind::AiProvider,
            "Ollama",
            "http://nonexistent.invalid:11434",
            "/api/tags",
            None,
        )
        .await;

        let err = result.expect_err("must error for unresolvable host");
        match err {
            AppError::EndpointOffline { reason, .. } => {
                assert!(
                    matches!(
                        reason,
                        OfflineReason::DnsFailure | OfflineReason::ConnectionRefused
                    ),
                    "DNS failure preferred, but ConnectionRefused acceptable if the platform's \
                     resolver error doesn't include 'dns' in its source chain; got {reason:?}"
                );
            }
            other => panic!("expected EndpointOffline, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_sends_bearer_when_provided() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(wiremock::matchers::header("authorization", "Bearer s3cret"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let result = probe_endpoint(
            ServiceKind::RemoteStt,
            "Whisper STT",
            &server.uri(),
            "/v1/models",
            Some("s3cret"),
        )
        .await;

        assert!(
            result.is_ok(),
            "bearer-protected 200 should be Ok; got {result:?}"
        );
    }

    #[tokio::test]
    async fn probe_returns_other_for_non_connectivity_error() {
        // Pass a malformed URL — reqwest fails at the builder stage with an
        // error that is neither is_timeout() nor is_connect(), so
        // classify_reqwest_error returns None and probe_endpoint should
        // surface this as AppError::Other rather than misleadingly claim
        // the server is refusing connections.
        let result = probe_endpoint(
            ServiceKind::AiProvider,
            "Ollama",
            "not://a valid url with spaces",
            "/api/tags",
            None,
        )
        .await;

        let err = result.expect_err("malformed URL must error");
        match err {
            AppError::Other(msg) => {
                assert!(
                    msg.contains("Unexpected probe error"),
                    "message should signal unexpected, not connectivity; got: {msg}"
                );
            }
            AppError::EndpointOffline { .. } => {
                panic!("non-connectivity error must NOT be reported as EndpointOffline");
            }
            other => panic!("expected AppError::Other, got {other:?}"),
        }
    }
}
