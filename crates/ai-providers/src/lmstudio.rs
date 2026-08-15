//! LM Studio provider — wraps [`OpenAiCompatibleClient`] against a local LM Studio server.
//!
//! # Default configuration
//!
//! - Base URL: `http://localhost:1234/v1`
//! - Fallback model: `default`
//! - Provider name: `"LM Studio"`
//!
//! # Endpoint resolution
//!
//! When a [`RemoteEndpoint`] is configured (via [`new_with_endpoint`] or
//! [`set_endpoint`]), the provider probes LAN then Tailscale addresses with
//! a 30-second cache. This supports multi-network deployments where the
//! LM Studio server runs on a LAN machine but the clinician connects via
//! Tailscale when working remotely.
//!
//! [`new_with_endpoint`]: LmStudioProvider::new_with_endpoint
//! [`set_endpoint`]: LmStudioProvider::set_endpoint

use async_trait::async_trait;
use futures_core::Stream;
use reqwest::Client;
use tokio::sync::{Mutex, RwLock};

use medical_core::{
    error::{AppError, AppResult},
    traits::AiProvider,
    types::endpoint::http_url,
    types::{
        CompletionRequest, CompletionResponse, ModelInfo, RemoteEndpoint, StreamChunk,
        ToolCompletionResponse, ToolDef,
    },
};

use crate::http_client::RetryConfig;
use crate::openai_compat::OpenAiCompatibleClient;

// ──────────────────────────────────────────────────────────────────────────────
// 30-second resolved-URL cache for RemoteEndpoint resolution
// ──────────────────────────────────────────────────────────────────────────────

struct ResolvedCache {
    url: String,
    resolved_at: std::time::Instant,
}

const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

// ──────────────────────────────────────────────────────────────────────────────

/// LM Studio provider implementing the [`AiProvider`] trait.
///
/// Wraps an [`OpenAiCompatibleClient`] pointed at an LM Studio server. Supports
/// optional [`RemoteEndpoint`] for LAN/Tailscale resolution, bearer-token
/// authentication, and configurable retry policy.
///
/// # Thread safety
///
/// The inner client is wrapped in a `tokio::sync::Mutex` — concurrent
/// requests are serialized. The endpoint and URL cache use `RwLock` for
/// read-heavy access patterns.
///
/// [`AiProvider`]: medical_core::traits::AiProvider
pub struct LmStudioProvider {
    /// Static base_url used when no RemoteEndpoint is configured.
    static_base_url: String,
    client: Mutex<OpenAiCompatibleClient>,
    /// Optional LAN/Tailscale endpoint; takes precedence over `static_base_url`.
    endpoint: RwLock<Option<RemoteEndpoint>>,
    url_cache: Mutex<Option<ResolvedCache>>,
}

