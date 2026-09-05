//! Shared implementation for FerriScribe's local OpenAI-compatible providers.
//!
//! [`OllamaProvider`](crate::ollama::OllamaProvider),
//! [`LmStudioProvider`](crate::lmstudio::LmStudioProvider), and
//! [`OmlxProvider`](crate::omlx::OmlxProvider) are thin wrappers around
//! [`LocalOpenAiProvider`]; everything they have in common — endpoint
//! validation, LAN/Tailscale resolution with a 30-second cache, bearer-token
//! propagation, thinking-model control, and the [`AiProvider`] surface over
//! [`OpenAiCompatibleClient`] — lives here, parameterized by a static
//! [`ProviderMeta`].
//!
//! # Endpoint resolution
//!
//! When a [`RemoteEndpoint`] is configured (via [`new_with_endpoint`] or
//! [`set_endpoint`]), the provider probes LAN then Tailscale addresses with
//! a 30-second cache. This supports multi-network deployments where the
//! inference server runs on a LAN machine but the clinician connects via
//! Tailscale when working remotely.
//!
//! # Thread safety
//!
//! The inner client is wrapped in a `tokio::sync::Mutex` — concurrent
//! requests are serialized. The endpoint and URL cache use `RwLock` for
//! read-heavy access patterns.
//!
//! [`new_with_endpoint`]: LocalOpenAiProvider::new_with_endpoint
//! [`set_endpoint`]: LocalOpenAiProvider::set_endpoint
//! [`AiProvider`]: medical_core::traits::AiProvider

use async_trait::async_trait;
use futures_core::Stream;
use reqwest::Client;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, RwLock};

use medical_core::{
    error::{AppError, AppResult},
    traits::AiProvider,
    types::endpoint::http_url,
    types::{
        CompletionRequest, CompletionResponse, Message, MessageContent, ModelInfo,
        REASONING_EFFORT_DISABLE, RemoteEndpoint, Role, StreamChunk, ToolCompletionResponse,
        ToolDef,
    },
};

use crate::http_client::RetryConfig;
use crate::openai_compat::OpenAiCompatibleClient;

// ─────────────────────────────────────────────────────────────────────────────
// 30-second resolved-URL cache for RemoteEndpoint resolution
// ─────────────────────────────────────────────────────────────────────────────

struct ResolvedCache {
    url: String,
    resolved_at: std::time::Instant,
}

const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

// ─────────────────────────────────────────────────────────────────────────────

/// How a provider expresses "skip the reasoning phase" on the wire when the
/// user has disabled thinking. Each strategy exists because its server
/// accepts exactly one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingControl {
    /// Force `reasoning_effort: "none"` on every request. The value is
    /// verified against Ollama 0.32.14, which 400-rejects `"off"` — its
    /// allowlist is `minimal|low|medium|high|xhigh|ultra|max|none`, with
    /// `"none"` as the disable value. (Ollama's native-API `think: false`
    /// does NOT work on `/v1`.)
    ReasoningEffortNone,
    /// Append an assistant prefill message containing a pre-closed
    /// `<think>` block — the model sees that thinking has "happened" and
    /// continues straight to the answer. Used by servers that silently drop
    /// every API-level thinking parameter (`enable_thinking`,
    /// `reasoning_effort`, `chat_template_kwargs`) but render proper Jinja
    /// chat templates: LM Studio (as of 0.4.16+) and oMLX. The model-side
    /// equivalent of the recommended server-side fix of editing the model's
    /// prompt template with `{%- set enable_thinking = false %}`, which
    /// needs no FerriScribe code and also disables thinking for every other
    /// client.
    AssistantPrefill,
}

/// The assistant-prefill payload used to suppress the reasoning/"thinking"
/// phase for [`ThinkingControl::AssistantPrefill`] providers: an
/// already-opened-and-closed empty `<think>` block.
const THINKING_PREFILL: &str = "<think>\n\n</think>\n\n";

