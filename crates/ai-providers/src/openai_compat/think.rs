//! Stripping of inlined reasoning ("thinking") from model output.
//!
//! Some providers — notably Ollama's OpenAI-compatible endpoint with
//! reasoning models — inline the model's `<think>…</think>` block into the
//! content/delta `content` field instead of returning it in a separate
//! reasoning field (which the wire layer already reduces to a length).
//! Both providers delegate to the shared OpenAI-compatible client, so
//! stripping here covers every consumer of `complete`, `complete_stream`,
//! and `complete_with_tools` — chat, SOAP, letters, translation, OCR, and
//! the agent loop alike.
//!
//! PHI: reasoning can echo clinical content. Stripping it inside the
//! provider layer means the reasoning text never crosses the provider
//! boundary — only the answer does.

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::stream::Stream;

use medical_core::{error::AppResult, types::StreamChunk};

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

/// Strip a leading `<think>…</think>` block from a complete response. If the
/// block is never closed, everything is reasoning — return the empty
/// remainder. Only a LEADING block (after optional whitespace) is stripped;
/// mid-text blocks are left alone.
pub fn strip_leading_think_block(content: &str) -> &str {
    let trimmed = content.trim_start();
    let Some(rest) = trimmed.strip_prefix(THINK_OPEN) else {
        return content;
    };
    match rest.find(THINK_CLOSE) {
        Some(end) => rest[end + THINK_CLOSE.len()..].trim_start(),
        None => "",
    }
}

/// State of a [`ThinkFilter`].
#[derive(Default)]
enum ThinkState {
    /// Still deciding whether the stream opens with a `<think>` block;
    /// `buf` holds everything seen so far (bounded ambiguity — see
    /// [`ThinkFilter::push`]).
    #[default]
    Undecided,
    /// Inside a leading think block; `buf` holds the not-yet-searched tail
    /// (kept to at most `THINK_CLOSE.len() - 1` bytes so a closing tag split
    /// across deltas is still matched). Discarded content — never emitted.
    Thinking,
    /// Think block closed; `buf` holds the whitespace run directly after
    /// `</think>` until the first non-whitespace answer character arrives
    /// (mirrors the `trim_start` in [`strip_leading_think_block`]).
    AnswerWhitespace,
    /// Not a thinking stream, or stripping complete — everything passes.
    Passthrough,
}

/// Streaming counterpart of [`strip_leading_think_block`]: feed it every
/// `Delta` text in order; it returns exactly the text that is safe to emit
/// (possibly empty while a leading think block or an ambiguous tag prefix is
/// still being received). Call [`ThinkFilter::finish`] at end of stream.
///
/// The ambiguity window is tiny: the filter only holds text while everything
/// received so far is whitespace plus a prefix of `<think>` (7 bytes). Any
/// other content settles the decision and is flushed immediately, so normal
/// streams are never delayed.
#[derive(Default)]
pub(super) struct ThinkFilter {
    state: ThinkState,
    buf: String,
}

impl ThinkFilter {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Feed one text delta; returns the text safe to emit now.
    pub(super) fn push(&mut self, text: &str) -> String {
        self.buf.push_str(text);
        loop {
            match self.state {
                ThinkState::Undecided => {
                    let trimmed = self.buf.trim_start();
                    if trimmed.is_empty() || THINK_OPEN.starts_with(trimmed) {
                        // Whitespace or a partial `<think>` prefix — could
                        // still become a think block, keep holding.
                        return String::new();
                    }
                    if let Some(rest) = trimmed.strip_prefix(THINK_OPEN) {
                        // Leading think block confirmed — everything held so
                        // far is reasoning and is discarded, never emitted.
                        self.buf = rest.to_string();
                        self.state = ThinkState::Thinking;
                        continue;
                    }
                    // Definitively not a think block — flush verbatim
                    // (leading whitespace included, like the strip helper).
                    self.state = ThinkState::Passthrough;
                    return std::mem::take(&mut self.buf);
                }
                ThinkState::Thinking => {
                    if let Some(pos) = self.buf.find(THINK_CLOSE) {
                        self.buf.drain(..pos + THINK_CLOSE.len());
                        self.state = ThinkState::AnswerWhitespace;
                        continue;
                    }
                    // No close tag yet — keep only a tag-length-minus-one
                    // tail so a split `</think>` still matches later.
                    let mut keep = self.buf.len().saturating_sub(THINK_CLOSE.len() - 1);
                    while keep < self.buf.len() && !self.buf.is_char_boundary(keep) {
                        keep += 1;
                    }
                    self.buf.drain(..keep);
                    return String::new();
                }
                ThinkState::AnswerWhitespace => match self.buf.find(|c: char| !c.is_whitespace()) {
                    Some(i) => {
                        self.buf.drain(..i);
                        self.state = ThinkState::Passthrough;
                        return std::mem::take(&mut self.buf);
                    }
                    // Whitespace-only so far — hold until the answer starts
                    // (a pure-whitespace remainder is dropped, like the
                    // strip helper's trim_start).
                    None => return String::new(),
                },
                ThinkState::Passthrough => return std::mem::take(&mut self.buf),
            }
        }
    }