impl LmStudioProvider {
    /// Create a new LM Studio provider.
    ///
    /// `host` defaults to `http://localhost:1234` when `None`.
    /// `bearer` is an optional bearer token for auth-proxied remote connections.
    /// `policy` controls retry behavior for inner HTTP calls.
    pub fn new(
        host: Option<&str>,
        allow_public: bool,
        bearer: Option<String>,
        policy: RetryConfig,
    ) -> AppResult<Self> {
        let base = host.unwrap_or("http://localhost:1234");
        medical_core::endpoint_policy::validate_url(base, allow_public)
            .map_err(|e| AppError::invalid_endpoint_for(e, "lmstudio_host"))?;
        let base_url = format!("{base}/v1");
        let http = Client::builder()
            .pool_max_idle_per_host(5)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| {
                AppError::ai_provider(format!("Failed to build LM Studio HTTP client: {e}"))
            })?;
        Ok(Self {
            static_base_url: base_url.clone(),
            client: Mutex::new(OpenAiCompatibleClient::new_with_bearer_and_name(
                http,
                base_url,
                policy,
                bearer,
                "LM Studio",
            )),
            endpoint: RwLock::new(None),
            url_cache: Mutex::new(None),
        })
    }

    /// Create a new LM Studio provider with a `RemoteEndpoint` pre-configured.
    ///
    /// Usable in synchronous initialization code (no running async runtime required).
    pub fn new_with_endpoint(
        host: Option<&str>,
        allow_public: bool,
        bearer: Option<String>,
        policy: RetryConfig,
        ep: Option<RemoteEndpoint>,
    ) -> AppResult<Self> {
        let base = host.unwrap_or("http://localhost:1234");
        medical_core::endpoint_policy::validate_url(base, allow_public)
            .map_err(|e| AppError::invalid_endpoint_for(e, "lmstudio_host"))?;
        let base_url = format!("{base}/v1");
        let http = Client::builder()
            .pool_max_idle_per_host(5)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| {
                AppError::ai_provider(format!("Failed to build LM Studio HTTP client: {e}"))
            })?;
        Ok(Self {
            static_base_url: base_url.clone(),
            client: Mutex::new(OpenAiCompatibleClient::new_with_bearer_and_name(
                http,
                base_url,
                policy,
                bearer,
                "LM Studio",
            )),
            endpoint: RwLock::new(ep),
            url_cache: Mutex::new(None),
        })
    }

    /// Override the remote endpoint used for LAN/Tailscale resolution.
    /// Invalidates the URL cache, replaces the endpoint, and propagates the
    /// endpoint's bearer into the inner HTTP client so subsequent requests
    /// authenticate with the current token. Without this last step, an
    /// in-session Unpair → Pair leaves the inner client carrying the bearer
    /// it had at construction time — a 401 source if the office admin
    /// revoked the previous client entry before re-pairing.
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
                        .map_err(|err| {
                            AppError::invalid_endpoint_for(err, format!("lmstudio_host.{label}"))
                        })?;
                }
            }
        }
        let new_bearer = ep.as_ref().and_then(|e| e.bearer.clone());
        *self.url_cache.lock().await = None;
        *self.endpoint.write().await = ep;
        self.client.lock().await.bearer = new_bearer;
        Ok(())
    }

    /// Resolve the current base URL (with the `/v1` suffix).
    /// If a RemoteEndpoint is configured, probe LAN then Tailscale with a 30s
    /// cache.  Falls back to the static URL when no endpoint is set.
    ///
    /// Lock ordering: `set_endpoint` takes `url_cache` then `endpoint` (write).
    /// To avoid an AB-BA inversion, this method clones the endpoint out of the
    /// read guard and drops the guard *before* acquiring the cache lock or
    /// probing the network.
    async fn current_base_url(&self) -> AppResult<String> {
        let ep = {
            let guard = self.endpoint.read().await;
            guard.clone()
        };
        if let Some(ep) = ep {
            // Fast path: cache hit under a short-lived lock, no probe.
            {
                let cache = self.url_cache.lock().await;
                if let Some(c) = cache.as_ref()
                    && c.resolved_at.elapsed() < CACHE_TTL
                {
                    return Ok(c.url.clone());
                }
            }
            // Slow path: probe the network with no locks held.
            let resolved = ep.resolve_base_url().await.ok_or_else(|| {
                use medical_core::error::{OfflineReason, ServiceKind};
                // RemoteEndpoint probed LAN then Tailscale and both failed. Pick
                // the LAN URL as the representative endpoint; if LAN isn't set,
                // fall back to Tailscale; if neither is set, this is a config
                // error and "(unresolved)" surfaces clearly in the dialog.
                let endpoint = ep
                    .lan
                    .as_deref()
                    .map(|h| http_url(h, ep.port))
                    .or_else(|| ep.tailscale.as_deref().map(|h| http_url(h, ep.port)))
                    .unwrap_or_else(|| "(unresolved)".into());
                AppError::EndpointOffline {
                    service: ServiceKind::AiProvider,
                    endpoint,
                    reason: OfflineReason::Timeout,
                    provider_name: "LM Studio".into(),
                }
            })?;
            let url = format!("{}/v1", resolved);
            *self.url_cache.lock().await = Some(ResolvedCache {
                url: url.clone(),
                resolved_at: std::time::Instant::now(),
            });
            return Ok(url);
        }
        // No endpoint — use static URL.
        Ok(self.static_base_url.clone())
    }

    /// Ensure the inner client's base_url matches the current resolved URL.
    async fn sync_client_url(
        &self,
    ) -> AppResult<tokio::sync::MutexGuard<'_, OpenAiCompatibleClient>> {
        let url = self.current_base_url().await?;
        let mut client = self.client.lock().await;
        client.base_url = url;
        Ok(client)
    }
}

#[async_trait]
impl AiProvider for LmStudioProvider {
    fn name(&self) -> &str {
        "lmstudio"
    }

