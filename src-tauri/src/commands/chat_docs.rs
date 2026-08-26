//! Conversation-scoped document index for chat "chart review" mode.
//!
//! When the user drops more document text than fits in the model's context
//! (e.g. a 300–600 page chart review), stuffing whole documents is
//! impossible. Instead the conversation's documents are indexed ONCE
//! (chunk → embed → store) into a per-conversation in-memory SQLite
//! database, and every question retrieves the most relevant excerpts via
//! the rag crate's hybrid search (vector + BM25 + RRF) which are stuffed
//! into the system prompt as cited excerpts.
//!
//! Lifecycle matches the once-off semantics agreed for chat documents:
//! nothing is persisted, nothing is synced — the index lives in
//! [`AppState::chat_doc_index`] until the conversation is cleared
//! (`chat_clear_documents`), the document set changes (hash-keyed rebuild),
//! or the app exits. All PHI stays in memory.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::types::rag::{DocumentChunk, RagChunkMetadata};
use medical_core::types::settings::AppConfig;
use medical_db::Database;
use medical_rag::bm25::Bm25Search;
use medical_rag::embeddings::EmbeddingGenerator;
use medical_rag::fusion::reciprocal_rank_fusion;
use medical_rag::ingestion::chunk_text;
use medical_rag::vector_store::VectorStore;
use uuid::Uuid;

use super::chat::ChatDocumentInput;

/// Total document chars at or below which the full-documents (stuffing)
/// mode is used; above it, retrieval. 96k chars = the frontend's
/// CHAT_DOC_BUDGET_TOKENS (24_000) × its 4-chars/token estimate — keep the
/// two in lockstep or the UI promises chart-review mode while the backend
/// still stuffs (or vice versa).
pub(crate) const STUFFING_CHAR_LIMIT: usize = 96_000;

/// Hard ceiling for retrieval mode. Still finite so a runaway client can't
/// OOM the indexer; ~600 dense pages of OCR text is well under it.
pub(crate) const MAX_RETRIEVAL_CHAR_LIMIT: usize = 5_000_000;

const _: () = assert!(STUFFING_CHAR_LIMIT < MAX_RETRIEVAL_CHAR_LIMIT);

/// Chunk shape — the rag crate's proven defaults (200 words, 50 overlap).
const CHUNK_WORDS: usize = 200;
const CHUNK_OVERLAP: usize = 50;

/// Excerpts cited per question, post-fusion.
const TOP_K: usize = 12;

/// A built conversation index. Cheap to query, expensive to build (one
/// embedding HTTP call per chunk), so it is cached in AppState keyed by
/// the document-set hash.
pub(crate) struct ChatDocIndex {
    key: u64,
    vector: VectorStore,
    bm25: Bm25Search,
    /// document_id → source document name, for citations.
    names: HashMap<Uuid, String>,
    /// The generator used at build time — queries MUST use the same model,
    /// so it travels with the index.
    embeddings: EmbeddingGenerator,
}

/// Deterministic key for a document set: same set → same key; any change
/// (including re-OCR of one page) rebuilds.
pub(crate) fn doc_set_key(documents: &[ChatDocumentInput]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for d in documents {
        d.name.hash(&mut hasher);
        d.content.hash(&mut hasher);
    }
    hasher.finish()
}

/// Resolve the embedding generator from config. Both Ollama and LM Studio
/// serve the OpenAI-compatible `/v1/embeddings` endpoint, so the compatible
/// client covers either provider. v1 limitation: hosts are the providers'
/// default ports (custom endpoint hosts are not yet consulted for
/// embeddings — the model-missing error names the host it tried).
pub(crate) fn embeddings_for_config(cfg: &AppConfig) -> AppResult<EmbeddingGenerator> {
    // `embedding_model`'s serde default is a stale OpenAI name no local
    // server has — treat it as unset and use the local-model default.
    let configured = cfg.embedding_model.trim();
    let model = if configured.is_empty() || configured == "text-embedding-3-small" {
        "nomic-embed-text"
    } else {
        configured
    };
    let host = if cfg.ai_provider == "lmstudio" {
        "http://localhost:1234"
    } else {
        "http://localhost:11434"
    };
    EmbeddingGenerator::new_openai_compatible(Some(host), Some(model))
}

