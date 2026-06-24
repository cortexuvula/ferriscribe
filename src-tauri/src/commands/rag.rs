use std::sync::Arc;

use medical_core::error::{AppError, AppResult};
use medical_core::types::rag::RagResult;
use medical_db::recordings::RecordingsRepo;
use medical_db::vectors::VectorsRepo;
use medical_rag::fusion::reciprocal_rank_fusion;
use serde::Serialize;
use uuid::Uuid;

use crate::state::AppState;

/// Response for the ingest_document command.
#[derive(Serialize)]
pub struct IngestResult {
    pub recording_id: String,
    pub chunks_created: u32,
}

/// Response for the rag_stats command.
#[derive(Serialize)]
pub struct RagStats {
    pub chunk_count: u32,
    pub entity_count: u32,
}

/// Ingest a recording's transcript into the RAG knowledge base.
///
/// Chunks the transcript text, generates embeddings, stores in the vector
/// store, and extracts medical entities into the knowledge graph.
#[tauri::command]
pub async fn ingest_document(
    state: tauri::State<'_, AppState>,
    recording_id: String,
) -> AppResult<IngestResult> {
    let uuid = Uuid::parse_str(&recording_id)
        .map_err(|e| AppError::Other(format!("invalid recording id: {e}")))?;

    // Load the recording from the database on a blocking thread.
    let db = Arc::clone(&state.db);
    let recording = tokio::task::spawn_blocking(move || {
        let conn = db.conn().map_err(|e| AppError::Database(e.to_string()))?;
        RecordingsRepo::get_by_id(&conn, &uuid).map_err(|e| AppError::Database(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Other(format!("Task join error: {e}")))??;

    // Extract transcript text
    let transcript = recording
        .transcript
        .as_deref()
        .ok_or_else(|| AppError::Rag("Recording has no transcript to ingest".to_string()))?;

    if transcript.trim().is_empty() {
        return Err(AppError::Rag("Recording transcript is empty".to_string()));
    }

    let title = recording
        .patient_name
        .as_deref()
        .unwrap_or(&recording.filename);

    // Run the ingestion pipeline (async — involves embedding API calls)
    let chunks_created = state
        .ingestion
        .ingest_text(uuid, title, transcript)
        .await
        .map_err(|e| {
            AppError::Rag(format!(
                "Ingestion failed: {}",
                super::unwrap_app_error_message(e)
            ))
        })?;

    Ok(IngestResult {
        recording_id,
        chunks_created,
    })
}

/// Search the RAG knowledge base with hybrid retrieval.
///
/// Performs vector similarity search + BM25 full-text search, then
/// fuses results using Reciprocal Rank Fusion.
#[tauri::command]
pub async fn search_rag(
    state: tauri::State<'_, AppState>,
    query: String,
    top_k: Option<u32>,
) -> AppResult<Vec<RagResult>> {
    let top_k = top_k.unwrap_or(5) as usize;
    let fetch_k = top_k * 2;

    // 1. Embed the query (async — calls embedding API)
    let query_embedding = state.embedding_generator.embed(&query).await.map_err(|e| {
        AppError::Rag(format!(
            "Embedding failed: {}",
            super::unwrap_app_error_message(e)
        ))
    })?;

    // 2. Vector + BM25 search on blocking threads (both hit SQLite)
    let vs = Arc::clone(&state.vector_store);
    let embedding_clone = query_embedding.clone();
    let vector_handle = tokio::task::spawn_blocking(move || {
        vs.search(&embedding_clone, fetch_k, 0.3)
            .map_err(|e| AppError::Rag(format!("Vector search failed: {e}")))
    });

    let bm25 = Arc::clone(&state.bm25_search);
    let query_clone = query.clone();
    let bm25_handle = tokio::task::spawn_blocking(move || {
        bm25.search(&query_clone, fetch_k)
            .map_err(|e| AppError::Rag(format!("BM25 search failed: {e}")))
    });

    // Await both searches concurrently
    let (vector_results, bm25_results) = tokio::try_join!(
        async {
            vector_handle
                .await
                .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
        },
        async {
            bm25_handle
                .await
                .map_err(|e| AppError::Other(format!("Task join error: {e}")))?
        },
    )?;

    // 4. Fuse with RRF
    let mut fused = reciprocal_rank_fusion(&[vector_results, bm25_results], 60.0);

    // 5. Truncate to top_k
    fused.truncate(top_k);

    Ok(fused)
}

/// Return statistics about the RAG knowledge base.
#[tauri::command]
pub fn rag_stats(state: tauri::State<'_, AppState>) -> AppResult<RagStats> {
    let conn = state
        .db
        .conn()
        .map_err(|e| AppError::Database(e.to_string()))?;

    let chunk_count = VectorsRepo::count(&conn).map_err(|e| AppError::Database(e.to_string()))?;

    // Count graph entities via a direct query (GraphSearch doesn't expose count)
    let entity_count: u32 = conn
        .query_row("SELECT COUNT(*) FROM graph_entities", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_or(0) as u32;

    Ok(RagStats {
        chunk_count,
        entity_count,
    })
}
