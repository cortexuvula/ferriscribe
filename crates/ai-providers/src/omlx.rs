//! oMLX provider — wraps [`OpenAiCompatibleClient`] against a local oMLX server.
//!
//! [oMLX](https://github.com/jundot/omlx) is an MLX-based LLM inference
//! server for Apple Silicon (macOS menu-bar app, continuous batching,
//! SSD-backed KV cache). Its OpenAI-compatible API listens on
//! `http://localhost:8000/v1` by default.
//!
//! # Default configuration
//!
//! - Base URL: `http://localhost:8000/v1`
//! - Fallback model: `default`
//! - Provider name: `"omlx"`
//!
//! All endpoint-resolution, caching, retry, and thinking-control machinery
//! lives in [`crate::local_openai`]; this module only supplies the
//! [`ProviderMeta`] identity. See that module's docs for the LAN/Tailscale
//! `RemoteEndpoint` probing behavior.
//!
//! [`OpenAiCompatibleClient`]: crate::openai_compat::OpenAiCompatibleClient

use async_trait::async_trait;
use futures_core::Stream;

use medical_core::{
    error::AppResult,
    traits::AiProvider,
    types::{
        CompletionRequest, CompletionResponse, ModelInfo, RemoteEndpoint, StreamChunk,
        ToolCompletionResponse, ToolDef,
    },
};

use crate::http_client::RetryConfig;
use crate::local_openai::{LocalOpenAiProvider, ProviderMeta, ThinkingControl};

/// Static identity for the oMLX provider. `thinking` uses the assistant
/// `<think>`-prefill strategy: oMLX serves mlx-lm's Jinja chat templates
/// but, like LM Studio, ignores API-level thinking parameters, so a
/// pre-closed `<think>` block in an assistant prefill is the one switch
/// that reaches the template.
pub(crate) static META: ProviderMeta = ProviderMeta {
    id: "omlx",
    display: "oMLX",
    default_base: "http://localhost:8000",
    err_field: "omlx_host",
    fallback_model: "default",
    thinking: ThinkingControl::AssistantPrefill,
};

/// oMLX provider implementing the [`AiProvider`] trait.
///
/// Wraps an [`OpenAiCompatibleClient`] pointed at an oMLX server. Supports
/// optional [`RemoteEndpoint`] for LAN/Tailscale resolution, bearer-token
/// authentication, and configurable retry policy.
///
/// [`OpenAiCompatibleClient`]: crate::openai_compat::OpenAiCompatibleClient
/// [`AiProvider`]: medical_core::traits::AiProvider
pub struct OmlxProvider {
    inner: LocalOpenAiProvider,
}

impl OmlxProvider {
    /// Create a new oMLX provider.
    ///
    /// `host` defaults to `http://localhost:8000` when `None`.
    /// `bearer` is an optional bearer token for auth-proxied remote connections.
    /// `policy` controls retry behavior for inner HTTP calls.
    pub fn new(
        host: Option<&str>,
        allow_public: bool,
        bearer: Option<String>,
        policy: RetryConfig,
    ) -> AppResult<Self> {
        Ok(Self {
            inner: LocalOpenAiProvider::new(&META, host, allow_public, bearer, policy)?,
        })
    }

    /// Create a new oMLX provider with a `RemoteEndpoint` pre-configured.
    ///
    /// Usable in synchronous initialization code (no running async runtime required).
    pub fn new_with_endpoint(
        host: Option<&str>,
        allow_public: bool,
        bearer: Option<String>,
        policy: RetryConfig,
        ep: Option<RemoteEndpoint>,
    ) -> AppResult<Self> {
        Ok(Self {
            inner: LocalOpenAiProvider::new_with_endpoint(
                &META,
                host,
                allow_public,
                bearer,
                policy,
                ep,
            )?,
        })
    }

    /// Toggle the reasoning/"thinking" phase off for this provider.
    ///
    /// When disabled, every request appends an assistant prefill message
    /// with a pre-closed `<think>` block (see [`META`] for the rationale).
    /// Called from `init_ai_providers` with the user's
    /// `omlx_disable_thinking` setting; the frontend must call
    /// `reinit_providers` after changing that setting for it to take effect.
    pub fn set_thinking_disabled(&self, disabled: bool) {
        self.inner.set_thinking_disabled(disabled);
    }

    /// Override the remote endpoint used for LAN/Tailscale resolution. See
    /// [`LocalOpenAiProvider::set_endpoint`] for the bearer-propagation and
    /// cache-invalidation contract.
    pub async fn set_endpoint(
        &self,
        ep: Option<RemoteEndpoint>,
        allow_public: bool,
    ) -> AppResult<()> {
        self.inner.set_endpoint(ep, allow_public).await
    }
}

#[async_trait]
impl AiProvider for OmlxProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn available_models(&self) -> AppResult<Vec<ModelInfo>> {
        self.inner.available_models().await
    }

    async fn complete(&self, request: CompletionRequest) -> AppResult<CompletionResponse> {
        self.inner.complete(request).await
    }

    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> AppResult<Box<dyn Stream<Item = AppResult<StreamChunk>> + Send + Unpin>> {
        self.inner.complete_stream(request).await
    }

    async fn complete_with_tools(
        &self,
        request: CompletionRequest,
        tools: Vec<ToolDef>,
    ) -> AppResult<ToolCompletionResponse> {
        self.inner.complete_with_tools(request, tools).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_omlx_identity() {
        let p = OmlxProvider::new(None, false, None, RetryConfig::default())
            .expect("build default provider");
        assert_eq!(p.name(), "omlx");
        assert_eq!(META.id, "omlx");
        assert_eq!(META.default_base, "http://localhost:8000");
        assert_eq!(META.err_field, "omlx_host");
        assert_eq!(META.fallback_model, "default");
        assert_eq!(META.thinking, ThinkingControl::AssistantPrefill);
    }
}