impl ChatDocIndex {
    /// Build the index: chunk each document, embed every chunk, store with
    /// vector + FTS. First build for a 600-page chart is dominated by the
    /// embedding calls (bounded-concurrency batch); subsequent questions
    /// only embed the query.
    pub(crate) async fn build(
        documents: &[ChatDocumentInput],
        embeddings: EmbeddingGenerator,
    ) -> AppResult<Self> {
        let db = Arc::new(
            Database::open_in_memory()
                .map_err(|e| AppError::Other(format!("chat doc index db: {e}")))?,
        );
        let vector = VectorStore::new(Arc::clone(&db));
        let bm25 = Bm25Search::new(Arc::clone(&db));
        let display_names = dedupe_names(documents);
        let mut names = HashMap::new();

        for (doc, doc_name) in documents.iter().zip(&display_names) {
            let doc_name = doc_name.clone();
            let chunks = chunk_text(&doc.content, CHUNK_WORDS, CHUNK_OVERLAP);
            if chunks.is_empty() {
                continue;
            }
            // Deterministic id per (deduped) name → stable citation mapping
            // across rebuilds of the same set.
            let document_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, doc_name.as_bytes());
            names.insert(document_id, doc_name.clone());
            let total = chunks.len() as u32;

            let refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
            let vectors = embeddings.embed_batch(&refs).await?;

            for (i, (text, embedding)) in chunks.into_iter().zip(vectors).enumerate() {
                let chunk = DocumentChunk {
                    id: Uuid::new_v4(),
                    document_id,
                    content: text,
                    embedding,
                    chunk_index: i as u32,
                    metadata: RagChunkMetadata {
                        document_title: Some(doc_name.clone()),
                        chunk_index: i as u32,
                        total_chunks: total,
                        page_number: None,
                    },
                };
                vector
                    .store_chunk(&chunk)
                    .map_err(|e| AppError::Other(format!("chat doc index store: {e}")))?;
            }
        }

        Ok(Self {
            key: doc_set_key(documents),
            vector,
            bm25,
            names,
            embeddings,
        })
    }

    /// Same document set as when built?
    pub(crate) fn matches(&self, documents: &[ChatDocumentInput]) -> bool {
        self.key == doc_set_key(documents)
    }

    /// Hybrid retrieval for one question → (document name, content)
    /// excerpts, fused and truncated to [`TOP_K`].
    pub(crate) async fn retrieve(&self, question: &str) -> AppResult<Vec<(String, String)>> {
        let query_vec = self.embeddings.embed(question).await?;
        let vector_hits = self
            .vector
            .search(&query_vec, TOP_K * 2, 0.3)
            .map_err(|e| AppError::Other(format!("chat doc vector search: {e}")))?;
        let bm25_hits = self
            .bm25
            .search(question, TOP_K * 2)
            .map_err(|e| AppError::Other(format!("chat doc bm25 search: {e}")))?;
        let fused = reciprocal_rank_fusion(&[vector_hits, bm25_hits], 60.0);

        Ok(fused
            .into_iter()
            .take(TOP_K)
            .map(|r| {
                let name = self
                    .names
                    .get(&r.document_id)
                    .cloned()
                    .unwrap_or_else(|| "document".to_string());
                (name, r.content)
            })
            .collect())
    }
}

