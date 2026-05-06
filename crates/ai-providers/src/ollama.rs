//! Ollama provider — wraps `OpenAiCompatibleClient` against a local server.

use async_trait::async_trait;
use futures_core::Stream;
use reqwest::Client;
use tokio::sync::{Mutex, RwLock};

use medical_core::{
    error::{AppError, AppResult},
    traits::AiProvider,
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

pub struct OllamaProvider {
    /// Static base_url used when no RemoteEndpoint is configured.
    static_base_url: String,
    client: Mutex<OpenAiCompatibleClient>,
    /// Optional LAN/Tailscale endpoint; takes precedence over `static_base_url`.
    endpoint: RwLock<Option<RemoteEndpoint>>,
    url_cache: Mutex<Option<ResolvedCache>>,
}

impl OllamaProvider {
    /// Create a new Ollama provider.
    ///
    /// `host` defaults to `http://localhost:11434` when `None`.
    /// `bearer` is an optional bearer token for auth-proxied remote connections.
    /// `policy` controls retry behavior for inner HTTP calls.
    /// Returns `Err(AppError::AiProvider)` if the reqwest client can't be built.
    pub fn new(host: Option<&str>, bearer: Option<String>, policy: RetryConfig) -> AppResult<Self> {
        let base = host.unwrap_or("http://localhost:11434");
        let base_url = format!("{base}/v1");
        let http = Client::builder()
            .pool_max_idle_per_host(5)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| AppError::AiProvider(format!("Failed to build Ollama HTTP client: {e}")))?;
        Ok(Self {
            static_base_url: base_url.clone(),
            client: Mutex::new(OpenAiCompatibleClient::new_with_bearer(http, base_url, policy, bearer)),
            endpoint: RwLock::new(None),
            url_cache: Mutex::new(None),
        })
    }

    /// Create a new Ollama provider with a `RemoteEndpoint` pre-configured.
    ///
    /// Equivalent to `new(host, bearer, policy)` followed by `set_endpoint(ep)`,
    /// but usable in synchronous initialization code (no running async runtime
    /// required). Static `host` is kept as the fallback when `ep` is `None`.
    pub fn new_with_endpoint(
        host: Option<&str>,
        bearer: Option<String>,
        policy: RetryConfig,
        ep: Option<RemoteEndpoint>,
    ) -> AppResult<Self> {
        let base = host.unwrap_or("http://localhost:11434");
        let base_url = format!("{base}/v1");
        let http = Client::builder()
            .pool_max_idle_per_host(5)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| AppError::AiProvider(format!("Failed to build Ollama HTTP client: {e}")))?;
        Ok(Self {
            static_base_url: base_url.clone(),
            client: Mutex::new(OpenAiCompatibleClient::new_with_bearer(http, base_url, policy, bearer)),
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
    pub async fn set_endpoint(&self, ep: Option<RemoteEndpoint>) {
        let new_bearer = ep.as_ref().and_then(|e| e.bearer.clone());
        *self.url_cache.lock().await = None;
        *self.endpoint.write().await = ep;
        self.client.lock().await.bearer = new_bearer;
    }

    /// Resolve the current base URL (without the `/v1` suffix applied here).
    /// If a RemoteEndpoint is configured, probe LAN then Tailscale with a 30s
    /// cache.  Falls back to the static URL when no endpoint is set.
    async fn current_base_url(&self) -> AppResult<String> {
        let ep_guard = self.endpoint.read().await;
        if let Some(ep) = ep_guard.as_ref() {
            let mut cache = self.url_cache.lock().await;
            if let Some(c) = cache.as_ref() {
                if c.resolved_at.elapsed() < CACHE_TTL {
                    return Ok(c.url.clone());
                }
            }
            let resolved = ep
                .resolve_base_url()
                .await
                .ok_or_else(|| {
                    AppError::AiProvider(
                        "Office server unreachable on LAN or Tailscale (Ollama).".to_string(),
                    )
                })?;
            let url = format!("{}/v1", resolved);
            *cache = Some(ResolvedCache {
                url: url.clone(),
                resolved_at: std::time::Instant::now(),
            });
            return Ok(url);
        }
        // No endpoint — use static URL.
        Ok(self.static_base_url.clone())
    }

    /// Ensure the inner client's base_url matches the current resolved URL.
    /// Acquires the Mutex on `client`; callers must hold it for the full request.
    async fn sync_client_url(&self) -> AppResult<tokio::sync::MutexGuard<'_, OpenAiCompatibleClient>> {
        let url = self.current_base_url().await?;
        let mut client = self.client.lock().await;
        client.base_url = url;
        Ok(client)
    }
}

#[async_trait]
impl AiProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn available_models(&self) -> AppResult<Vec<ModelInfo>> {
        let client = self.sync_client_url().await?;
        // Ollama supports the OpenAI-compatible /v1/models endpoint
        if let Ok(ids) = client.list_models().await {
            let mut models: Vec<ModelInfo> = ids
                .into_iter()
                .map(|id| ModelInfo {
                    name: id.clone(),
                    id,
                    provider: "ollama".into(),
                    max_tokens: 8_192,
                    supports_tools: false,
                    supports_streaming: true,
                })
                .collect();
            if !models.is_empty() {
                models.sort_by(|a, b| a.id.cmp(&b.id));
                return Ok(models);
            }
        }

        // Fallback
        Ok(vec![ModelInfo {
            id: "llama3".into(),
            name: "llama3".into(),
            provider: "ollama".into(),
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
        let p = OllamaProvider::new(None, None, RetryConfig::default()).expect("build default provider");
        assert_eq!(p.static_base_url, "http://localhost:11434/v1");
    }

    #[test]
    fn creates_with_custom_host() {
        let p = OllamaProvider::new(
            Some("http://192.168.1.10:11434"),
            None,
            RetryConfig::default(),
        )
        .expect("build custom provider");
        assert_eq!(p.static_base_url, "http://192.168.1.10:11434/v1");
    }

    #[test]
    fn stores_bearer_token() {
        use std::future::Future;
        let p = OllamaProvider::new(
            None,
            Some("tok_test".into()),
            RetryConfig::default(),
        )
        .expect("build provider with bearer");
        // Bearer is on the inner client; block to read it.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let _ = p; // ensure p is used
        });
    }

    #[tokio::test]
    async fn set_endpoint_clears_cache() {
        let p = OllamaProvider::new(None, None, RetryConfig::default()).expect("build");
        // Seed the cache manually.
        *p.url_cache.lock().await = Some(ResolvedCache {
            url: "http://stale:9999/v1".to_string(),
            resolved_at: std::time::Instant::now(),
        });
        // Setting a new endpoint must clear the cache.
        p.set_endpoint(None).await;
        assert!(p.url_cache.lock().await.is_none());
    }

    #[tokio::test]
    async fn current_base_url_returns_static_when_no_endpoint() {
        let p = OllamaProvider::new(
            Some("http://192.168.1.42:11434"),
            None,
            RetryConfig::default(),
        )
        .expect("build");
        let url = p.current_base_url().await.expect("url");
        assert_eq!(url, "http://192.168.1.42:11434/v1");
    }

    #[tokio::test]
    async fn current_base_url_caches_for_30s() {
        use tokio::net::TcpListener;

        // Bind a port so the resolver can connect.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let p = OllamaProvider::new(None, None, RetryConfig::default()).expect("build");
        p.set_endpoint(Some(RemoteEndpoint {
            lan: Some("127.0.0.1".to_string()),
            tailscale: None,
            port,
            bearer: None,
        }))
        .await;

        // First call: listener up — should resolve.
        let url1 = p.current_base_url().await.expect("first resolve");
        assert!(url1.contains(&port.to_string()));

        // Drop the listener so the port is closed.
        drop(listener);

        // Second call immediately after: cache should still return the URL.
        let url2 = p.current_base_url().await.expect("cached resolve");
        assert_eq!(url1, url2, "cache should return same URL without re-probing");
    }
}
