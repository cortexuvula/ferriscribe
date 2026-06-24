//! Server-Sent Events stream parser for AI provider streaming responses.

use eventsource_stream::Eventsource;
use futures_core::Stream;
use reqwest::Response;
use std::pin::Pin;
use tokio_stream::StreamExt;

/// Parse a streaming HTTP response as Server-Sent Events (SSE), yielding
/// non-empty, non-`[DONE]` data lines.
///
/// Each SSE `data:` field from the response body is emitted as a `String`.
/// The SSE terminator `data: [DONE]` and empty data lines are silently
/// filtered out. Transport-level SSE parse errors are emitted as
/// `Err(String)` items.
///
/// # Provider quirks handled
///
/// - Both Ollama and LM Studio send `data: [DONE]` as the final event;
///   this is filtered here rather than in callers.
/// - Some providers emit blank `data:` lines between events as keep-alives;
///   these are dropped by the empty-check filter.
///
/// # Errors
///
/// Transport-level errors from the underlying `eventsource-stream` parser
/// are forwarded as `Err(String)` items in the stream. JSON parse errors
/// are **not** produced here — this function operates on raw SSE data lines
/// only; callers are responsible for deserializing each line.
pub fn parse_sse_response(
    response: Response,
) -> Pin<Box<dyn Stream<Item = Result<String, String>> + Send>> {
    let stream =
        response
            .bytes_stream()
            .eventsource()
            .filter_map(|event_result| match event_result {
                Err(e) => Some(Err(e.to_string())),
                Ok(event) => {
                    let data = event.data;
                    if data.is_empty() || data == "[DONE]" {
                        None
                    } else {
                        Some(Ok(data))
                    }
                }
            });

    Box::pin(stream)
}
