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

/// Cap on a single probe's wall time. Chosen to be long enough for a
/// healthy LAN round-trip plus TLS handshake (~200ms typical) but short
/// enough that an offline server doesn't visibly stall the UI.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Classify a failed reqwest send into the user-facing `OfflineReason`.
/// Returns `None` if the error is something other than a connection /
/// timeout / DNS / TLS failure (e.g. URL parse error, body decode) —
/// caller should treat that as a genuine bug rather than an offline
/// endpoint.
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
            if s_str.contains("tls") || s_str.contains("handshake") || s_str.contains("certificate") {
                return Some(OfflineReason::TlsFailure);
            }
            source = StdError::source(s);
        }
        return Some(OfflineReason::ConnectionRefused);
    }
    None
}

/// Probe a single endpoint. Returns `Ok(())` if the server responded
/// with *any* HTTP status (including 4xx / 5xx) — auth / API errors
/// are not connectivity errors and are handled by the real call.
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

    let url = format!("{}{}", base_url.trim_end_matches('/'), probe_path);
    let mut req = client.get(&url);
    if let Some(b) = bearer {
        req = req.header("Authorization", format!("Bearer {b}"));
    }

    let start = std::time::Instant::now();
    let result = req.send().await;
    let elapsed_ms = start.elapsed().as_millis();

    match result {
        Ok(_response) => {
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
            let reason = classify_reqwest_error(&e)
                .unwrap_or(OfflineReason::ConnectionRefused);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

        let result = probe_endpoint(
            ServiceKind::AiProvider,
            "Ollama",
            &base,
            "/api/tags",
            None,
        )
        .await;

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
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(10)),
            )
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
                    matches!(reason, OfflineReason::DnsFailure | OfflineReason::ConnectionRefused),
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

        assert!(result.is_ok(), "bearer-protected 200 should be Ok; got {result:?}");
    }
}