/// Static identity and policy knobs that distinguish the local
/// OpenAI-compatible providers. Each provider crate module exports one
/// `static META` and delegates to [`LocalOpenAiProvider`].
#[derive(Debug)]
pub struct ProviderMeta {
    /// Wire id returned by [`AiProvider::name`] and stored in
    /// `AppConfig.ai_provider` / `ModelInfo.provider`.
    pub id: &'static str,
    /// Human-readable name for errors and logs ("Ollama", "LM Studio", "oMLX").
    pub display: &'static str,
    /// Base URL used when no host is configured (no `/v1` suffix).
    pub default_base: &'static str,
    /// `AppError::InvalidEndpoint` field name for endpoint-policy failures
    /// (e.g. `"ollama_host"`) — the frontend maps this to the offending input.
    pub err_field: &'static str,
    /// How `thinking_disabled` is expressed on the wire.
    pub thinking: ThinkingControl,
}

/// The shared local OpenAI-compatible provider. Implements the full
/// [`AiProvider`] trait; the per-provider types wrap this and add nothing
/// but their [`ProviderMeta`].
///
/// Wraps an [`OpenAiCompatibleClient`]. Supports optional [`RemoteEndpoint`]
/// for LAN/Tailscale resolution, bearer-token authentication, and a
/// configurable retry policy.
pub struct LocalOpenAiProvider {
    meta: &'static ProviderMeta,
    /// Static base_url used when no RemoteEndpoint is configured.
    static_base_url: String,
    client: Mutex<OpenAiCompatibleClient>,
    /// Optional LAN/Tailscale endpoint; takes precedence over `static_base_url`.
    endpoint: RwLock<Option<RemoteEndpoint>>,
    url_cache: Mutex<Option<ResolvedCache>>,
    /// When `true`, [`apply_thinking_control`] suppresses the reasoning
    /// ("thinking") phase per [`ProviderMeta::thinking`]. Set via
    /// [`set_thinking_disabled`](LocalOpenAiProvider::set_thinking_disabled)
    /// from AppConfig; changes only take effect when `reinit_providers`
    /// rebuilds the provider.
    thinking_disabled: AtomicBool,
}

impl LocalOpenAiProvider {
    /// Create a new provider.
    ///
    /// `host` defaults to [`ProviderMeta::default_base`] when `None`.
    /// `bearer` is an optional bearer token for auth-proxied remote connections.
    /// `policy` controls retry behavior for inner HTTP calls.
    ///
    /// **`allow_public` is captured at construction, not stored.** If the user
    /// changes `allow_public_endpoint` in settings, the provider keeps using
    /// the policy it was built with until `reinit_providers` reconstructs it.
    /// The frontend must call `reinit_providers` after changing that setting.
    pub fn new(
        meta: &'static ProviderMeta,
        host: Option<&str>,
        allow_public: bool,
        bearer: Option<String>,
        policy: RetryConfig,
    ) -> AppResult<Self> {
        Self::new_with_endpoint(meta, host, allow_public, bearer, policy, None)
    }

    /// Create a new provider with a `RemoteEndpoint` pre-configured.
    ///
    /// Equivalent to `new` followed by `set_endpoint(ep)`, but usable in
    /// synchronous initialization code (no running async runtime required).
    /// Static `host` is kept as the fallback when `ep` is `None`.
    pub fn new_with_endpoint(
        meta: &'static ProviderMeta,
        host: Option<&str>,
        allow_public: bool,
        bearer: Option<String>,
        policy: RetryConfig,
        ep: Option<RemoteEndpoint>,
    ) -> AppResult<Self> {
        let base = host.unwrap_or(meta.default_base);
        medical_core::endpoint_policy::validate_url(base, allow_public)
            .map_err(|e| AppError::invalid_endpoint_for(e, meta.err_field))?;
        // Same policy set_endpoint enforces: an endpoint supplied at
        // construction must not smuggle a public host past validation.
        if let Some(ref e) = ep {
            medical_core::endpoint_policy::validate_endpoint_pair(
                e.lan.as_deref(),
                e.tailscale.as_deref(),
                allow_public,
            )
            .map_err(|err| AppError::invalid_endpoint_for(err, meta.err_field))?;
        }
        let base_url = format!("{base}/v1");
        let http = build_http_client(meta.display)?;
        Ok(Self {
            meta,
            static_base_url: base_url.clone(),
            client: Mutex::new(OpenAiCompatibleClient::new_with_bearer_and_name(
                http,
                base_url,
                policy,
                bearer,
                meta.display,
            )),
            endpoint: RwLock::new(ep),
            url_cache: Mutex::new(None),
            thinking_disabled: AtomicBool::new(false),
        })
    }

