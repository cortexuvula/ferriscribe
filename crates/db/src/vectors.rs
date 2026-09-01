//! CRUD operations for the `document_chunks` table (RAG vector store).

use std::collections::HashMap;

use rusqlite::{Connection, params};

use crate::DbResult;

/// A single document chunk with its embedding vector.
///
/// The `embedding` field is `None` when the chunk has not yet been embedded.
#[derive(Debug, Clone)]
pub struct DocumentChunk {
    pub id: String,
    pub document_id: String,
    pub content: String,
    pub embedding: Option<Vec<f32>>,
    pub chunk_index: i64,
    pub metadata: String,
    pub created_at: String,
}

/// Lightweight record returned by [`VectorsRepo::get_all_embeddings`].
///
/// Only includes chunks that have a non-NULL embedding.
#[derive(Debug, Clone)]
pub struct EmbeddingRecord {
    pub id: String,
    pub document_id: String,
    pub content: String,
    pub embedding: Vec<f32>,
}

/// Lightweight record for the scoring-only pass of vector search —
/// excludes content to avoid loading large strings for the whole corpus.
/// Content is hydrated only for the top-k winners via [`VectorsRepo::get_content_by_ids`].
#[derive(Debug, Clone)]
pub struct EmbeddingVectorRecord {
    pub id: String,
    pub document_id: String,
    pub embedding: Vec<f32>,
}

/// Result of an FTS5 full-text search via [`VectorsRepo::search_fts`].
///
/// Higher `rank` values indicate better matches (BM25 score negated so
/// higher = better).
#[derive(Debug, Clone)]
pub struct FtsResult {
    pub id: String,
    /// Owning document for the chunk — lets BM25 results carry the same
    /// document identity as vector results (used for chat citations).
    pub document_id: String,
    pub content: String,
    pub rank: f64,
}

/// Repository for the `document_chunks` table (RAG vector store).
///
/// Stores text chunks with optional `f32` embedding vectors serialized as
/// `BLOB` via `bytemuck`. Also provides FTS5 full-text search through the
/// companion `chunks_fts` virtual table.
pub struct VectorsRepo;

impl VectorsRepo {
    pub fn new() -> Self {
        Self
    }

    // ------------------------------------------------------------------
    // Write operations
    // ------------------------------------------------------------------

    /// Insert (or replace) a document chunk with an optional embedding.
    ///
    /// The `Vec<f32>` embedding is serialised to a `BLOB` using `bytemuck`.
    /// Uses `INSERT OR REPLACE` so re-inserting the same `id` overwrites
    /// the previous row.
    pub fn insert_chunk(
        conn: &Connection,
        id: &str,
        document_id: &str,
        content: &str,
        embedding: Option<&[f32]>,
        chunk_index: i64,
        metadata: &str,
    ) -> DbResult<()> {
        let blob: Option<Vec<u8>> = embedding.map(|e| bytemuck::cast_slice(e).to_vec());

        conn.execute(
            "INSERT OR REPLACE INTO document_chunks
                (id, document_id, content, embedding, chunk_index, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, document_id, content, blob, chunk_index, metadata],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Read operations
    // ------------------------------------------------------------------

    /// Return every chunk that has a non-NULL embedding.
    ///
    /// Each `BLOB` is deserialised back into `Vec<f32>` via `bytemuck`.
    /// Chunks without embeddings are excluded. Chunks whose embedding BLOB
    /// cannot be deserialised (e.g. wrong length / corrupt) are skipped with
    /// a warning rather than silently returning an empty vector, which would
    /// pollute search results with a zero-similarity entry.
    pub fn get_all_embeddings(conn: &Connection) -> DbResult<Vec<EmbeddingRecord>> {
        let mut stmt = conn.prepare(
            "SELECT id, document_id, content, embedding
             FROM document_chunks
             WHERE embedding IS NOT NULL",
        )?;

        // Read the raw columns first (real DB errors propagate via `?`),
        // then cast the BLOB outside the query_map closure so corrupt
        // embeddings can be skipped individually instead of poisoning the
        // whole result set with empty vectors.
        let raw: Vec<(String, String, String, Vec<u8>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut rows = Vec::with_capacity(raw.len());
        for (id, document_id, content, blob) in raw {
            let embedding = match bytemuck::try_cast_slice::<u8, f32>(&blob) {
                Ok(slice) => slice.to_vec(),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        doc_len = blob.len(),
                        chunk_id = %id,
                        "corrupt embedding blob, skipping"
                    );
                    continue;
                }
            };
            rows.push(EmbeddingRecord {
                id,
                document_id,
                content,
                embedding,
            });
        }

        Ok(rows)
    }

    /// Return every chunk's id, document_id, and embedding (no content).
    ///
    /// Used by the vector search's first phase (score-only pass) to avoid
    /// loading potentially large content strings for the entire corpus.
    /// Content is fetched only for the top-k winners via [`get_content_by_ids`].
    /// Corrupt embedding BLOBs are skipped with a warning (same policy as
    /// [`get_all_embeddings`]).
    pub fn get_all_embedding_vectors(conn: &Connection) -> DbResult<Vec<EmbeddingVectorRecord>> {
        let mut stmt = conn.prepare(
            "SELECT id, document_id, embedding
             FROM document_chunks
             WHERE embedding IS NOT NULL",
        )?;

        let raw: Vec<(String, String, Vec<u8>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut rows = Vec::with_capacity(raw.len());
        for (id, document_id, blob) in raw {
            let embedding = match bytemuck::try_cast_slice::<u8, f32>(&blob) {
                Ok(slice) => slice.to_vec(),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        doc_len = blob.len(),
                        chunk_id = %id,
                        "corrupt embedding blob, skipping"
                    );
                    continue;
                }
            };
            rows.push(EmbeddingVectorRecord {
                id,
                document_id,
                embedding,
            });
        }

        Ok(rows)
    }

