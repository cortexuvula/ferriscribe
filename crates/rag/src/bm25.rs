use std::sync::Arc;

use uuid::Uuid;

use medical_core::types::rag::{RagChunkMetadata, RagResult, SearchSource};
use medical_db::Database;
use medical_db::vectors::VectorsRepo;

use crate::RagError;

/// BM25 full-text search backed by SQLite FTS5 via `VectorsRepo::search_fts`.
///
/// FTS5 rank values are negative (more negative = better match).
/// We normalize to a positive score: `score = 1.0 / (1.0 + rank.abs())`.
pub struct Bm25Search {
    db: Arc<Database>,
}

impl Bm25Search {
    /// Create a new `Bm25Search` backed by the given database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Search the FTS5 index for documents matching `query`, returning up to
    /// `top_k` results ranked by BM25 relevance.
    pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<RagResult>, RagError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let conn = self
            .db
            .conn()
            .map_err(|e| RagError::Database(e.to_string()))?;

        let fts_results = VectorsRepo::search_fts(&conn, query, top_k as u32)
            .map_err(|e| RagError::Database(e.to_string()))?;

        let results = fts_results
            .into_iter()
            .map(|fts| {
                // FTS5 rank is negative (more negative = better match).
                // Normalize to a positive score in (0, 1] where higher = better.
                // Use -rank so that more negative ranks produce higher scores.
                let score = if fts.rank < 0.0 {
                    (-fts.rank / (1.0 + (-fts.rank))) as f32
                } else {
                    0.01_f32 // shouldn't happen, but safe fallback
                };

                let chunk_id = Uuid::parse_str(&fts.id).unwrap_or(Uuid::nil());
                let document_id = Uuid::parse_str(&fts.document_id).unwrap_or(Uuid::nil());

                RagResult {
                    chunk_id,
                    document_id,
                    content: fts.content,
                    score,
                    source: SearchSource::Bm25,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_db::vectors::VectorsRepo;

    fn make_db() -> Arc<Database> {
        Arc::new(Database::open_in_memory().expect("open in-memory DB"))
    }

    /// Helper: insert a chunk directly via VectorsRepo for test setup.
    fn insert_test_chunk(db: &Database, id: &str, doc_id: &str, content: &str, index: i64) {
        let conn = db.conn().expect("conn");
        VectorsRepo::insert_chunk(&conn, id, doc_id, content, None, index, "{}")
            .expect("insert chunk");
    }

    #[test]
    fn search_finds_matching_documents() {
        let db = make_db();

        insert_test_chunk(&db, "c1", "doc1", "the patient has diabetes mellitus", 0);
        insert_test_chunk(&db, "c2", "doc1", "hypertension treatment protocol", 1);
        insert_test_chunk(&db, "c3", "doc2", "diabetes management guidelines", 0);

        let bm25 = Bm25Search::new(db);
        let results = bm25.search("diabetes", 10).expect("search");

        assert_eq!(results.len(), 2);
        let ids: Vec<Uuid> = results.iter().map(|r| r.chunk_id).collect();
        assert!(ids.contains(&Uuid::parse_str("c1").unwrap_or(Uuid::nil())));
        // Both results should have source Bm25
        assert!(results.iter().all(|r| r.source == SearchSource::Bm25));
    }

    #[test]
    fn search_returns_positive_scores() {
        let db = make_db();

        insert_test_chunk(&db, "c1", "doc1", "medical record for diabetes patient", 0);

        let bm25 = Bm25Search::new(db);
        let results = bm25.search("diabetes", 10).expect("search");

        assert_eq!(results.len(), 1);
        assert!(
            results[0].score > 0.0,
            "score should be positive, got {}",
            results[0].score
        );
        assert!(
            results[0].score <= 1.0,
            "score should be <= 1.0, got {}",
            results[0].score
        );
    }

    #[test]
    fn search_respects_top_k() {
        let db = make_db();

        for i in 0..5 {
            let id = format!("c{i}");
            let content = format!("medical record number {i}");
            insert_test_chunk(&db, &id, "doc1", &content, i);
        }

        let bm25 = Bm25Search::new(db);
        let results = bm25.search("medical", 2).expect("search");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_no_match_returns_empty() {
        let db = make_db();

        insert_test_chunk(&db, "c1", "doc1", "hypertension treatment", 0);

        let bm25 = Bm25Search::new(db);
        let results = bm25.search("xyznonexistent", 10).expect("search");
        assert!(results.is_empty());
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let db = make_db();

        insert_test_chunk(&db, "c1", "doc1", "some content", 0);

        let bm25 = Bm25Search::new(db);
        let results = bm25.search("", 10).expect("search");
        assert!(results.is_empty());
    }

    #[test]
    fn search_empty_store_returns_empty() {
        let db = make_db();
        let bm25 = Bm25Search::new(db);
        let results = bm25.search("anything", 10).expect("search");
        assert!(results.is_empty());
    }
}