    /// Toggle the reasoning/"thinking" phase off for this provider.
    ///
    /// Called from `init_ai_providers` with the user's
    /// `{provider}_disable_thinking` setting; the frontend must call
    /// `reinit_providers` after changing that setting for it to take effect.
    /// Independent of this toggle, any request carrying
    /// `reasoning_effort: "none"` opts itself out (see
    /// [`apply_thinking_control`](LocalOpenAiProvider::apply_thinking_control)).
    pub fn set_thinking_disabled(&self, disabled: bool) {
        self.thinking_disabled.store(disabled, Ordering::Relaxed);
    }

    /// Apply [`ProviderMeta::thinking`] to an outgoing request when the user
    /// has disabled thinking, or when the REQUEST itself opts out by carrying
    /// `reasoning_effort: "none"` (e.g. the translate tab's one-line
    /// translations, which have nothing to reason about). The prefill
    /// variant appends the control as the LAST message so the model
    /// continues from it directly into the answer instead of opening a new
    /// thinking block.
    ///
    /// The request-level opt-out exists because `reasoning_effort` alone
    /// only reaches servers that honor the parameter (Ollama); LM Studio /
    /// oMLX silently drop it and need the prefill instead.
    fn apply_thinking_control(&self, request: &mut CompletionRequest) {
        let request_opts_out =
            request.reasoning_effort.as_deref() == Some(REASONING_EFFORT_DISABLE);
        if !self.thinking_disabled.load(Ordering::Relaxed) && !request_opts_out {
            return;
        }
        match self.meta.thinking {
            ThinkingControl::ReasoningEffortNone => {
                request.reasoning_effort = Some(REASONING_EFFORT_DISABLE.into());
            }
            ThinkingControl::AssistantPrefill => {
                request.messages.push(Message {
                    role: Role::Assistant,
                    content: MessageContent::Text(THINKING_PREFILL.into()),
                    tool_calls: vec![],
                });
            }
        }
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
            medical_core::endpoint_policy::validate_endpoint_pair(
                e.lan.as_deref(),
                e.tailscale.as_deref(),
                allow_public,
            )
            .map_err(|err| AppError::invalid_endpoint_for(err, self.meta.err_field))?;
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
    pub(crate) async fn current_base_url(&self) -> AppResult<String> {
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
                    provider_name: self.meta.display.into(),
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
    /// Acquires the Mutex on `client`; callers must hold it for the full request.
    async fn sync_client_url(
        &self,
    ) -> AppResult<tokio::sync::MutexGuard<'_, OpenAiCompatibleClient>> {
        let url = self.current_base_url().await?;
        let mut client = self.client.lock().await;
        client.base_url = url;
        Ok(client)
    }
}

/// Shared reqwest builder for every local provider.
fn build_http_client(display: &str) -> AppResult<Client> {
    Client::builder()
        .pool_max_idle_per_host(5)
        .connect_timeout(std::time::Duration::from_secs(10))
        // Generous budget: reasoning ("thinking") models can spend many minutes
        // generating before producing output. Timeouts are NOT retried
        // (http_client::classify_error), so this is the single wall-clock
        // ceiling per attempt.
        .timeout(std::time::Duration::from_secs(900))
        .build()
        .map_err(|e| AppError::ai_provider(format!("Failed to build {display} HTTP client: {e}")))
}

#[async_trait]
impl AiProvider for LocalOpenAiProvider {
    fn name(&self) -> &str {
        self.meta.id
    }

    async fn available_models(&self) -> AppResult<Vec<ModelInfo>> {
        let client = self.sync_client_url().await?;
        // Errors propagate instead of substituting a placeholder list:
        // callers (Settings → Models, the pair flow) must be able to tell
        // "server unreachable" apart from a real list, and the offline
        // error already carries the endpoint URL so the UI can tell the
        // user exactly what isn't running.
        let ids = client.list_models().await?;
        let mut models: Vec<ModelInfo> = ids
            .into_iter()
            .map(|id| ModelInfo {
                name: id.clone(),
                id,
                provider: self.meta.id.into(),
            })
            .collect();
        if models.is_empty() {
            return Err(AppError::ai_provider(format!(
                "{} is reachable but returned no models at {}. Pull or download a model in the {} app, then refresh.",
                self.meta.display, client.base_url, self.meta.display
            )));
        }
        models.sort_by_key(|m| m.id.clone());
        Ok(models)
    }