    /// End of stream (or a terminal marker): flush held text that was never
    /// proven to be part of a think block. An unterminated think block and a
    /// whitespace-only answer remainder are discarded — matching
    /// [`strip_leading_think_block`]. Afterwards the filter passes through.
    pub(super) fn finish(&mut self) -> String {
        let was_undecided = matches!(self.state, ThinkState::Undecided);
        self.state = ThinkState::Passthrough;
        if was_undecided {
            std::mem::take(&mut self.buf)
        } else {
            self.buf.clear();
            String::new()
        }
    }
}

/// Stream adapter that runs every `Delta` through a [`ThinkFilter`] so an
/// inlined leading think block never reaches the consumer as text.
///
/// `Usage`/`Done` markers are forwarded only after any still-held text is
/// flushed — the generation driver stops at `Done`, so a flush emitted
/// after it would be lost. The filter's `finish` runs when the inner stream
/// ends (or a marker arrives, whichever comes first).
pub(super) struct ThinkStripStream<S> {
    inner: S,
    filter: ThinkFilter,
    queue: VecDeque<AppResult<StreamChunk>>,
    done: bool,
}

impl<S> ThinkStripStream<S>
where
    S: Stream<Item = AppResult<StreamChunk>> + Unpin,
{
    pub(super) fn new(inner: S) -> Self {
        Self {
            inner,
            filter: ThinkFilter::new(),
            queue: VecDeque::new(),
            done: false,
        }
    }
}

