//! OpenAI-compatible chat-completions client.
//!
//! This module provides [`OpenAiCompatibleClient`], a provider-agnostic HTTP
//! client for any server implementing the OpenAI chat-completions wire protocol.
//! Both [`OllamaProvider`](crate::ollama::OllamaProvider) and
//! [`LmStudioProvider`](crate::lmstudio::LmStudioProvider) delegate to this
//! client.
//!
//! # Submodules
//!
//! - `wire` — private serde types modeling the OpenAI request/response shape
//!   (`ChatRequest`, `ChatResponse`, `ChatMessage`, `ApiToolCall`, etc.).
//!   These are `pub(super)` and not part of the public API.
//! - `client` — `OpenAiCompatibleClient` struct, constructors, message
//!   conversion, request building, and response parsing.
//! - `methods` — the four public methods: `list_models`, `complete`,
//!   `complete_stream`, `complete_with_tools`.
//! - `think` — stripping of inlined `<think>…</think>` reasoning blocks from
//!   responses and streams (non-streaming helper + streaming filter).
//!
//! # Wire protocol coverage
//!
//! The client covers the subset of the OpenAI API that local providers support:
//!
//! | Endpoint | Method | Purpose |
//! |----------|--------|---------|
//! | `GET /models` | `list_models` | Enumerate available models |
//! | `POST /chat/completions` | `complete` | Non-streaming completion |
//! | `POST /chat/completions` (stream) | `complete_stream` | SSE streaming completion |
//! | `POST /chat/completions` (tools) | `complete_with_tools` | Completion with tool definitions |

mod client;
mod methods;
mod think;
mod wire;

pub use client::OpenAiCompatibleClient;
pub use think::strip_leading_think_block;