    async fn available_models(&self) -> AppResult<Vec<ModelInfo>> {
        let client = self.sync_client_url().await?;
        // LM Studio supports the OpenAI-compatible /v1/models endpoint
        if let Ok(ids) = client.list_models().await {
            let mut models: Vec<ModelInfo> = ids
                .into_iter()
                .map(|id| ModelInfo {
                    name: id.clone(),
                    id,
                    provider: "lmstudio".into(),
                    max_tokens: 8_192,
                    supports_tools: false,
                    supports_streaming: true,
                })
                .collect();
            if !models.is_empty() {
                models.sort_by_key(|m| m.id.clone());
                return Ok(models);
            }
        }

        // Fallback
        Ok(vec![ModelInfo {
            id: "default".into(),
            name: "default".into(),
            provider: "lmstudio".into(),
            max_tokens: 8_192,
            supports_tools: false,
            supports_streaming: true,
        }])
    }

    async fn complete(&self, request: CompletionRequest) -> AppResult<CompletionResponse> {
        let client = self.sync_client_url().await?;
        client.complete(&request).await
    }

    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> AppResult<Box<dyn Stream<Item = AppResult<StreamChunk>> + Send + Unpin>> {
        let client = self.sync_client_url().await?;
        let pinned = client.complete_stream(&request).await?;
        Ok(Box::new(pinned))
    }

    async fn complete_with_tools(
        &self,
        request: CompletionRequest,
        tools: Vec<ToolDef>,
    ) -> AppResult<ToolCompletionResponse> {
        let client = self.sync_client_url().await?;
        client.complete_with_tools(&request, tools).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_with_default_host() {
        let p = LmStudioProvider::new(None, false, None, RetryConfig::default())
            .expect("build default provider");
        assert_eq!(p.static_base_url, "http://localhost:1234/v1");
    }

    #[test]
    fn creates_with_custom_host() {
        let p = LmStudioProvider::new(
            Some("http://192.168.1.10:1234"),
            false,
            None,
            RetryConfig::default(),
        )
        .expect("build custom provider");
        assert_eq!(p.static_base_url, "http://192.168.1.10:1234/v1");
    }

    #[test]
    fn stores_bearer_token() {
        let _p = LmStudioProvider::new(None, false, Some("tok_lms".into()), RetryConfig::default())
            .expect("build provider with bearer");
        // Bearer is stored on the inner client (tested via integration calls).
    }

    #[tokio::test]
    async fn set_endpoint_clears_cache() {
        let p = LmStudioProvider::new(None, false, None, RetryConfig::default()).expect("build");
        *p.url_cache.lock().await = Some(ResolvedCache {
            url: "http://stale:9999/v1".to_string(),
            resolved_at: std::time::Instant::now(),
        });
        p.set_endpoint(None, false).await.expect("clear endpoint");
        assert!(p.url_cache.lock().await.is_none());
    }

    #[tokio::test]
    async fn current_base_url_returns_static_when_no_endpoint() {
        let p = LmStudioProvider::new(
            Some("http://192.168.1.42:1234"),
            false,
            None,
            RetryConfig::default(),
        )
        .expect("build");
        let url = p.current_base_url().await.expect("url");
        assert_eq!(url, "http://192.168.1.42:1234/v1");
    }

    #[tokio::test]
    async fn current_base_url_caches_for_30s() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let p = LmStudioProvider::new(None, false, None, RetryConfig::default()).expect("build");
        p.set_endpoint(
            Some(RemoteEndpoint {
                lan: Some("127.0.0.1".to_string()),
                tailscale: None,
                port,
                bearer: None,
            }),
            false,
        )
        .await
        .expect("set endpoint");

        let url1 = p.current_base_url().await.expect("first resolve");
        assert!(url1.contains(&port.to_string()));

        drop(listener);

        // Cache should still return the URL without re-probing.
        let url2 = p.current_base_url().await.expect("cached resolve");
        assert_eq!(url1, url2);
    }

    #[test]
    fn new_blocks_public_endpoint_by_default() {
        let result = LmStudioProvider::new(
            Some("http://api.openai.com/v1"),
            /* allow_public */ false,
            None,
            RetryConfig::default(),
        );
        assert!(matches!(
            result,
            Err(medical_core::error::AppError::InvalidEndpoint {
                field, ..
            }) if field == "lmstudio_host"
        ));
    }