    /// Fetch content for specific chunk IDs (used after the top-k scoring
    /// pass to hydrate only the winning results).
    pub fn get_content_by_ids(
        conn: &Connection,
        ids: &[String],
    ) -> DbResult<HashMap<String, String>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, content FROM document_chunks WHERE id IN ({placeholders})");
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(params.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().collect())
    }

    /// Retrieve all chunks belonging to a given document, ordered by
    /// `chunk_index ASC`.
    ///
    /// A corrupt embedding BLOB (one whose byte length is not a multiple of
    /// 4) surfaces as an error rather than silently returning an empty
    /// vector — unlike the `get_all_embeddings` bulk loaders, this is a
    /// targeted single-document read where the caller should know the data
    /// is damaged.
    pub fn get_by_document(conn: &Connection, document_id: &str) -> DbResult<Vec<DocumentChunk>> {
        let mut stmt = conn.prepare(
            "SELECT id, document_id, content, embedding, chunk_index, metadata, created_at
             FROM document_chunks
             WHERE document_id = ?1
             ORDER BY chunk_index ASC",
        )?;

        let rows = stmt
            .query_map([document_id], |row| {
                let blob: Option<Vec<u8>> = row.get(3)?;
                let embedding = match blob {
                    None => None,
                    Some(b) => match bytemuck::try_cast_slice::<u8, f32>(&b) {
                        Ok(slice) => Some(slice.to_vec()),
                        Err(e) => {
                            tracing::warn!(error = %e, doc_len = b.len(), "corrupt embedding blob");
                            return Err(rusqlite::Error::FromSqlConversionFailure(
                                3,
                                rusqlite::types::Type::Blob,
                                Box::<dyn std::error::Error + Send + Sync>::from(format!(
                                    "corrupt embedding: {e}"
                                )),
                            ));
                        }
                    },
                };

                Ok(DocumentChunk {
                    id: row.get(0)?,
                    document_id: row.get(1)?,
                    content: row.get(2)?,
                    embedding,
                    chunk_index: row.get(4)?,
                    metadata: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Total number of rows in `document_chunks` (with or without embeddings).
    pub fn count(conn: &Connection) -> DbResult<u32> {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM document_chunks", [], |r| r.get(0))?;
        Ok(n as u32)
    }

    /// Full-text search via the FTS5 index.
    ///
    /// Returns up to `top_k` results ranked by BM25 relevance. The `rank`
    /// field in [`FtsResult`] is negated so higher values = better matches.
    pub fn search_fts(conn: &Connection, query: &str, top_k: u32) -> DbResult<Vec<FtsResult>> {
        let mut stmt = conn.prepare(
            "SELECT dc.id, dc.document_id, dc.content, f.rank
             FROM chunks_fts f
             JOIN document_chunks dc ON dc.rowid = f.rowid
             WHERE chunks_fts MATCH ?1
             ORDER BY f.rank
             LIMIT ?2",
        )?;

        let rows = stmt
            .query_map(params![query, top_k], |row| {
                Ok(FtsResult {
                    id: row.get(0)?,
                    document_id: row.get(1)?,
                    content: row.get(2)?,
                    rank: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Delete operations
    // ------------------------------------------------------------------

    /// Delete all chunks belonging to a document.
    ///
    /// Returns the number of rows removed (0 if the document had no chunks).
    pub fn delete_by_document(conn: &Connection, document_id: &str) -> DbResult<u32> {
        let deleted = conn.execute(
            "DELETE FROM document_chunks WHERE document_id = ?1",
            [document_id],
        )?;
        Ok(deleted as u32)
    }
}

impl Default for VectorsRepo {
    fn default() -> Self {
        Self::new()
    }
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::MigrationEngine;
    use rusqlite::Connection;

    fn migrated_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        MigrationEngine::migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn insert_and_count() {
        let conn = migrated_conn();
        assert_eq!(VectorsRepo::count(&conn).unwrap(), 0);

        VectorsRepo::insert_chunk(
            &conn,
            "c1",
            "doc1",
            "Hello world",
            Some(&[1.0, 2.0, 3.0]),
            0,
            "{}",
        )
        .unwrap();

        assert_eq!(VectorsRepo::count(&conn).unwrap(), 1);
    }

    #[test]
    fn insert_and_retrieve_embedding() {
        let conn = migrated_conn();
        let emb = vec![0.1_f32, 0.2, 0.3, 0.4];

        VectorsRepo::insert_chunk(&conn, "c1", "doc1", "test content", Some(&emb), 0, "{}")
            .unwrap();

        let all = VectorsRepo::get_all_embeddings(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "c1");
        assert_eq!(all[0].content, "test content");
        assert_eq!(all[0].embedding, emb);
    }

    #[test]
    fn null_embedding_excluded_from_get_all() {
        let conn = migrated_conn();

        // Insert one with embedding, one without.
        VectorsRepo::insert_chunk(
            &conn,
            "c1",
            "doc1",
            "has embedding",
            Some(&[1.0, 2.0]),
            0,
            "{}",
        )
        .unwrap();
        VectorsRepo::insert_chunk(&conn, "c2", "doc1", "no embedding", None, 1, "{}").unwrap();

        let all = VectorsRepo::get_all_embeddings(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "c1");
    }

    #[test]
    fn get_by_document_ordered() {
        let conn = migrated_conn();

        // Insert out of order.
        VectorsRepo::insert_chunk(&conn, "c2", "doc1", "second chunk", None, 1, "{}").unwrap();
        VectorsRepo::insert_chunk(&conn, "c1", "doc1", "first chunk", None, 0, "{}").unwrap();

        let chunks = VectorsRepo::get_by_document(&conn, "doc1").unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].id, "c1");
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[1].id, "c2");
        assert_eq!(chunks[1].chunk_index, 1);
    }

    #[test]
    fn delete_by_document() {
        let conn = migrated_conn();

        VectorsRepo::insert_chunk(&conn, "c1", "doc1", "chunk a", None, 0, "{}").unwrap();
        VectorsRepo::insert_chunk(&conn, "c2", "doc1", "chunk b", None, 1, "{}").unwrap();
        VectorsRepo::insert_chunk(&conn, "c3", "doc2", "other doc", None, 0, "{}").unwrap();

        let deleted = VectorsRepo::delete_by_document(&conn, "doc1").unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(VectorsRepo::count(&conn).unwrap(), 1);
    }

    #[test]
    fn delete_nonexistent_returns_zero() {
        let conn = migrated_conn();
        let deleted = VectorsRepo::delete_by_document(&conn, "nope").unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn fts_search() {
        let conn = migrated_conn();

        VectorsRepo::insert_chunk(
            &conn,
            "c1",
            "doc1",
            "the patient has diabetes mellitus",
            None,
            0,
            "{}",
        )
        .unwrap();
        VectorsRepo::insert_chunk(
            &conn,
            "c2",
            "doc1",
            "hypertension treatment protocol",
            None,
            1,
            "{}",
        )
        .unwrap();
        VectorsRepo::insert_chunk(
            &conn,
            "c3",
            "doc2",
            "diabetes management guidelines",
            None,
            0,
            "{}",
        )
        .unwrap();

        let results = VectorsRepo::search_fts(&conn, "diabetes", 10).unwrap();
        assert_eq!(results.len(), 2);
        // Both results should mention diabetes-related content.
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"c1"));
        assert!(ids.contains(&"c3"));
    }

    #[test]
    fn fts_search_top_k_limit() {
        let conn = migrated_conn();

        for i in 0..5 {
            let id = format!("c{i}");
            let content = format!("medical record number {i}");
            VectorsRepo::insert_chunk(&conn, &id, "doc1", &content, None, i, "{}").unwrap();
        }

        let results = VectorsRepo::search_fts(&conn, "medical", 2).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn insert_or_replace_overwrites() {
        let conn = migrated_conn();

        VectorsRepo::insert_chunk(&conn, "c1", "doc1", "original", Some(&[1.0]), 0, "{}").unwrap();

        VectorsRepo::insert_chunk(&conn, "c1", "doc1", "updated", Some(&[2.0]), 0, "{}").unwrap();

        assert_eq!(VectorsRepo::count(&conn).unwrap(), 1);

        let all = VectorsRepo::get_all_embeddings(&conn).unwrap();
        assert_eq!(all[0].content, "updated");
        assert_eq!(all[0].embedding, vec![2.0_f32]);
    }
}
