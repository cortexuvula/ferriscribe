use std::sync::Arc;

use uuid::Uuid;

use medical_core::error::{AppError, AppResult};
use medical_core::types::rag::{DocumentChunk, EntityType, GraphEntity, RagChunkMetadata};

use crate::embeddings::EmbeddingGenerator;
use crate::graph_search::GraphSearch;
use crate::vector_store::VectorStore;

/// Ingestion pipeline that chunks documents, generates embeddings,
/// stores them in the vector store, and extracts medical entities
/// into the knowledge graph.
pub struct IngestionPipeline {
    embeddings: Arc<EmbeddingGenerator>,
    vector_store: Arc<VectorStore>,
    graph_search: Arc<GraphSearch>,
}

impl IngestionPipeline {
    /// Create a new pipeline with all required backends.
    pub fn new(
        embeddings: Arc<EmbeddingGenerator>,
        vector_store: Arc<VectorStore>,
        graph_search: Arc<GraphSearch>,
    ) -> Self {
        Self {
            embeddings,
            vector_store,
            graph_search,
        }
    }

    /// Ingest a plain-text document.
    ///
    /// 1. Splits `text` into overlapping chunks (200 words, 50-word overlap).
    /// 2. Generates embeddings for every chunk via the configured backend.
    /// 3. Stores each chunk + embedding in the vector store.
    /// 4. Extracts medical entities and persists them in the knowledge graph.
    ///
    /// Returns the number of chunks created.
    pub async fn ingest_text(&self, doc_id: Uuid, title: &str, text: &str) -> AppResult<u32> {
        let chunks = chunk_text(text, 200, 50);
        if chunks.is_empty() {
            return Ok(0);
        }

        // Generate embeddings for all chunks in one batch
        let chunk_refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        let embeddings = self.embeddings.embed_batch(&chunk_refs).await?;

        // Store each chunk with its embedding
        let total = chunks.len() as u32;
        for (i, (chunk_text, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            let chunk = DocumentChunk {
                id: Uuid::new_v4(),
                document_id: doc_id,
                content: chunk_text.clone(),
                embedding: embedding.clone(),
                chunk_index: i as u32,
                metadata: RagChunkMetadata {
                    document_title: Some(title.to_string()),
                    chunk_index: i as u32,
                    total_chunks: total,
                    page_number: None,
                },
            };
            self.vector_store
                .store_chunk(&chunk)
                .map_err(|e| AppError::Rag(format!("Vector store: {e}")))?;
        }

        // Extract medical entities and store in the knowledge graph
        let entities = extract_medical_entities(text);
        for entity in &entities {
            if let Err(e) = self.graph_search.store_entity(entity) {
                tracing::warn!("Failed to store entity '{}': {e}", entity.name);
            }
        }

        Ok(total)
    }

    /// Delete all indexed data for a document from the vector store.
    ///
    /// Removes every chunk (and its embedding) that was stored under
    /// `doc_id`. Knowledge-graph entities extracted from the document are
    /// **not** removed, because entities are deduplicated across documents
    /// via deterministic UUIDs and may still be referenced by other
    /// documents.
    pub async fn delete_document(&self, doc_id: Uuid) -> AppResult<()> {
        self.vector_store
            .delete_document(&doc_id)
            .map_err(|e| AppError::Rag(format!("Delete: {e}")))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Medical entity extraction
// ---------------------------------------------------------------------------

/// Simple keyword-based medical entity extractor.
///
/// Scans the input text for known drug names and medical conditions.
/// Uses deterministic UUIDs (v5) so the same term always maps to the
/// same entity ID, enabling natural deduplication on upsert.
fn extract_medical_entities(text: &str) -> Vec<GraphEntity> {
    let mut entities = Vec::new();
    let text_lower = text.to_lowercase();

    // Common drug names
    let drugs = [
        "aspirin",
        "ibuprofen",
        "metformin",
        "lisinopril",
        "atorvastatin",
        "amlodipine",
        "metoprolol",
        "omeprazole",
        "losartan",
        "warfarin",
        "levothyroxine",
        "gabapentin",
        "prednisone",
        "amoxicillin",
        "albuterol",
    ];

    for drug in &drugs {
        if text_lower.contains(drug) {
            entities.push(GraphEntity {
                id: Uuid::new_v5(&Uuid::NAMESPACE_OID, drug.as_bytes()),
                entity_type: EntityType::Drug,
                name: drug.to_string(),
                properties: serde_json::json!({}),
            });
        }
    }

    // Common medical conditions
    let conditions = [
        "hypertension",
        "diabetes",
        "asthma",
        "copd",
        "pneumonia",
        "heart failure",
        "atrial fibrillation",
        "stroke",
        "depression",
        "anxiety",
        "arthritis",
        "obesity",
        "anemia",
        "hypothyroidism",
    ];

    for condition in &conditions {
        if text_lower.contains(condition) {
            entities.push(GraphEntity {
                id: Uuid::new_v5(&Uuid::NAMESPACE_OID, condition.as_bytes()),
                entity_type: EntityType::Condition,
                name: condition.to_string(),
                properties: serde_json::json!({}),
            });
        }
    }

    entities
}

/// Split `text` into overlapping chunks of approximately `chunk_size` words.
///
/// Words that fall within the overlap window are repeated at the start of the
/// next chunk. Returns an empty `Vec` when `chunk_size` is 0.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if chunk_size == 0 {
        return Vec::new();
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    if words.len() <= chunk_size {
        return vec![words.join(" ")];
    }

    let step = if chunk_size > overlap {
        chunk_size - overlap
    } else {
        1
    };

    let mut chunks: Vec<String> = Vec::new();
    let mut start = 0;

    while start < words.len() {
        let end = (start + chunk_size).min(words.len());
        chunks.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        start += step;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::rag::EntityType;

    // -----------------------------------------------------------------------
    // chunk_text tests
    // -----------------------------------------------------------------------

    #[test]
    fn basic_chunking() {
        // 6 words, chunk_size=3, overlap=1 -> step=2
        // chunk 0: [0,1,2], chunk 1: [2,3,4], chunk 2: [4,5]
        let text = "one two three four five six";
        let chunks = chunk_text(text, 3, 1);
        assert_eq!(chunks.len(), 3, "expected 3 chunks, got {:?}", chunks);
        assert_eq!(chunks[0], "one two three");
        assert_eq!(chunks[1], "three four five");
        assert_eq!(chunks[2], "five six");
    }

    #[test]
    fn short_text_returns_one_chunk() {
        let text = "hello world";
        let chunks = chunk_text(text, 10, 2);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello world");
    }

    #[test]
    fn empty_returns_empty() {
        let chunks = chunk_text("", 5, 1);
        assert!(chunks.is_empty());
    }

    #[test]
    fn zero_size_returns_empty() {
        let chunks = chunk_text("one two three", 0, 0);
        assert!(chunks.is_empty());
    }

    // -----------------------------------------------------------------------
    // extract_medical_entities tests
    // -----------------------------------------------------------------------

    #[test]
    fn extracts_drugs() {
        let text = "The patient was prescribed aspirin and metformin for their conditions.";
        let entities = extract_medical_entities(text);

        let drug_names: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Drug)
            .map(|e| e.name.as_str())
            .collect();

        assert!(drug_names.contains(&"aspirin"), "should find aspirin");
        assert!(drug_names.contains(&"metformin"), "should find metformin");
    }

    #[test]
    fn extracts_conditions() {
        let text = "Patient presents with hypertension and diabetes mellitus.";
        let entities = extract_medical_entities(text);

        let condition_names: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Condition)
            .map(|e| e.name.as_str())
            .collect();

        assert!(
            condition_names.contains(&"hypertension"),
            "should find hypertension"
        );
        assert!(
            condition_names.contains(&"diabetes"),
            "should find diabetes"
        );
    }

    #[test]
    fn extracts_mixed_drugs_and_conditions() {
        let text = "Lisinopril is commonly used to treat hypertension and heart failure.";
        let entities = extract_medical_entities(text);

        let drugs: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Drug)
            .map(|e| e.name.as_str())
            .collect();
        let conditions: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Condition)
            .map(|e| e.name.as_str())
            .collect();

