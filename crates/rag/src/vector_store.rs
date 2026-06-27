use std::sync::Arc;

use uuid::Uuid;

use medical_core::types::rag::{DocumentChunk, RagChunkMetadata, RagResult, SearchSource};
use medical_db::Database;
use medical_db::vectors::VectorsRepo;

use crate::RagError;
use crate::mmr::cosine_similarity;

/// Vector store backed by SQLite via `medical_db::vectors::VectorsRepo`.
///
/// Stores document chunk embeddings and performs brute-force cosine similarity
/// search over all stored embeddings.
pub struct VectorStore {
    db: Arc<Database>,
}

impl VectorStore {
    /// Create a new `VectorStore` backed by the given database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Persist a document chunk with its embedding into SQLite.
    pub fn store_chunk(&self, chunk: &DocumentChunk) -> Result<(), RagError> {
        let conn = self
            .db
            .conn()
            .map_err(|e| RagError::Database(e.to_string()))?;

        let metadata_json = serde_json::to_string(&chunk.metadata)
            .map_err(|e| RagError::Database(format!("failed to serialize metadata: {e}")))?;

        let embedding_ref: Option<&[f32]> = if chunk.embedding.is_empty() {
            None
        } else {
            Some(&chunk.embedding)
        };

        VectorsRepo::insert_chunk(
            &conn,
            &chunk.id.to_string(),
            &chunk.document_id.to_string(),
            &chunk.content,
            embedding_ref,
            chunk.chunk_index as i64,
            &metadata_json,
        )
        .map_err(|e| RagError::Database(e.to_string()))?;

        Ok(())
    }