/// Compose the retrieval query for chart-review mode.
///
/// The LATEST user question alone loses conversational referents — "and the
/// trend before that?" retrieves nothing useful. Prepending a slice of the
/// preceding turn (the prior question + the head of the prior answer)
/// restores the anchor terms, which the prior answer almost always
/// contains. Pure; unit-tested. Empty result = no user question at all.
pub(crate) fn compose_retrieval_query(messages: &[super::chat::ChatMessageInput]) -> String {
    /// Head of the previous question kept as context.
    const PRIOR_QUESTION_CAP: usize = 300;
    /// Head of the previous answer kept as context (answers carry the
    /// entity anchors a follow-up refers to).
    const PRIOR_ANSWER_CAP: usize = 500;
    /// Total query cap — comfortably inside embedding-model context
    /// windows (nomic-embed-text: 8k tokens).
    const QUERY_CAP: usize = 2_000;

    let Some(last_pos) = messages.iter().rposition(|m| m.role == "user") else {
        return String::new();
    };
    let last_question = messages[last_pos].content.trim();
    if last_question.is_empty() {
        return String::new();
    }

    // One preceding turn (newest answer + newest question before it),
    // collected newest-first then restored to chronological order.
    let mut parts: Vec<String> = Vec::new();
    for m in messages[..last_pos].iter().rev() {
        if parts.len() >= 2 {
            break;
        }
        let (label, cap) = match m.role.as_str() {
            "assistant" => ("assistant", PRIOR_ANSWER_CAP),
            "user" => ("user", PRIOR_QUESTION_CAP),
            _ => continue,
        };
        let text = m.content.trim();
        if text.is_empty() {
            continue;
        }
        let snippet: String = text.chars().take(cap).collect();
        parts.push(format!("[{label}] {snippet}"));
    }
    parts.reverse();

    let mut query = parts.join("\n");
    if !query.is_empty() {
        query.push('\n');
    }
    query.push_str(last_question);
    query.chars().take(QUERY_CAP).collect()
}

/// Deduplicate display names: two files both named "scan.pdf" would
/// otherwise collapse into one UUIDv5 citation identity. Later duplicates
/// get a " (2)" suffix (originals keep their exact name).
pub(crate) fn dedupe_names(documents: &[ChatDocumentInput]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    documents
        .iter()
        .map(|d| {
            let count = seen.entry(d.name.clone()).or_insert(0);
            *count += 1;
            if *count == 1 {
                d.name.clone()
            } else {
                format!("{} ({})", d.name, count)
            }
        })
        .collect()
}

/// Compose the excerpt section appended to the grounding prompt in
/// retrieval mode. Pure — unit-tested.
pub(crate) fn build_excerpt_section(excerpts: &[(String, String)]) -> String {
    let mut section = String::from(
        "\n\n## Relevant document excerpts\n\n\
         The user's documents are too large to include whole. Below are the \
         excerpts most relevant to their latest question, retrieved by search. \
         Ground the answer in these excerpts and cite the document name. If they \
         do not contain the answer, say so plainly — never guess at unretrieved \
         content.\n\n",
    );
    for (i, (name, content)) in excerpts.iter().enumerate() {
        section.push_str(&format!(
            "--- Excerpt {} — document: {} ---\n{}\n\n",
            i + 1,
            name,
            content
        ));
    }
    section
}

/// Drop the conversation document index (chart-review mode teardown).
/// Called when the chat is cleared; the frontend treats it as best-effort.
/// Nothing else persists, so this is the entire lifecycle.
#[tauri::command]
pub async fn chat_clear_documents(
    state: tauri::State<'_, crate::state::AppState>,
) -> AppResult<()> {
    *state.chat_doc_index.lock().await = None;
    Ok(())
}

