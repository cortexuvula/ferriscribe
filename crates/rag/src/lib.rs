//! # medical-rag
//!
//! Retrieval-augmented generation (RAG) for FerriScribe's clinical AI chat.
//!
//! This crate implements the complete RAG pipeline: document ingestion
//! (chunking, embedding, vector-store persistence, knowledge-graph entity
//! extraction), multi-strategy retrieval (vector similarity, BM25 keyword
//! search, graph traversal), result fusion (reciprocal-rank and weighted),
//! and diversity-aware re-ranking with Maximal Marginal Relevance.
//!
//! ## Module overview
//!
//! | Module | Purpose |
//! |---|---|
//! | [`embeddings`] | HTTP client for local Ollama embedding generation |
//! | [`vector_store`] | SQLite-backed embedding storage and cosine similarity search |
//! | [`bm25`] | SQLite FTS5 full-text keyword search |
//! | [`graph_search`] | SQLite-backed knowledge graph (entities + relations) |
//! | [`fusion`] | Reciprocal-rank fusion and weighted fusion of ranked result sets |
//! | [`mmr`] | Maximal Marginal Relevance re-ranking; cosine/Jaccard similarity |
//! | [`query_expander`] | Medical abbreviation and synonym expansion |
//! | [`ingestion`] | End-to-end ingestion pipeline (chunk → embed → store → extract entities) |
//!
//! ## Crate error type
//!
//! [`RagError`] covers search, embedding, ingestion, and database failures.
//! [`RagResult<T>`] is the crate-level convenience alias.

pub mod bm25;
pub mod embeddings;
pub mod fusion;
pub mod graph_search;
pub mod ingestion;
pub mod mmr;
/// Query expander — not yet wired into production; tests-only until integrated.
#[cfg(test)]
pub mod query_expander;
pub mod vector_store;

use thiserror::Error;

/// Errors that can occur during RAG operations.
///
/// Every fallible function in this crate returns `Result<_, RagError>` (or
/// `AppResult` from `medical-core` when the error must cross crate
/// boundaries, e.g. in the ingestion pipeline).
#[derive(Debug, Error)]
pub enum RagError {
    /// A retrieval or search operation failed.
    #[error("search error: {0}")]
    Search(String),
    /// Embedding generation failed (typically an Ollama HTTP error).
    #[error("embedding error: {0}")]
    Embedding(String),
    /// Document ingestion failed (chunking, entity extraction, or storage).
    #[error("ingestion error: {0}")]
    Ingestion(String),
    /// A database operation failed (SQLite read/write or schema migration).
    #[error("database error: {0}")]
    Database(String),
    /// A search produced no results at all.
    #[error("no results found")]
    NoResults,
}

/// Crate-level result alias: `Result<T, RagError>`.
pub type RagResult<T> = Result<T, RagError>;