    /// Search for the closest chunks to a query embedding vector.
    ///
    /// Loads embedding vectors (no content) from the database, computes
    /// cosine similarity against each, filters by `threshold`, keeps the
    /// top `top_k` via a bounded selection, then hydrates content only for
    /// those winners. This avoids loading the entire corpus's content
    /// strings into memory on every query.
    pub fn search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        threshold: f32,
    ) -> Result<Vec<RagResult>, RagError> {
        let conn = self
            .db
            .conn()
            .map_err(|e| RagError::Database(e.to_string()))?;

        // Phase 1: score only (no content loaded — avoids deserializing
        // large text strings for the entire corpus).
        let vectors = VectorsRepo::get_all_embedding_vectors(&conn)
            .map_err(|e| RagError::Database(e.to_string()))?;

        let mut scored: Vec<(f32, &medical_db::vectors::EmbeddingVectorRecord)> = vectors
            .iter()
            .map(|rec| {
                let sim = cosine_similarity(query_embedding, &rec.embedding);
                (sim, rec)
            })
            .filter(|(sim, _)| *sim >= threshold)
            .collect();

        // Sort by score descending and take top_k.
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        if scored.is_empty() {
            return Ok(vec![]);
        }

        // Phase 2: hydrate content only for the top-k winners.
        let ids: Vec<String> = scored.iter().map(|(_, r)| r.id.clone()).collect();
        let content_map = VectorsRepo::get_content_by_ids(&conn, &ids)
            .map_err(|e| RagError::Database(e.to_string()))?;

        let results = scored
            .into_iter()
            .map(|(score, rec)| {
                let chunk_id = Uuid::parse_str(&rec.id).unwrap_or(Uuid::nil());
                let content = content_map.get(&rec.id).cloned().unwrap_or_default();

                RagResult {
                    chunk_id,
                    document_id: Uuid::parse_str(&rec.document_id).unwrap_or(Uuid::nil()),
                    content,
                    score,
                    source: SearchSource::Vector,
                    metadata: RagChunkMetadata {
                        document_title: None,
                        chunk_index: 0,
                        total_chunks: 0,
                        page_number: None,
                    },
                }
            })
            .collect();

        Ok(results)
    }

    /// Delete all chunks belonging to a document.
    pub fn delete_document(&self, document_id: &Uuid) -> Result<(), RagError> {
        let conn = self
            .db
            .conn()
            .map_err(|e| RagError::Database(e.to_string()))?;

        VectorsRepo::delete_by_document(&conn, &document_id.to_string())
            .map_err(|e| RagError::Database(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db() -> Arc<Database> {
        Arc::new(Database::open_in_memory().expect("open in-memory DB"))
    }

    fn make_chunk(
        id: u128,
        doc_id: u128,
        content: &str,
        embedding: Vec<f32>,
        index: u32,
    ) -> DocumentChunk {
        DocumentChunk {
            id: Uuid::from_u128(id),
            document_id: Uuid::from_u128(doc_id),
            content: content.to_string(),
            embedding,
            chunk_index: index,
            metadata: RagChunkMetadata {
                document_title: Some("Test Document".to_string()),
                chunk_index: index,
                total_chunks: 1,
                page_number: None,
            },
        }
    }

    #[test]
    fn store_and_search_returns_results() {
        let db = make_db();
        let store = VectorStore::new(db);

        // Store two chunks with different embeddings
        let chunk1 = make_chunk(1, 100, "diabetes management", vec![1.0, 0.0, 0.0], 0);
        let chunk2 = make_chunk(2, 100, "hypertension treatment", vec![0.0, 1.0, 0.0], 1);
        let chunk3 = make_chunk(3, 200, "diabetes insulin therapy", vec![0.9, 0.1, 0.0], 0);

        store.store_chunk(&chunk1).expect("store chunk1");
        store.store_chunk(&chunk2).expect("store chunk2");
        store.store_chunk(&chunk3).expect("store chunk3");

        // Search with an embedding close to chunk1 and chunk3
        let query = vec![1.0, 0.0, 0.0];
        let results = store.search(&query, 10, 0.0).expect("search");

        assert!(!results.is_empty(), "should return at least one result");
        // chunk1 should be ranked first (exact match)
        assert_eq!(results[0].chunk_id, Uuid::from_u128(1));
        assert!(
            (results[0].score - 1.0).abs() < 1e-5,
            "exact match should have score ~1.0"
        );
    }

    #[test]
    fn search_respects_threshold() {
        let db = make_db();
        let store = VectorStore::new(db);

        let chunk1 = make_chunk(1, 100, "relevant content", vec![1.0, 0.0], 0);
        let chunk2 = make_chunk(2, 100, "irrelevant content", vec![0.0, 1.0], 1);

        store.store_chunk(&chunk1).expect("store chunk1");
        store.store_chunk(&chunk2).expect("store chunk2");

        // With a high threshold, only the highly similar chunk should be returned
        let query = vec![1.0, 0.0];
        let results = store.search(&query, 10, 0.9).expect("search");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk_id, Uuid::from_u128(1));
    }

    #[test]
    fn search_respects_top_k() {
        let db = make_db();
        let store = VectorStore::new(db);

        for i in 0..5 {
            let chunk = make_chunk(i, 100, &format!("chunk {i}"), vec![1.0, 0.0], i as u32);
            store.store_chunk(&chunk).expect("store chunk");
        }

        let query = vec![1.0, 0.0];
        let results = store.search(&query, 2, 0.0).expect("search");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_orders_by_score_descending() {
        let db = make_db();
        let store = VectorStore::new(db);

        // Embeddings with decreasing similarity to [1, 0, 0]
        let chunk1 = make_chunk(1, 100, "most similar", vec![1.0, 0.0, 0.0], 0);
        let chunk2 = make_chunk(2, 100, "somewhat similar", vec![0.7, 0.7, 0.0], 1);
        let chunk3 = make_chunk(3, 100, "least similar", vec![0.0, 0.0, 1.0], 2);

        store.store_chunk(&chunk1).expect("store");
        store.store_chunk(&chunk2).expect("store");
        store.store_chunk(&chunk3).expect("store");

        let query = vec![1.0, 0.0, 0.0];
        let results = store.search(&query, 10, 0.0).expect("search");

        assert_eq!(results.len(), 3);
        assert!(results[0].score >= results[1].score);
        assert!(results[1].score >= results[2].score);
    }

    #[test]
    fn delete_document_removes_chunks() {
        let db = make_db();
        let store = VectorStore::new(db);

        let doc_id: u128 = 100;
        let chunk1 = make_chunk(1, doc_id, "chunk a", vec![1.0, 0.0], 0);
        let chunk2 = make_chunk(2, doc_id, "chunk b", vec![0.0, 1.0], 1);
        let chunk3 = make_chunk(3, 200, "other doc chunk", vec![0.5, 0.5], 0);

        store.store_chunk(&chunk1).expect("store");
        store.store_chunk(&chunk2).expect("store");
        store.store_chunk(&chunk3).expect("store");

        store
            .delete_document(&Uuid::from_u128(doc_id))
            .expect("delete");

        // Only chunk3 (from doc 200) should remain
        let query = vec![1.0, 0.0];
        let results = store.search(&query, 10, 0.0).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk_id, Uuid::from_u128(3));
    }

    #[test]
    fn search_empty_store_returns_empty() {
        let db = make_db();
        let store = VectorStore::new(db);

        let query = vec![1.0, 0.0, 0.0];
        let results = store.search(&query, 10, 0.0).expect("search");
        assert!(results.is_empty());
    }
}
