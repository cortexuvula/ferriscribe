//! RAG (retrieval-augmented generation) knowledge-base search tool.
//!
//! Bridges the `medical-rag` crate into the agent tool system. When
//! constructed with [`RagSearchTool::with_rag()`], performs real hybrid
//! search: embed query → vector similarity + BM25 keyword search →
//! reciprocal rank fusion → top-k results. When constructed with
//! [`RagSearchTool::new()`], returns a stub "not yet connected" message.

use std::sync::Arc;

use async_trait::async_trait;
use medical_core::{
    error::AppResult,
    traits::Tool,
    types::{ToolDef, ToolOutput},
};
use serde_json::json;
use tracing::info;

use medical_rag::bm25::Bm25Search;
use medical_rag::embeddings::EmbeddingGenerator;
use medical_rag::fusion::reciprocal_rank_fusion;
use medical_rag::vector_store::VectorStore;

/// Tool for querying the medical knowledge base (RAG system).
///
/// When constructed with `new()`, returns a stub "not configured" message.
/// When constructed with `with_rag(...)`, performs real hybrid search:
/// embed -> vector search + BM25 -> reciprocal rank fusion -> results.
pub struct RagSearchTool {
    embeddings: Option<Arc<EmbeddingGenerator>>,
    vector_store: Option<Arc<VectorStore>>,
    bm25: Option<Arc<Bm25Search>>,
}

impl Default for RagSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl RagSearchTool {
    /// Create an unconfigured tool that returns a "not connected" message.
    pub fn new() -> Self {
        Self {
            embeddings: None,
            vector_store: None,
            bm25: None,
        }
    }

    /// Create a tool wired to real RAG backends.
    pub fn with_rag(
        embeddings: Arc<EmbeddingGenerator>,
        vector_store: Arc<VectorStore>,
        bm25: Arc<Bm25Search>,
    ) -> Self {
        Self {
            embeddings: Some(embeddings),
            vector_store: Some(vector_store),
            bm25: Some(bm25),
        }
    }

    /// Whether this tool is connected to a real RAG backend.
    pub fn is_configured(&self) -> bool {
        self.embeddings.is_some() && self.vector_store.is_some() && self.bm25.is_some()
    }
}

#[async_trait]
impl Tool for RagSearchTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "search_knowledge_base".into(),
            description: "Search the medical knowledge base using semantic similarity. Returns relevant clinical documents and evidence for the given query.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The clinical question or search query"
                    },
                    "top_k": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 5)",
                        "default": 5,
                        "minimum": 1,
                        "maximum": 20
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> AppResult<ToolOutput> {
        let query = match arguments.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return Ok(ToolOutput::error("query parameter is required")),
        };

        let top_k = arguments.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        // If RAG is not configured, return an informational stub.
        // Use an if-let guard (rather than is_configured() + unwrap) so the
        // check and the borrow happen atomically — eliminates any theoretical
        // panic risk from concurrent reconfiguration and keeps unwraps off the
        // agent hot path.
        let (embeddings, vector_store, bm25) = match (
            &self.embeddings,
            &self.vector_store,
            &self.bm25,
        ) {
            (Some(embeddings), Some(vector_store), Some(bm25)) => (embeddings, vector_store, bm25),
            _ => {
                let content = serde_json::to_string_pretty(&json!({
                        "query": query,
                        "top_k": top_k,
                        "results": [],
                        "note": "Knowledge base search is not yet connected. Results will be available once the RAG system is initialized."
                    }))
                    .unwrap_or_else(|_| "serialization error".into());

                return Ok(ToolOutput::success(content));
            }
        };

        // 1. Embed the query
        let query_embedding = embeddings.embed(query).await?;

        // 2. Search vector store (request extra to allow for fusion/reranking)
        let fetch_k = top_k * 2;
        let vector_results = vector_store
            .search(&query_embedding, fetch_k, 0.3)
            .map_err(|e| medical_core::error::AppError::rag(format!("Vector search: {e}")))?;

        // 3. Search BM25
        let bm25_results = bm25
            .search(query, fetch_k)
            .map_err(|e| medical_core::error::AppError::rag(format!("BM25 search: {e}")))?;

        info!(
            vector_count = vector_results.len(),
            bm25_count = bm25_results.len(),
            "RAG search completed"
        );

        // 4. Fuse results with Reciprocal Rank Fusion (k=60)
        let mut fused = reciprocal_rank_fusion(&[vector_results, bm25_results], 60.0);

        // 5. Truncate to top_k
        fused.truncate(top_k);

        // 6. Serialize results
        let result_json: Vec<serde_json::Value> = fused
            .iter()
            .map(|r| {
                json!({
                    "content": r.content,
                    "score": r.score,
                    "source": r.source,
                    "chunk_id": r.chunk_id.to_string(),
                    "metadata": r.metadata,
                })
            })
            .collect();

        let content = serde_json::to_string_pretty(&json!({
            "query": query,
            "top_k": top_k,
            "results": result_json,
            "count": result_json.len(),
        }))
        .unwrap_or_else(|_| "serialization error".into());

        Ok(ToolOutput::success(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_empty_results_when_unconfigured() {
        let tool = RagSearchTool::new();
        let result = tool
            .execute(json!({"query": "hypertension treatment guidelines"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["results"].as_array().unwrap().is_empty());
        assert!(
            parsed["note"]
                .as_str()
                .unwrap()
                .contains("not yet connected")
        );
    }

    #[tokio::test]
    async fn respects_top_k_parameter() {
        let tool = RagSearchTool::new();
        let result = tool
            .execute(json!({"query": "diabetes management", "top_k": 10}))
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["top_k"].as_u64().unwrap(), 10);
    }

    #[test]
    fn tool_definition_has_correct_name() {
        let tool = RagSearchTool::new();
        assert_eq!(tool.definition().name, "search_knowledge_base");
    }

    #[test]
    fn new_creates_unconfigured_tool() {
        let tool = RagSearchTool::new();
        assert!(!tool.is_configured());
    }

    #[test]
    fn missing_query_returns_error() {
        let tool = RagSearchTool::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(json!({}))).unwrap();
        assert!(result.is_error);
    }
}