    #[test]
    fn new_accepts_public_endpoint_when_allow_public() {
        let result = LmStudioProvider::new(
            Some("http://api.openai.com/v1"),
            /* allow_public */ true,
            None,
            RetryConfig::default(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn new_accepts_local_endpoints_with_default_allow_public() {
        for host in [
            None,
            Some("http://localhost:1234"),
            Some("http://192.168.1.42:1234"),
            Some("http://100.64.0.1:1234"),
            Some("http://clinic.local:1234"),
        ] {
            let r = LmStudioProvider::new(
                host,
                /* allow_public */ false,
                None,
                RetryConfig::default(),
            );
            assert!(r.is_ok(), "expected Ok for {host:?}");
        }
    }

    #[tokio::test]
    async fn set_endpoint_rejects_public_lan_address() {
        let p = LmStudioProvider::new(None, false, None, RetryConfig::default()).expect("build");
        let bad = medical_core::types::RemoteEndpoint {
            lan: Some("api.openai.com".into()),
            tailscale: None,
            port: 1234,
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
        let p = LmStudioProvider::new(None, false, None, RetryConfig::default()).expect("build");
        let good = medical_core::types::RemoteEndpoint {
            lan: Some("192.168.1.42".into()),
            tailscale: Some("100.64.0.1".into()),
            port: 1234,
            bearer: None,
        };
        assert!(p.set_endpoint(Some(good), false).await.is_ok());
    }
}

#[cfg(test)]
mod offline_tests {
    use super::*;
    use medical_core::{
        error::{AppError, OfflineReason, ServiceKind},
        types::{Message, MessageContent, Role},
    };

    fn dead_port() -> u16 {
        // Bind then immediately drop to get a free port that is guaranteed closed.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    fn minimal_request(model: &str) -> CompletionRequest {
        CompletionRequest {
            model: model.to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("hi".into()),
                tool_calls: vec![],
            }],
            temperature: Some(0.0),
            max_tokens: Some(10),
            system_prompt: None,
        }
    }

    /// Test the RemoteEndpoint resolution path: both LAN and Tailscale are
    /// unreachable, so `resolve_base_url()` returns `None`, and
    /// `current_base_url()` must emit `EndpointOffline`.
    #[tokio::test]
    async fn resolve_failure_returns_endpoint_offline() {
        let port = dead_port();

        let p = LmStudioProvider::new(None, false, None, RetryConfig::default()).expect("build");
        p.set_endpoint(
            Some(RemoteEndpoint {
                lan: Some("127.0.0.1".to_string()),
                tailscale: None,
                port,
                bearer: None,
            }),
            false,
        )
        .await
        .expect("set endpoint");

        let err = p.current_base_url().await.unwrap_err();
        match err {
            AppError::EndpointOffline {
                service,
                reason,
                provider_name,
                endpoint,
            } => {
                assert_eq!(service, ServiceKind::AiProvider);
                assert_eq!(reason, OfflineReason::Timeout);
                assert_eq!(provider_name, "LM Studio");
                assert!(
                    endpoint.contains("127.0.0.1"),
                    "endpoint should carry host; got {endpoint:?}"
                );
            }
            other => panic!("expected EndpointOffline, got {other:?}"),
        }
    }

    /// Test the downstream HTTP-send path (race condition / static URL):
    /// provider is pointed at a dead port via static URL, so `complete()`
    /// hits a connection-refused during the actual HTTP send.
    #[tokio::test]
    async fn complete_returns_endpoint_offline_when_host_refused() {
        let port = dead_port();
        let host = format!("http://127.0.0.1:{port}");

        // No endpoint set — uses static_base_url pointing at dead port.
        let policy = RetryConfig {
            max_retries: 0,
            ..RetryConfig::default()
        };
        let p = LmStudioProvider::new(Some(&host), false, None, policy).expect("build");

        let req = minimal_request("default");
        let err = p.complete(req).await.unwrap_err();
        match err {
            AppError::EndpointOffline {
                service,
                reason,
                provider_name,
                endpoint,
            } => {
                assert_eq!(service, ServiceKind::AiProvider);
                assert_eq!(reason, OfflineReason::ConnectionRefused);
                assert_eq!(provider_name, "LM Studio");
                assert!(
                    endpoint.contains("127.0.0.1"),
                    "endpoint should carry host; got {endpoint:?}"
                );
            }
            other => panic!("expected EndpointOffline, got {other:?}"),
        }
    }
}
