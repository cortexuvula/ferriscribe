//! Endpoint resolution and URL caching for remote STT providers.

use std::time::{Duration, Instant};

use medical_core::error::{AppError, AppResult, OfflineReason, ServiceKind};
use medical_core::types::{http_url, RemoteEndpoint};

const CACHE_TTL: Duration = Duration::from_secs(30);

/// Cached result of endpoint resolution.
pub struct ResolvedCache {
    pub url: String,
    pub resolved_at: Instant,
}

/// Resolve the current base URL from a RemoteEndpoint or fall back to static base_url.
///
/// If an endpoint is configured, probes LAN then Tailscale addresses with a 30-second
/// cache. Returns the first reachable address or an EndpointOffline error if both fail.
pub async fn current_base_url(
    endpoint: &Option<RemoteEndpoint>,
    base_url: &str,
    cache: &mut Option<ResolvedCache>,
) -> AppResult<String> {
    if let Some(ep) = endpoint {
        // Check cache validity
        if let Some(c) = cache.as_ref() {
            if c.resolved_at.elapsed() < CACHE_TTL {
                return Ok(c.url.clone());
            }
        }

        // Resolve endpoint (probe LAN then Tailscale)
        let url = ep.resolve_base_url().await.ok_or_else(|| {
            // Both probes failed — pick LAN as representative endpoint
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

        // Update cache
        *cache = Some(ResolvedCache {
            url: url.clone(),
            resolved_at: Instant::now(),
        });

        return Ok(url);
    }

    Ok(base_url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_static_url_when_no_endpoint() {
        let mut cache = None;
        let result = current_base_url(&None, "http://localhost:8080", &mut cache).await;
        assert_eq!(result.unwrap(), "http://localhost:8080");
        assert!(cache.is_none());
    }
}
