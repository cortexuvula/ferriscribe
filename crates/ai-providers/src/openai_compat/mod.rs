//! Base client for any OpenAI-compatible chat-completions endpoint.
//!
//! Submodules:
//! - [`wire`] — the private serde types modeling the OpenAI request/response shape.
//! - [`client`] — `OpenAiCompatibleClient`, its constructors, and request/response helpers.
//! - [`methods`] — the four public methods (`list_models`, `complete`, `complete_stream`, `complete_with_tools`).

mod client;
mod methods;
mod wire;

pub use client::OpenAiCompatibleClient;
