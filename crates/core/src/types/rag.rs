//! RAG (Retrieval-Augmented Generation) types — search results, document
//! chunks, and medical knowledge graph entities.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A retrieved chunk from the RAG system with its relevance score.
///
/// Produced by the `rag` crate's retrieval pipeline and passed to AI
/// providers as context in [`AgentContext::rag_context`](super::agent::AgentContext::rag_context).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagResult {
    /// UUID of the chunk in the vector store.
    pub chunk_id: Uuid,
    /// UUID of the source document.
    pub document_id: Uuid,
    /// The chunk text content.
    pub content: String,
    /// Relevance score (higher = more relevant).
    pub score: f32,
    /// Which retrieval strategy produced this result.
    pub source: SearchSource,
    /// Metadata about the chunk's position in its document.
    pub metadata: RagChunkMetadata,
}

/// The retrieval strategy that produced a result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSource {
    /// Dense vector similarity search.
    Vector,
    /// BM25 keyword search.
    Bm25,
    /// Knowledge-graph traversal.
    Graph,
    /// Hybrid fusion of multiple retrieval methods.
    Fused,
}

/// Metadata attached to a retrieved chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagChunkMetadata {
    /// Title of the source document.
    pub document_title: Option<String>,
    /// Zero-based index of this chunk within the document.
    pub chunk_index: u32,
    /// Total number of chunks the document was split into.
    pub total_chunks: u32,
    /// Page number in the source document (if applicable).
    pub page_number: Option<u32>,
}

/// Configuration for a RAG search query.
///
/// Controls which retrieval methods are enabled and their parameters.
/// The `mmr_lambda` parameter trades off relevance (1.0) vs. diversity
/// (0.0) in Maximal Marginal Relevance reranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Maximum number of results to return.
    pub top_k: u32,
    /// Minimum similarity score to include a result.
    pub similarity_threshold: f32,
    /// MMR diversity parameter: 1.0 = pure relevance, 0.0 = max diversity.
    pub mmr_lambda: f32,
    /// Whether vector similarity search is enabled.
    pub enable_vector: bool,
    /// Whether BM25 keyword search is enabled.
    pub enable_bm25: bool,
    /// Whether knowledge-graph search is enabled.
    pub enable_graph: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            top_k: 5,
            similarity_threshold: 0.75,
            mmr_lambda: 0.7,
            enable_vector: true,
            enable_bm25: true,
            enable_graph: true,
        }
    }
}

/// A query after expansion with synonyms or related terms.
///
/// Produced by the query-expansion step in the RAG pipeline before
/// retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpandedQuery {
    /// The original user query.
    pub original: String,
    /// Additional terms added during expansion.
    pub expanded_terms: Vec<String>,
    /// The full expanded query text sent to retrieval.
    pub full_query: String,
}

/// A chunk of a document prepared for indexing.
///
/// Contains the chunk text, its embedding vector, and positional
/// metadata. Stored in the vector database by the `rag` crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk {
    /// Unique chunk identifier.
    pub id: Uuid,
    /// The parent document's identifier.
    pub document_id: Uuid,
    /// The chunk text content.
    pub content: String,
    /// Dense embedding vector for similarity search.
    pub embedding: Vec<f32>,
    /// Zero-based position within the document.
    pub chunk_index: u32,
    /// Positional metadata.
    pub metadata: RagChunkMetadata,
}

/// A node in the medical knowledge graph.
///
/// Represents a medical entity (drug, condition, procedure, etc.) with
/// typed properties stored as freeform JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEntity {
    /// Unique entity identifier.
    pub id: Uuid,
    /// The semantic type of this entity.
    pub entity_type: EntityType,
    /// Human-readable entity name.
    pub name: String,
    /// Type-specific properties as freeform JSON.
    pub properties: serde_json::Value,
}

/// The type of a medical entity in the knowledge graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    /// A pharmaceutical drug or medication.
    Drug,
    /// A medical condition or diagnosis.
    Condition,
    /// A medical procedure or intervention.
    Procedure,
    /// A symptom or clinical finding.
    Symptom,
    /// A laboratory test.
    LabTest,
}

/// A directed relationship between two entities in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRelation {
    /// Source entity UUID.
    pub from: Uuid,
    /// Target entity UUID.
    pub to: Uuid,
    /// The semantic type of the relationship.
    pub relation_type: RelationType,
    /// Relationship-specific properties as freeform JSON.
    pub properties: serde_json::Value,
}

/// The semantic type of a graph relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    /// Drug treats condition.
    Treats,
    /// Drug contraindicated for condition.
    Contraindicates,
    /// Entity causes another entity.
    Causes,
    /// Test diagnoses condition.
    Diagnoses,
    /// Symptom indicates condition.
    Indicates,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_config_defaults() {
        let config = SearchConfig::default();
        assert_eq!(config.top_k, 5);
        assert!((config.similarity_threshold - 0.75).abs() < f32::EPSILON);
        assert!((config.mmr_lambda - 0.7).abs() < f32::EPSILON);
        assert!(config.enable_vector);
        assert!(config.enable_bm25);
        assert!(config.enable_graph);
    }

    #[test]
    fn search_source_serializes() {
        let source = SearchSource::Fused;
        let json = serde_json::to_value(&source).unwrap();
        assert_eq!(json, "fused");

        let vector: SearchSource = serde_json::from_str("\"vector\"").unwrap();
        assert_eq!(vector, SearchSource::Vector);
    }

    #[test]
    fn entity_type_serializes() {
        let et = EntityType::Drug;
        let json = serde_json::to_value(&et).unwrap();
        assert_eq!(json, "drug");

        let condition: EntityType = serde_json::from_str("\"condition\"").unwrap();
        assert_eq!(condition, EntityType::Condition);
    }

    #[test]
    fn relation_type_serializes() {
        let rt = RelationType::Treats;
        let json = serde_json::to_value(&rt).unwrap();
        assert_eq!(json, "treats");

        let contra: RelationType = serde_json::from_str("\"contraindicates\"").unwrap();
        assert_eq!(contra, RelationType::Contraindicates);
    }

    #[test]
    fn rag_result_round_trip() {
        let result = RagResult {
            chunk_id: Uuid::new_v4(),
            document_id: Uuid::new_v4(),
            content: "Metformin treats type 2 diabetes.".into(),
            score: 0.92,
            source: SearchSource::Vector,
            metadata: RagChunkMetadata {
                document_title: Some("Drug Guide".into()),
                chunk_index: 0,
                total_chunks: 10,
                page_number: Some(1),
            },
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: RagResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content, result.content);
        assert_eq!(back.source, SearchSource::Vector);
    }
}