/// Total document chars (name + content), the mode-decision input.
pub(crate) fn documents_total_chars(documents: &[ChatDocumentInput]) -> usize {
    documents
        .iter()
        .map(|d| d.name.len() + d.content.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str, content: &str) -> ChatDocumentInput {
        ChatDocumentInput {
            name: name.into(),
            content: content.into(),
        }
    }

    fn msg(role: &str, content: &str) -> super::super::chat::ChatMessageInput {
        super::super::chat::ChatMessageInput {
            role: role.into(),
            content: content.into(),
        }
    }

    #[test]
    fn retrieval_query_carries_prior_turn_anchors_into_follow_ups() {
        let messages = vec![
            msg("user", "What was his latest creatinine?"),
            msg(
                "assistant",
                "The most recent creatinine (2024-03-12) was 112 umol/L.",
            ),
            msg("user", "and the trend before that?"),
        ];
        let q = compose_retrieval_query(&messages);
        // The follow-up keeps its own words AND regains the referent.
        assert!(q.contains("and the trend before that?"), "{q}");
        assert!(q.contains("creatinine"), "anchor from prior answer: {q}");
        assert!(q.contains("[user] What was his latest creatinine?"));
    }

    #[test]
    fn retrieval_query_is_bare_question_when_no_history() {
        let q = compose_retrieval_query(&[msg("user", "lipid history?")]);
        assert_eq!(q, "lipid history?");
    }

    #[test]
    fn retrieval_query_caps_oversized_context() {
        let messages = vec![
            msg("assistant", &"x".repeat(40_000)),
            msg("user", &"y".repeat(40_000)),
            msg("assistant", &"z".repeat(40_000)),
            msg("user", "current question"),
        ];
        let q = compose_retrieval_query(&messages);
        assert!(q.chars().count() <= 2_000, "cap respected");
        assert!(
            q.ends_with("current question"),
            "latest question survives: {q}"
        );
    }

    #[test]
    fn retrieval_query_empty_without_a_user_message() {
        assert_eq!(compose_retrieval_query(&[msg("assistant", "hi")]), "");
    }

    #[test]
    fn duplicate_document_names_get_distinct_citation_names() {
        let names = dedupe_names(&[
            doc("scan.pdf", "first"),
            doc("other.pdf", "x"),
            doc("scan.pdf", "second"),
            doc("scan.pdf", "third"),
        ]);
        assert_eq!(
            names,
            vec![
                "scan.pdf".to_string(),
                "other.pdf".to_string(),
                "scan.pdf (2)".to_string(),
                "scan.pdf (3)".to_string(),
            ]
        );
    }

    #[test]
    fn doc_set_key_is_stable_and_sensitive() {
        let a = vec![doc("a.pdf", "one"), doc("b.pdf", "two")];
        let same = vec![doc("a.pdf", "one"), doc("b.pdf", "two")];
        assert_eq!(doc_set_key(&a), doc_set_key(&same));
        let reordered = vec![doc("b.pdf", "two"), doc("a.pdf", "one")];
        assert_ne!(doc_set_key(&a), doc_set_key(&reordered), "order matters");
        let changed = vec![doc("a.pdf", "one!"), doc("b.pdf", "two")];
        assert_ne!(doc_set_key(&a), doc_set_key(&changed));
    }

    #[test]
    fn excerpt_section_numbers_and_cites_documents() {
        let section = build_excerpt_section(&[
            ("consult.pdf".into(), "Cardiology text".into()),
            ("labs.pdf".into(), "LDL 3.2".into()),
        ]);
        assert!(section.contains("## Relevant document excerpts"));
        assert!(section.contains("--- Excerpt 1 — document: consult.pdf ---"));
        assert!(section.contains("Cardiology text"));
        assert!(section.contains("--- Excerpt 2 — document: labs.pdf ---"));
        assert!(section.contains("never guess"));
    }

    /// End-to-end index + retrieval over a wiremock embeddings server:
    /// proves chunk→embed→store→hybrid-search→citation mapping works
    /// without any real model. The server returns the same vector for every
    /// input, so ranking is driven by BM25.
    #[tokio::test]
    async fn index_build_and_retrieve_maps_excerpts_to_document_names() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(body_partial_json(
                serde_json::json!({ "model": "nomic-embed-text" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "embedding": [1.0, 0.0] }]
            })))
            .mount(&server)
            .await;

        let emb = EmbeddingGenerator::new_openai_compatible(Some(&server.uri()), None)
            .expect("generator");
        let big = "word ".repeat(400); // forces multiple chunks
        let index = ChatDocIndex::build(
            &[
                doc("cardiology.pdf", &format!("{big}echo shows ef 45 percent")),
                doc("labs.pdf", &format!("{big}ldl 3.2 hba1c 7.1")),
            ],
            emb,
        )
        .await
        .expect("index builds");

        // The BM25 half of hybrid search keys on distinctive terms.
        let excerpts = index.retrieve("hba1c").await.expect("retrieve");
        assert!(!excerpts.is_empty(), "must find something");
        assert!(
            excerpts
                .iter()
                .any(|(name, content)| name == "labs.pdf" && content.contains("hba1c")),
            "excerpt must map to the right document: {excerpts:?}"
        );
    }
}
