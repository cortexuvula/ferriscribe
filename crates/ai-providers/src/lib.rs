//! Local AI provider integration for FerriScribe.
//!
//! This crate provides Ollama and LM Studio connectivity via the
//! OpenAI-compatible chat-completions wire protocol, with streaming (SSE),
//! tool calling, automatic retry with exponential backoff, and LAN/Tailscale
//! endpoint resolution.
//!
//! # Hard Constraint: Local-Only Providers
//!
//! Only local and LAN-accessible AI providers are supported. Hosted APIs
//! (OpenAI, Anthropic, Google, etc.) are intentionally **not** supported.
//! This is a PHI/HIPAA requirement — patient data must never leave the
//! local network. The [`endpoint_policy`] validation layer in `medical-core`
//! rejects public URLs by default.
//!
//! # Architecture
//!
//! - [`ProviderRegistry`] — holds registered [`AiProvider`] instances keyed
//!   by name, tracks the active provider. Used by `src-tauri` to switch
//!   between Ollama and LM Studio at runtime.
//! - [`ollama::OllamaProvider`] — wraps [`openai_compat::OpenAiCompatibleClient`] pointed at
//!   an Ollama server (default `http://localhost:11434/v1`).
//! - [`lmstudio::LmStudioProvider`] — wraps [`openai_compat::OpenAiCompatibleClient`] pointed
//!   at an LM Studio server (default `http://localhost:1234/v1`).
//! - [`openai_compat::OpenAiCompatibleClient`] — generic HTTP client for any
//!   endpoint implementing the OpenAI chat-completions protocol.
//! - [`http_client`] — retry infrastructure: [`RetryConfig`],
//!   `send_with_retry`, and retry classification helpers.
//! - [`sse`] — SSE stream parser for streaming AI responses.
//!
//! [`AiProvider`]: medical_core::traits::AiProvider
//! [`endpoint_policy`]: medical_core::endpoint_policy
//! [`RetryConfig`]: http_client::RetryConfig

pub mod http_client;
pub mod lmstudio;
pub mod ollama;
pub mod openai_compat;
pub mod sse;

use medical_core::traits::AiProvider;
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of named [`AiProvider`] instances with a tracked active provider.
///
/// Used by `src-tauri` to hold all configured AI providers and switch between
/// them at runtime (e.g., user toggles between Ollama and LM Studio in
/// settings). The first registered provider becomes the active one by default.
///
/// # Examples
///
/// ```rust,ignore
/// let mut registry = ProviderRegistry::new();
/// registry.register(Arc::new(ollama_provider));
/// registry.register(Arc::new(lmstudio_provider));
/// registry.set_active("lmstudio");
/// let provider = registry.active().expect("active provider");
/// ```
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn AiProvider>>,
    active: String,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    /// Create an empty registry with no providers and no active provider.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            active: String::new(),
        }
    }

    /// Register a provider under its [`AiProvider::name`].
    ///
    /// If this is the first provider registered, it automatically becomes
    /// the active provider.
    pub fn register(&mut self, provider: Arc<dyn AiProvider>) {
        let name = provider.name().to_string();
        if self.active.is_empty() {
            self.active = name.clone();
        }
        self.providers.insert(name, provider);
    }

    /// Look up a provider by name.
    pub fn get(&self, name: &str) -> Option<&dyn AiProvider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    /// Return the currently active provider, if any.
    pub fn active(&self) -> Option<&dyn AiProvider> {
        self.get(&self.active)
    }

    /// Returns the name of the currently active provider.
    pub fn active_name(&self) -> &str {
        &self.active
    }

    /// Returns a cloned `Arc` of a named provider, suitable for use across await points.
    pub fn get_arc(&self, name: &str) -> Option<Arc<dyn AiProvider>> {
        self.providers.get(name).cloned()
    }

    /// Returns a cloned `Arc` of the active provider, suitable for use across await points.
    pub fn get_active_arc(&self) -> Option<Arc<dyn AiProvider>> {
        self.providers.get(&self.active).cloned()
    }

    /// Set the active provider by name. Returns `true` if the provider exists.
    pub fn set_active(&mut self, name: &str) -> bool {
        if self.providers.contains_key(name) {
            self.active = name.to_string();
            true
        } else {
            false
        }
    }

    /// List all registered provider names.
    pub fn list_available(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }
}
