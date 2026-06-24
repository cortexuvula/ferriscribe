//! Endpoint resolution and URL caching for remote STT providers.

use std::time::{Duration, Instant};

use medical_core::error::{AppError, AppResult, OfflineReason, ServiceKind};
use medical_core::types::{RemoteEndpoint, http_url};

const CACHE_TTL: Duration = Duration::from_secs(30);

/// Cached result of endpoint resolution.
///
/// Stored inside `RemoteSttProvider` behind a `Mutex`. The cache is invalidated
/// when `set_endpoint()` is called or when `resolved_at` exceeds `CACHE_TTL`.
pub struct ResolvedCache {
    /// The resolved base URL (e.g. `http://192.168.1.42:8080`).
    pub url: String,
    /// When this URL was resolved. Compared against `CACHE_TTL` (30 seconds).
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
        if let Some(c) = cache.as_ref()
            && c.resolved_at.elapsed() < CACHE_TTL
        {
            return Ok(c.url.clone());
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
    async fn returns_cached_url_within_ttl() {
        let mut cache = Some(ResolvedCache {
            url: "http://cached.example.com:8080".to_string(),
            resolved_at: Instant::now(),
        });

        // Even with an endpoint configured, should return cached URL
        let endpoint = Some(RemoteEndpoint {
            lan: Some("lan.example.com".to_string()),
            tailscale: None,
            port: 8080,
            bearer: None,
        });

        let result = current_base_url(&endpoint, "http://fallback.com:8080", &mut cache).await;
        assert_eq!(result.unwrap(), "http://cached.example.com:8080");
    }

    #[tokio::test]
    async fn resolves_fresh_url_when_cache_expired() {
        use std::net::TcpListener;

        // Bind a local port so the probe succeeds
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let endpoint = Some(RemoteEndpoint {
            lan: Some("127.0.0.1".to_string()),
            tailscale: None,
            port,
            bearer: None,
        });

        // Pre-populate cache with expired entry pointing to different URL
        let mut cache = Some(ResolvedCache {
            url: "http://old.example.com:8080".to_string(),
            resolved_at: Instant::now() - Duration::from_secs(31), // Expired
        });

        // Should ignore expired cache and resolve fresh
        let result = current_base_url(&endpoint, "http://fallback.com:8080", &mut cache)
            .await
            .unwrap();
        assert_eq!(
            result,
            format!("http://127.0.0.1:{}", port),
            "should resolve fresh, not use expired cache"
        );

        // Cache should be updated with new URL
        let updated = cache.unwrap();
        assert_eq!(updated.url, result);
        assert!(
            updated.resolved_at.elapsed() < Duration::from_secs(1),
            "cache timestamp should be fresh"
        );
    }

    #[tokio::test]
    async fn current_base_url_returns_static_when_no_endpoint() {
        let mut cache = None;
        let result = current_base_url(&None, "http://localhost:8080", &mut cache).await;
        assert_eq!(result.unwrap(), "http://localhost:8080");
    }

    #[tokio::test]
    async fn current_base_url_caches_for_30s() {
        use std::net::TcpListener;

        // Bind a local port so the probe succeeds
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let endpoint = Some(RemoteEndpoint {
            lan: Some("127.0.0.1".to_string()),
            tailscale: None,
            port,
            bearer: None,
        });

        let mut cache = None;

        // First call resolves and caches
        let url1 = current_base_url(&endpoint, "http://fallback.com:8080", &mut cache)
            .await
            .unwrap();
        assert_eq!(url1, format!("http://127.0.0.1:{}", port));
        assert!(cache.is_some());

        // Drop the listener (close the port)
        drop(listener);

        // Second call should return cached URL (port is closed, so fresh probe would fail)
        let url2 = current_base_url(&endpoint, "http://fallback.com:8080", &mut cache)
            .await
            .unwrap();
        assert_eq!(url1, url2, "cache should serve URL even after port closes");
    }
}