    async fn complete(&self, mut request: CompletionRequest) -> AppResult<CompletionResponse> {
        let client = self.sync_client_url().await?;
        self.apply_thinking_control(&mut request);
        client.complete(&request).await
    }

    async fn complete_stream(
        &self,
        mut request: CompletionRequest,
    ) -> AppResult<Box<dyn Stream<Item = AppResult<StreamChunk>> + Send + Unpin>> {
        let client = self.sync_client_url().await?;
        self.apply_thinking_control(&mut request);
        let pinned = client.complete_stream(&request).await?;
        Ok(Box::new(pinned))
    }

    async fn complete_with_tools(
        &self,
        mut request: CompletionRequest,
        tools: Vec<ToolDef>,
    ) -> AppResult<ToolCompletionResponse> {
        let client = self.sync_client_url().await?;
        // Same thinking control as complete/complete_stream — the agent
        // orchestrator's tool loop goes through this method, and a thinking
        // model would burn minutes of reasoning on every iteration.
        self.apply_thinking_control(&mut request);
        client.complete_with_tools(&request, tools).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lmstudio;
    use crate::ollama;

    fn all_metas() -> [&'static ProviderMeta; 3] {
        [&ollama::META, &lmstudio::META, &crate::omlx::META]
    }

    #[test]
    fn creates_with_default_host() {
        for m in all_metas() {
            let p = LocalOpenAiProvider::new(m, None, false, None, RetryConfig::default())
                .unwrap_or_else(|e| panic!("build default {}: {e}", m.id));
            assert_eq!(p.static_base_url, format!("{}/v1", m.default_base));
        }
    }

    /// Regression: `new_with_endpoint` used to skip the lan/tailscale
    /// validation `set_endpoint` enforces, letting a public host bypass
    /// the local-only policy at construction time.
    #[test]
    fn new_with_endpoint_rejects_public_ep_host() {
        for m in all_metas() {
            let ep = RemoteEndpoint {
                lan: Some("api.openai.com".into()),
                tailscale: None,
                port: 9999,
                bearer: None,
            };
            let err = LocalOpenAiProvider::new_with_endpoint(
                m,
                None,
                false,
                None,
                RetryConfig::default(),
                Some(ep),
            );
            assert!(err.is_err(), "public ep.lan must be rejected");

            let ep = RemoteEndpoint {
                lan: Some("192.168.1.10".into()),
                tailscale: Some("8.8.8.8".into()),
                port: 9999,
                bearer: None,
            };
            let err = LocalOpenAiProvider::new_with_endpoint(
                m,
                None,
                false,
                None,
                RetryConfig::default(),
                Some(ep),
            );
            assert!(err.is_err(), "public ep.tailscale must be rejected");

            let ep = RemoteEndpoint {
                lan: Some("192.168.1.10".into()),
                tailscale: Some("mac.tail161478.ts.net".into()),
                port: 9999,
                bearer: None,
            };
            assert!(
                LocalOpenAiProvider::new_with_endpoint(
                    m,
                    None,
                    false,
                    None,
                    RetryConfig::default(),
                    Some(ep)
                )
                .is_ok(),
                "local ep hosts must be accepted"
            );
        }
    }

    #[test]
    fn creates_with_custom_host() {
        for m in all_metas() {
            let p = LocalOpenAiProvider::new(
                m,
                Some("http://192.168.1.10:9999"),
                false,
                None,
                RetryConfig::default(),
            )
            .unwrap_or_else(|e| panic!("build custom {}: {e}", m.id));
            assert_eq!(p.static_base_url, "http://192.168.1.10:9999/v1");
        }
    }

    #[test]
    fn stores_bearer_token() {
        for m in all_metas() {
            let _p = LocalOpenAiProvider::new(
                m,
                None,
                false,
                Some("tok_test".into()),
                RetryConfig::default(),
            )
            .unwrap_or_else(|e| panic!("build with bearer {}: {e}", m.id));
            // Bearer is stored on the inner client (exercised via the
            // auth-header paths in openai_compat tests).
        }
    }

    #[tokio::test]
    async fn set_endpoint_clears_cache() {
        let p = LocalOpenAiProvider::new(&ollama::META, None, false, None, RetryConfig::default())
            .expect("build");
        // Seed the cache manually.
        *p.url_cache.lock().await = Some(ResolvedCache {
            url: "http://stale:9999/v1".to_string(),
            resolved_at: std::time::Instant::now(),
        });
        // Setting a new endpoint must clear the cache.
        p.set_endpoint(None, false).await.expect("clear endpoint");
        assert!(p.url_cache.lock().await.is_none());
    }

    #[tokio::test]
    async fn current_base_url_returns_static_when_no_endpoint() {
        for m in all_metas() {
            let p = LocalOpenAiProvider::new(
                m,
                Some("http://192.168.1.42:9999"),
                false,
                None,
                RetryConfig::default(),
            )
            .expect("build");
            let url = p.current_base_url().await.expect("url");
            assert_eq!(url, "http://192.168.1.42:9999/v1");
        }
    }

    #[tokio::test]
    async fn current_base_url_caches_for_30s() {
        use tokio::net::TcpListener;

        // Bind a port so the resolver can connect.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let p =
            LocalOpenAiProvider::new(&lmstudio::META, None, false, None, RetryConfig::default())
                .expect("build");
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

        // First call: listener up — should resolve.
        let url1 = p.current_base_url().await.expect("first resolve");
        assert!(url1.contains(&port.to_string()));

        // Drop the listener so the port is closed.
        drop(listener);

        // Second call immediately after: cache should still return the URL.
        let url2 = p.current_base_url().await.expect("cached resolve");
        assert_eq!(
            url1, url2,
            "cache should return same URL without re-probing"
        );
    }

    #[test]
    fn new_blocks_public_endpoint_by_default() {
        for m in all_metas() {
            let result = LocalOpenAiProvider::new(
                m,
                Some("http://api.openai.com/v1"),
                /* allow_public */ false,
                None,
                RetryConfig::default(),
            );
            assert!(
                matches!(result, Err(medical_core::error::AppError::InvalidEndpoint { field, .. }) if field == m.err_field),
                "{} must reject public hosts with its own field",
                m.id
            );
        }
    }

    #[test]
    fn new_accepts_public_endpoint_when_allow_public() {
        for m in all_metas() {
            let result = LocalOpenAiProvider::new(
                m,
                Some("http://api.openai.com/v1"),
                /* allow_public */ true,
                None,
                RetryConfig::default(),
            );
            assert!(result.is_ok(), "expected Ok for {}", m.id);
        }
    }

    #[test]
    fn new_accepts_local_endpoints_with_default_allow_public() {
        for m in all_metas() {
            for host in [
                None,
                Some("http://localhost:9999"),
                Some("http://192.168.1.42:9999"),
                Some("http://100.64.0.1:9999"),
                Some("http://clinic.local:9999"),
            ] {
                let r = LocalOpenAiProvider::new(
                    m,
                    host,
                    /* allow_public */ false,
                    None,
                    RetryConfig::default(),
                );
                assert!(r.is_ok(), "expected Ok for {} and {host:?}", m.id);
            }
        }
    }

    #[tokio::test]
    async fn set_endpoint_rejects_public_lan_address() {
        for m in all_metas() {
            let p = LocalOpenAiProvider::new(m, None, false, None, RetryConfig::default())
                .expect("build");
            let bad = RemoteEndpoint {
                lan: Some("api.openai.com".into()),
                tailscale: None,
                port: 9999,
                bearer: None,
            };
            let r = p.set_endpoint(Some(bad), false).await;
            assert!(
                matches!(
                    r,
                    Err(medical_core::error::AppError::InvalidEndpoint { .. })
                ),
                "{} must reject public lan",
                m.id
            );
        }
    }

    #[tokio::test]
    async fn set_endpoint_accepts_lan_and_tailscale_addresses() {
        for m in all_metas() {
            let p = LocalOpenAiProvider::new(m, None, false, None, RetryConfig::default())
                .expect("build");
            let good = RemoteEndpoint {
                lan: Some("192.168.1.42".into()),
                tailscale: Some("100.64.0.1".into()),
                port: 9999,
                bearer: None,
            };
            assert!(p.set_endpoint(Some(good), false).await.is_ok());
        }
    }

    #[test]
    fn thinking_control_leaves_request_alone_when_not_disabled() {
        for m in all_metas() {
            let p = LocalOpenAiProvider::new(m, None, false, None, RetryConfig::default())
                .expect("build");
            let mut req = offline_tests::minimal_request("test-model");
            p.apply_thinking_control(&mut req);
            assert_eq!(
                req.messages.len(),
                1,
                "no prefill must be injected when thinking is enabled"
            );
            assert_eq!(
                req.reasoning_effort, None,
                "default provider must not touch reasoning_effort"
            );
        }
    }

    #[test]
    fn effort_strategy_forces_reasoning_effort_off_when_disabled() {
        let p = LocalOpenAiProvider::new(&ollama::META, None, false, None, RetryConfig::default())
            .expect("build");
        p.set_thinking_disabled(true);
        let mut req = offline_tests::minimal_request("qwen3.8:27b");
        p.apply_thinking_control(&mut req);
        assert_eq!(req.reasoning_effort.as_deref(), Some("none"));
        assert_eq!(
            req.messages.len(),
            1,
            "effort strategy must not add messages"
        );
    }

    #[test]
    fn prefill_strategy_appends_prefill_as_last_message_when_disabled() {
        let p =
            LocalOpenAiProvider::new(&lmstudio::META, None, false, None, RetryConfig::default())
                .expect("build");
        p.set_thinking_disabled(true);
        let mut req = offline_tests::minimal_request("qwen3.8-27b");
        p.apply_thinking_control(&mut req);
        assert_eq!(req.messages.len(), 2);
        let prefill = req.messages.last().expect("prefill present");
        assert!(matches!(prefill.role, Role::Assistant));
        match &prefill.content {
            MessageContent::Text(text) => {
                assert_eq!(text, "<think>\n\n</think>\n\n");
                assert!(text.contains("</think>"), "think block must be closed");
            }
            other => panic!("prefill must be plain text, got {other:?}"),
        }
    }

    /// Regression (request-level opt-out): a request carrying
    /// `reasoning_effort: "none"` must fire the prefill strategy even with
    /// the provider toggle OFF — LM Studio/oMLX drop the parameter itself,
    /// so without the prefill the opt-out is a no-op on those servers and
    /// thinking models burn a CoT preamble on trivial calls (translate).
    #[test]
    fn request_level_opt_out_fires_prefill_without_provider_toggle() {
        let p =
            LocalOpenAiProvider::new(&lmstudio::META, None, false, None, RetryConfig::default())
                .expect("build");
        let mut req = offline_tests::minimal_request("qwen3.8-27b");
        req.reasoning_effort = Some(REASONING_EFFORT_DISABLE.into());
        p.apply_thinking_control(&mut req);
        assert_eq!(
            req.messages.len(),
            2,
            "the request's own opt-out must append the prefill"
        );
        let prefill = req.messages.last().expect("prefill present");
        assert!(matches!(prefill.role, Role::Assistant));
        assert_eq!(req.reasoning_effort.as_deref(), Some("none"));
    }

    /// Regression (request-level opt-out): the same opt-out on the
    /// effort-strategy provider keeps the effort value (idempotent) and
    /// adds no messages.
    #[test]
    fn request_level_opt_out_keeps_effort_value_on_effort_strategy() {
        let p = LocalOpenAiProvider::new(&ollama::META, None, false, None, RetryConfig::default())
            .expect("build");
        let mut req = offline_tests::minimal_request("qwen3.8:27b");
        req.reasoning_effort = Some(REASONING_EFFORT_DISABLE.into());
        p.apply_thinking_control(&mut req);
        assert_eq!(req.reasoning_effort.as_deref(), Some("none"));
        assert_eq!(req.messages.len(), 1, "effort strategy adds no messages");
    }

    /// Only the exact disable value opts a request out — a caller asking
    /// for a REAL effort level ("low") must not get the prefill appended.
    #[test]
    fn non_disable_effort_values_do_not_trigger_thinking_control() {
        for m in all_metas() {
            let p = LocalOpenAiProvider::new(m, None, false, None, RetryConfig::default())
                .expect("build");
            let mut req = offline_tests::minimal_request("test-model");
            req.reasoning_effort = Some("low".into());
            p.apply_thinking_control(&mut req);
            assert_eq!(req.messages.len(), 1, "no prefill for non-disable effort");
            assert_eq!(req.reasoning_effort.as_deref(), Some("low"));
        }
    }

    /// End-to-end over HTTP: with thinking disabled, the effort-strategy
    /// provider's POST body must carry the disable effort value.
    #[tokio::test]
    async fn complete_wire_body_carries_reasoning_effort_off() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(serde_json::json!({
                "reasoning_effort": REASONING_EFFORT_DISABLE
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "qwen3.8:27b",
                "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let policy = RetryConfig {
            max_retries: 0,
            ..RetryConfig::default()
        };
        let p = LocalOpenAiProvider::new(&ollama::META, Some(&server.uri()), false, None, policy)
            .expect("build");
        p.set_thinking_disabled(true);

        let resp = p
            .complete(offline_tests::minimal_request("qwen3.8:27b"))
            .await
            .expect("complete should succeed against mock");
        assert_eq!(resp.content, "ok");
        server.verify().await;
    }

    /// End-to-end over HTTP: the tool-calling path must carry the same
    /// thinking control as complete/complete_stream (regression — the agent
    /// orchestrator's tool loop used to bypass the disable).
    #[tokio::test]
    async fn complete_with_tools_wire_body_carries_reasoning_effort_off() {
        use medical_core::types::ToolDef;
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(serde_json::json!({
                "reasoning_effort": REASONING_EFFORT_DISABLE
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "qwen3.8:27b",
                "choices": [{
                    "message": {"content": "ok", "tool_calls": []},
                    "finish_reason": "stop"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let policy = RetryConfig {
            max_retries: 0,
            ..RetryConfig::default()
        };
        let p = LocalOpenAiProvider::new(&ollama::META, Some(&server.uri()), false, None, policy)
            .expect("build");
        p.set_thinking_disabled(true);

        let resp = p
            .complete_with_tools(
                offline_tests::minimal_request("qwen3.8:27b"),
                vec![ToolDef {
                    name: "lookup".into(),
                    description: "look something up".into(),
                    parameters: serde_json::json!({"type": "object", "properties": {}}),
                }],
            )
            .await
            .expect("complete_with_tools should succeed against mock");
        assert_eq!(resp.content.as_deref(), Some("ok"));
        server.verify().await;
    }

    /// End-to-end over HTTP: with thinking disabled, the prefill-strategy
    /// provider's POST body's last message must be the assistant prefill
    /// with the closed think block.
    #[tokio::test]
    async fn complete_wire_body_carries_think_prefill() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(serde_json::json!({
                "messages": [
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "content": "<think>\n\n</think>\n\n"}
                ]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "qwen3.8-27b",
                "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let policy = RetryConfig {
            max_retries: 0,
            ..RetryConfig::default()
        };
        let p = LocalOpenAiProvider::new(&lmstudio::META, Some(&server.uri()), false, None, policy)
            .expect("build");
        p.set_thinking_disabled(true);

        let resp = p
            .complete(offline_tests::minimal_request("qwen3.8-27b"))
            .await
            .expect("complete should succeed against mock");
        assert_eq!(resp.content, "ok");
        server.verify().await;
    }

    /// End-to-end over HTTP: the request-level opt-out (no provider toggle)
    /// must land the prefill on the wire for prefill-strategy servers —
    /// `reasoning_effort: "none"` alone is silently dropped by them.
    #[tokio::test]
    async fn complete_wire_body_carries_prefill_on_request_level_opt_out() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(serde_json::json!({
                "messages": [
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "content": "<think>\n\n</think>\n\n"}
                ]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "qwen3.8-27b",
                "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let policy = RetryConfig {
            max_retries: 0,
            ..RetryConfig::default()
        };
        let p = LocalOpenAiProvider::new(&lmstudio::META, Some(&server.uri()), false, None, policy)
            .expect("build");
        // NOTE: no set_thinking_disabled — the opt-out comes from the request.
        let mut req = offline_tests::minimal_request("qwen3.8-27b");
        req.reasoning_effort = Some(REASONING_EFFORT_DISABLE.into());

        let resp = p
            .complete(req)
            .await
            .expect("complete should succeed against mock");
        assert_eq!(resp.content, "ok");
        server.verify().await;
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

    pub(super) fn minimal_request(model: &str) -> CompletionRequest {
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
            reasoning_effort: None,
        }
    }

    /// Test the RemoteEndpoint resolution path: both LAN and Tailscale are
    /// unreachable, so `resolve_base_url()` returns `None`, and
    /// `current_base_url()` must emit `EndpointOffline` carrying the
    /// provider's display name.
    #[tokio::test]
    async fn resolve_failure_returns_endpoint_offline() {
        let port = dead_port();

        for m in [
            &crate::ollama::META,
            &crate::lmstudio::META,
            &crate::omlx::META,
        ] {
            let p = LocalOpenAiProvider::new(m, None, false, None, RetryConfig::default())
                .expect("build");
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
                    assert_eq!(provider_name, m.display);
                    assert!(
                        endpoint.contains("127.0.0.1"),
                        "endpoint should carry host; got {endpoint:?}"
                    );
                }
                other => panic!("expected EndpointOffline, got {other:?}"),
            }
        }
    }

    /// Test the downstream HTTP-send path (race condition / static URL):
    /// provider is pointed at a dead port via static URL, so `complete()`
    /// hits a connection-refused during the actual HTTP send.
    #[tokio::test]
    async fn complete_returns_endpoint_offline_when_host_refused() {
        let port = dead_port();
        let host = format!("http://127.0.0.1:{port}");

        for m in [
            &crate::ollama::META,
            &crate::lmstudio::META,
            &crate::omlx::META,
        ] {
            // No endpoint set — uses static_base_url pointing at dead port.
            let policy = RetryConfig {
                max_retries: 0,
                ..RetryConfig::default()
            };
            let p = LocalOpenAiProvider::new(m, Some(&host), false, None, policy).expect("build");

            let req = minimal_request("test-model");
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
                    assert_eq!(provider_name, m.display);
                    assert!(
                        endpoint.contains("127.0.0.1"),
                        "endpoint should carry host; got {endpoint:?}"
                    );
                }
                other => panic!("expected EndpointOffline, got {other:?}"),
            }
        }
    }

    /// Model listing must fail loudly when the provider's server is down.
    /// The Settings → Models UI relies on this error (which carries the
    /// endpoint URL) to tell the user exactly what isn't running — the old
    /// behavior of silently returning a placeholder model made a dead
    /// server look like a one-model server.
    #[tokio::test]
    async fn available_models_errors_when_server_down() {
        let port = dead_port();
        let host = format!("http://127.0.0.1:{port}");

        for m in [
            &crate::ollama::META,
            &crate::lmstudio::META,
            &crate::omlx::META,
        ] {
            let policy = RetryConfig {
                max_retries: 0,
                ..RetryConfig::default()
            };
            let p = LocalOpenAiProvider::new(m, Some(&host), false, None, policy).expect("build");

            let err = p.available_models().await.unwrap_err();
            match err {
                AppError::EndpointOffline {
                    provider_name,
                    endpoint,
                    ..
                } => {
                    assert_eq!(provider_name, m.display);
                    assert!(
                        endpoint.contains("127.0.0.1"),
                        "endpoint should carry host; got {endpoint:?}"
                    );
                }
                other => panic!("expected EndpointOffline, got {other:?}"),
            }
        }
    }

    /// A reachable server with zero models is an actionable error too
    /// ("pull a model"), not an empty success.
    #[tokio::test]
    async fn available_models_errors_on_empty_model_list() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let srv = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": [] })),
            )
            .expect(1)
            .mount(&srv)
            .await;

        let p = LocalOpenAiProvider::new(
            &crate::omlx::META,
            Some(srv.uri().as_str()),
            false,
            None,
            RetryConfig {
                max_retries: 0,
                ..RetryConfig::default()
            },
        )
        .expect("build");

        let err = p.available_models().await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no models"), "got: {msg}");
        assert!(msg.contains("oMLX"), "names the provider: {msg}");
        srv.verify().await;
    }
}