        assert!(drugs.contains(&"lisinopril"));
        assert!(conditions.contains(&"hypertension"));
        assert!(conditions.contains(&"heart failure"));
    }

    #[test]
    fn case_insensitive_extraction() {
        let text = "ASPIRIN and Ibuprofen for ARTHRITIS management.";
        let entities = extract_medical_entities(text);

        let names: Vec<&str> = entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"aspirin"));
        assert!(names.contains(&"ibuprofen"));
        assert!(names.contains(&"arthritis"));
    }

    #[test]
    fn no_entities_in_unrelated_text() {
        let text = "The quick brown fox jumps over the lazy dog.";
        let entities = extract_medical_entities(text);
        assert!(entities.is_empty());
    }

    #[test]
    fn deterministic_ids_via_uuid_v5() {
        let text = "aspirin aspirin aspirin";
        let entities = extract_medical_entities(text);

        // Should produce exactly one entity (keyword match, not per-occurrence)
        assert_eq!(entities.len(), 1);

        // Running extraction twice should give the same UUID
        let entities2 = extract_medical_entities(text);
        assert_eq!(entities[0].id, entities2[0].id);
    }

    #[test]
    fn empty_text_returns_no_entities() {
        let entities = extract_medical_entities("");
        assert!(entities.is_empty());
    }
}