impl<S> Stream for ThinkStripStream<S>
where
    S: Stream<Item = AppResult<StreamChunk>> + Unpin,
{
    type Item = AppResult<StreamChunk>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        while this.queue.is_empty() && !this.done {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    this.done = true;
                    let rest = this.filter.finish();
                    if !rest.is_empty() {
                        this.queue.push_front(Ok(StreamChunk::Delta { text: rest }));
                    }
                }
                Poll::Ready(Some(item)) => match item {
                    Ok(StreamChunk::Delta { text }) => {
                        let out = this.filter.push(&text);
                        if !out.is_empty() {
                            this.queue.push_front(Ok(StreamChunk::Delta { text: out }));
                        }
                    }
                    Ok(marker @ (StreamChunk::Usage(_) | StreamChunk::Done)) => {
                        // Terminal marker: any still-ambiguous held text is
                        // real content and must precede the marker.
                        let held = this.filter.finish();
                        this.queue.push_front(Ok(marker));
                        if !held.is_empty() {
                            this.queue.push_front(Ok(StreamChunk::Delta { text: held }));
                        }
                    }
                    other => this.queue.push_front(other),
                },
            }
        }
        Poll::Ready(this.queue.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_edge_cases() {
        assert_eq!(strip_leading_think_block("  <think> forever"), "");
        assert_eq!(strip_leading_think_block("no tags"), "no tags");
        assert_eq!(strip_leading_think_block("<think>x</think>  body"), "body");
        // Mid-text think blocks are left alone (only a LEADING block is stripped).
        assert_eq!(
            strip_leading_think_block("SOAP <think>late</think>"),
            "SOAP <think>late</think>"
        );
        // Leading whitespace before the tag is part of the block.
        assert_eq!(strip_leading_think_block("\n<think>r</think>ok"), "ok");
    }

    #[test]
    fn filter_strips_split_tags_across_deltas() {
        let mut f = ThinkFilter::new();
        assert_eq!(f.push("<th"), "");
        assert_eq!(f.push("ink>the patient has"), "");
        assert_eq!(f.push(" chest pain</th"), "");
        assert_eq!(f.push("ink>\n\nS: cough"), "S: cough");
        assert_eq!(f.finish(), "");
    }

    #[test]
    fn filter_passes_normal_text_through_immediately() {
        let mut f = ThinkFilter::new();
        assert_eq!(f.push("Dear Dr. Smith,"), "Dear Dr. Smith,");
        assert_eq!(f.push(" hi"), " hi");
        assert_eq!(f.finish(), "");
    }

    #[test]
    fn filter_flushes_ambiguous_prefix_when_it_never_becomes_a_tag() {
        // "<think" alone is ambiguous; once it grows into something else,
        // everything flows through untouched.
        let mut f = ThinkFilter::new();
        assert_eq!(f.push("  <think"), "");
        assert_eq!(f.push("ing of you>"), "  <thinking of you>");
        assert_eq!(f.finish(), "");
    }

    #[test]
    fn filter_drops_unterminated_block() {
        let mut f = ThinkFilter::new();
        assert_eq!(f.push("<think>reasoning that never ends"), "");
        assert_eq!(f.finish(), "");
    }

    #[test]
    fn filter_drops_whitespace_only_answer() {
        let mut f = ThinkFilter::new();
        assert_eq!(f.push("<think>r</think>\n\n"), "");
        assert_eq!(f.finish(), "");
    }

    #[test]
    fn filter_flushes_undecided_buffer_at_finish() {
        let mut f = ThinkFilter::new();
        assert_eq!(f.push("hi <th"), "hi <th"); // not a leading tag — immediate
        let mut f2 = ThinkFilter::new();
        assert_eq!(f2.push("  <thi"), ""); // still ambiguous at end of stream
        assert_eq!(f2.finish(), "  <thi");
    }

    #[test]
    fn filter_mid_text_tag_passes_through() {
        let mut f = ThinkFilter::new();
        assert_eq!(f.push("SOAP note "), "SOAP note ");
        assert_eq!(f.push("<think>late</think>"), "<think>late</think>");
        assert_eq!(f.finish(), "");
    }

    #[tokio::test]
    async fn stream_adapter_strips_think_and_keeps_marker_order() {
        use futures_util::StreamExt;
        use medical_core::types::UsageInfo;

        let chunks: Vec<AppResult<StreamChunk>> = vec![
            Ok(StreamChunk::Delta { text: "<th".into() }),
            Ok(StreamChunk::Delta {
                text: "ink>reasoning</th".into(),
            }),
            Ok(StreamChunk::Delta {
                text: "ink>\n\nAnswer".into(),
            }),
            Ok(StreamChunk::Usage(UsageInfo {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
                decode_tokens_per_second: None,
            })),
            Ok(StreamChunk::Done),
        ];
        let stripped = ThinkStripStream::new(tokio_stream::iter(chunks));
        let out: Vec<AppResult<StreamChunk>> = stripped.collect().await;

        assert_eq!(
            out.len(),
            3,
            "expect Delta, Usage, Done — reasoning dropped"
        );
        assert!(matches!(&out[0], Ok(StreamChunk::Delta { text }) if text == "Answer"));
        assert!(matches!(&out[1], Ok(StreamChunk::Usage(u)) if u.total_tokens == 3));
        assert!(matches!(out[2], Ok(StreamChunk::Done)));
    }

    #[tokio::test]
    async fn stream_adapter_flushes_held_text_before_done_marker() {
        use futures_util::StreamExt;

        // "  <thi" stays ambiguous (whitespace + partial tag); when Done
        // arrives it must be flushed as a Delta BEFORE the Done marker.
        let chunks: Vec<AppResult<StreamChunk>> = vec![
            Ok(StreamChunk::Delta {
                text: "  <thi".into(),
            }),
            Ok(StreamChunk::Done),
        ];
        let stripped = ThinkStripStream::new(tokio_stream::iter(chunks));
        let out: Vec<AppResult<StreamChunk>> = stripped.collect().await;

        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Ok(StreamChunk::Delta { text }) if text == "  <thi"));
        assert!(matches!(out[1], Ok(StreamChunk::Done)));
    }

    #[tokio::test]
    async fn stream_adapter_passes_errors_and_unterminated_block_drops_everything() {
        use futures_util::StreamExt;

        let chunks: Vec<AppResult<StreamChunk>> = vec![
            Ok(StreamChunk::Delta {
                text: "<think>cut off mid".into(),
            }),
            Err(medical_core::error::AppError::ai_provider(
                "boom".to_string(),
            )),
        ];
        let stripped = ThinkStripStream::new(tokio_stream::iter(chunks));
        let out: Vec<AppResult<StreamChunk>> = stripped.collect().await;

        assert_eq!(
            out.len(),
            1,
            "no Delta — the error is forwarded, text is not"
        );
        assert!(out[0].is_err());
    }
}
