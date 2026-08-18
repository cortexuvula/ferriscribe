//! Shared streaming driver for the generation commands: consumes the
//! provider's SSE stream, assembles the completion, enforces an idle
//! timeout, and reports throttled count-only progress via a callback.

use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::traits::AiProvider;
use medical_core::types::{CompletionRequest, CompletionResponse, StreamChunk, UsageInfo};

use super::GenerationProgressStats;

/// No data from the model for this long → stalled. Generous enough for
/// reasoning pauses; the request itself is capped at 1h by the provider
/// layer (`STREAM_TOTAL_TIMEOUT` in ai-providers methods.rs).
pub(super) const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Minimum spacing between on_progress callbacks.
const PROGRESS_THROTTLE: std::time::Duration = std::time::Duration::from_millis(500);

/// Drive a streamed completion to completion. `on_progress` is invoked at
/// most every 500ms with count-only stats, plus one terminal flush when the
/// stream ends; call sites wire it to the `generation-progress` event.
pub(super) async fn stream_to_completion(
    provider: &Arc<dyn AiProvider>,
    mut on_progress: impl FnMut(&GenerationProgressStats),
    request: CompletionRequest,
) -> AppResult<CompletionResponse> {
    stream_with_idle_timeout(provider, &mut on_progress, request, STREAM_IDLE_TIMEOUT).await
}

/// Testable core with an injectable idle budget. The callback stays generic
/// (not erased to `dyn FnMut`) so a `Send` closure at the call site keeps the
/// whole future `Send` — Tauri command futures must be.
async fn stream_with_idle_timeout(
    provider: &Arc<dyn AiProvider>,
    on_progress: &mut impl FnMut(&GenerationProgressStats),
    request: CompletionRequest,
    idle: std::time::Duration,
) -> AppResult<CompletionResponse> {
    use futures_util::StreamExt;

    let model = request.model.clone();
    let mut stream = provider.complete_stream(request).await?;

    let mut content = String::new();
    let mut usage = UsageInfo::default();
    let mut tokens: u64 = 0;
    let mut first_chunk_at: Option<std::time::Instant> = None;
    let mut last_emit: Option<std::time::Instant> = None;

    loop {
        let chunk = match tokio::time::timeout(idle, stream.next()).await {
            Err(_) => {
                return Err(AppError::ai_provider(format!(
                    "generation stalled — no data from the model for {}s",
                    idle.as_secs()
                )));
            }
            Ok(None) => break, // stream ended (with or without a Done chunk)
            Ok(Some(result)) => result?,
        };
        let now = std::time::Instant::now();
        if first_chunk_at.is_none() {
            first_chunk_at = Some(now);
        }
        match chunk {
            StreamChunk::Delta { text } => {
                content.push_str(&text);
                tokens += 1;
            }
            StreamChunk::ReasoningDelta { .. } => {
                // Length only by construction — the reasoning text stays
                // inside the provider layer (AGENTS.md PHI rule).
                tokens += 1;
            }
            StreamChunk::Usage(u) => usage = u,
            StreamChunk::Done => break,
            StreamChunk::ToolCallDelta { .. } => {}
        }
        let start = first_chunk_at.unwrap_or(now);
        let elapsed = now.duration_since(start);
        // Skip the very first chunk: elapsed is zero, so a rate would be
        // meaningless (and enormous). The next chunk provides a real rate.
        let due = !elapsed.is_zero()
            && last_emit.is_none_or(|t| now.duration_since(t) >= PROGRESS_THROTTLE);
        if due {
            on_progress(&GenerationProgressStats {
                tokens,
                elapsed_ms: elapsed.as_millis() as u64,
                tokens_per_second: tokens as f64 / elapsed.as_secs_f64().max(f64::EPSILON),
            });
            last_emit = Some(now);
        }
    }

    // Terminal flush: the throttle may have suppressed the most recent
    // deltas, so emit one final update with the complete counts — unless
    // the whole stream was instantaneous (zero elapsed → meaningless rate).
    if let Some(start) = first_chunk_at {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(start);
        if !elapsed.is_zero() {
            on_progress(&GenerationProgressStats {
                tokens,
                elapsed_ms: elapsed.as_millis() as u64,
                tokens_per_second: tokens as f64 / elapsed.as_secs_f64(),
            });
        }
    }

    Ok(CompletionResponse {
        content: strip_leading_think_block(&content).to_string(),
        model,
        usage,
        tool_calls: vec![],
    })
}

/// Strip a leading `<think>…</think>` block (some providers, notably Ollama
/// with reasoning models, inline reasoning into content). If the block is
/// never closed, everything is reasoning — return the empty remainder.
pub(super) fn strip_leading_think_block(content: &str) -> &str {
    let trimmed = content.trim_start();
    let Some(rest) = trimmed.strip_prefix("<think>") else {
        return content;
    };
    match rest.find("</think>") {
        Some(end) => rest[end + "</think>".len()..].trim_start(),
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::generation::test_helpers::ScriptedStreamProvider;
    use medical_core::types::{Message, MessageContent, Role};

    fn req() -> CompletionRequest {
        CompletionRequest {
            model: "llama3".into(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("hi".into()),
                tool_calls: vec![],
            }],
            temperature: None,
            max_tokens: None,
            system_prompt: None,
            reasoning_effort: None,
        }
    }

    fn provider(chunks: Vec<Result<StreamChunk, String>>, stall: bool) -> Arc<dyn AiProvider> {
        Arc::new(ScriptedStreamProvider {
            name: "scripted",
            chunks,
            stall_after_last: stall,
        })
    }

    #[tokio::test]
    async fn assembles_deltas_and_usage() {
        let p = provider(
            vec![
                Ok(StreamChunk::Delta {
                    text: "**S:** ".into(),
                }),
                Ok(StreamChunk::ReasoningDelta { len: 900 }),
                Ok(StreamChunk::Delta {
                    text: "headache".into(),
                }),
                Ok(StreamChunk::Usage(UsageInfo {
                    prompt_tokens: 10,
                    completion_tokens: 3,
                    total_tokens: 13,
                })),
                Ok(StreamChunk::Done),
            ],
            false,
        );
        let mut seen = Vec::new();
        let resp = stream_to_completion(&p, |s| seen.push(*s), req())
            .await
            .expect("stream completes");
        assert_eq!(resp.content, "**S:** headache");
        assert_eq!(resp.model, "llama3");
        assert_eq!(resp.usage.completion_tokens, 3);
        assert!(!seen.is_empty());
        assert!(seen.last().unwrap().tokens >= 3);
    }

    #[tokio::test]
    async fn idle_timeout_errors_when_stalled() {
        let p = provider(vec![Ok(StreamChunk::Delta { text: "x".into() })], true);
        let err = stream_with_idle_timeout(
            &p,
            &mut |_| {},
            req(),
            std::time::Duration::from_millis(100),
        )
        .await
        .expect_err("must stall");
        assert!(err.to_string().contains("stalled"));
    }

    #[tokio::test]
    async fn stream_error_propagates() {
        let p = provider(vec![Err("provider exploded".to_string())], false);
        let err = stream_to_completion(&p, |_| {}, req())
            .await
            .expect_err("must fail");
        assert!(err.to_string().contains("provider exploded"));
    }

    #[tokio::test]
    async fn strips_leading_think_block() {
        let p = provider(
            vec![
                Ok(StreamChunk::Delta {
                    text: "<think>secret reasoning</think>\n\nSOAP".into(),
                }),
                Ok(StreamChunk::Done),
            ],
            false,
        );
        let resp = stream_to_completion(&p, |_| {}, req())
            .await
            .expect("completes");
        assert_eq!(resp.content, "SOAP");
    }

    #[test]
    fn think_block_edge_cases() {
        assert_eq!(strip_leading_think_block("  <think> forever"), "");
        assert_eq!(strip_leading_think_block("no tags"), "no tags");
        assert_eq!(strip_leading_think_block("<think>x</think>  body"), "body");
        // Mid-text think blocks are left alone (only a LEADING block is stripped).
        assert_eq!(
            strip_leading_think_block("SOAP <think>late</think>"),
            "SOAP <think>late</think>"
        );
    }

    #[tokio::test]
    async fn no_bogus_rate_on_instant_streams() {
        // Regression: the very first chunk has zero elapsed time, which
        // once produced a ~4.5e15 "tok/s" flash. Emitted stats must always
        // carry a finite, sane rate.
        let p = provider(
            vec![
                Ok(StreamChunk::Delta { text: "x".into() }),
                Ok(StreamChunk::Done),
            ],
            false,
        );
        let mut seen = Vec::new();
        let _ = stream_to_completion(&p, |s| seen.push(*s), req()).await;
        assert!(
            seen.iter()
                .all(|s| s.tokens_per_second.is_finite() && s.tokens_per_second < 1e12)
        );
    }

    #[tokio::test]
    async fn missing_usage_records_default() {
        let p = provider(
            vec![
                Ok(StreamChunk::Delta { text: "ok".into() }),
                Ok(StreamChunk::Done),
            ],
            false,
        );
        let resp = stream_to_completion(&p, |_| {}, req())
            .await
            .expect("completes");
        assert_eq!(resp.usage.completion_tokens, 0);
    }
}
