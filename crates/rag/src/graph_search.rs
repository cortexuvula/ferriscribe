use std::sync::Arc;

use uuid::Uuid;

use medical_core::types::rag::{
    GraphEntity, GraphRelation, RagChunkMetadata, RagResult, SearchSource,
};
use medical_db::Database;

use crate::RagError;

/// SQLite-backed knowledge-graph search for the RAG layer.
///
/// Stores entities and relations in two lightweight SQLite tables
/// (`graph_entities` and `graph_relations`), providing graph-based
/// retrieval within the RAG pipeline without a separate graph database.
pub struct GraphSearch {
    db: Arc<Database>,
    initialized: std::sync::OnceLock<Result<(), String>>,
}

impl GraphSearch {
    /// Create a new `GraphSearch` backed by the given database.
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            initialized: std::sync::OnceLock::new(),
        }
    }

    /// Ensure the graph tables exist (created lazily on first use).
    ///
    /// Uses `OnceLock` so that: if initialization fails, the error is cached
    /// and consistently reported on every subsequent call (instead of silently
    /// returning `Ok` like `std::sync::Once` would).
    fn ensure_tables(&self) -> Result<(), RagError> {
        let result = self
            .initialized
            .get_or_init(|| self.create_tables().map_err(|e| e.to_string()));
        match result {
            Ok(()) => Ok(()),
            Err(msg) => Err(RagError::Database(msg.clone())),
        }
    }

    fn create_tables(&self) -> Result<(), RagError> {
        let conn = self
            .db
            .conn()
            .map_err(|e| RagError::Database(e.to_string()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS graph_entities (
                id          TEXT PRIMARY KEY NOT NULL,
                name        TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                properties  TEXT DEFAULT '{}'
            );

            CREATE INDEX IF NOT EXISTS idx_graph_entities_name
                ON graph_entities(name);

            CREATE TABLE IF NOT EXISTS graph_relations (
                from_id       TEXT NOT NULL,
                to_id         TEXT NOT NULL,
                relation_type TEXT NOT NULL,
                weight        REAL DEFAULT 1.0,
                properties    TEXT DEFAULT '{}',
                PRIMARY KEY (from_id, to_id, relation_type)
            );

            CREATE INDEX IF NOT EXISTS idx_graph_relations_from
                ON graph_relations(from_id);
            CREATE INDEX IF NOT EXISTS idx_graph_relations_to
                ON graph_relations(to_id);",
        )
        .map_err(|e| RagError::Database(format!("failed to create graph tables: {e}")))?;

        Ok(())
    }

    /// Search the graph for entities matching `query` (case-insensitive
    /// substring match) and include related entities as context.
    ///
    /// Returns up to `top_k` results with `source: SearchSource::Graph`.
    pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<RagResult>, RagError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        self.ensure_tables()?;

        let conn = self
            .db
            .conn()
            .map_err(|e| RagError::Database(e.to_string()))?;
        let pattern = format!("%{}%", query.to_lowercase());

        // Step 1: Find entities matching the query by name
        let mut stmt = conn
            .prepare(
                "SELECT id, name, entity_type, properties
                 FROM graph_entities
                 WHERE LOWER(name) LIKE ?1
                 LIMIT ?2",
            )
            .map_err(|e| RagError::Database(e.to_string()))?;

        let matching_entities: Vec<(String, String, String, String)> = stmt
            .query_map(rusqlite::params![pattern, top_k as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| RagError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| RagError::Database(e.to_string()))?;

        let mut results = Vec::new();

        for (entity_id, name, entity_type, properties) in &matching_entities {
            // Build a content string from the entity
            let content = format!("{} ({}): {}", name, entity_type, properties);

            let chunk_id = Uuid::parse_str(entity_id).unwrap_or(Uuid::nil());

            results.push(RagResult {
                chunk_id,
                document_id: Uuid::nil(),
                content,
                score: 1.0, // Direct match
                source: SearchSource::Graph,
                metadata: RagChunkMetadata {
                    document_title: None,
                    chunk_index: 0,
                    total_chunks: 0,
                    page_number: None,
                },
            });

            // Step 2: Find related entities via relations (outgoing)
            let mut rel_stmt = conn
                .prepare(
                    "SELECT e.id, e.name, e.entity_type, e.properties, r.relation_type
                     FROM graph_relations r
                     JOIN graph_entities e ON e.id = r.to_id
                     WHERE r.from_id = ?1
                     LIMIT ?2",
                )
                .map_err(|e| RagError::Database(e.to_string()))?;

            let outgoing: Vec<(String, String, String, String, String)> = rel_stmt
                .query_map(rusqlite::params![entity_id, top_k as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })
                .map_err(|e| RagError::Database(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| RagError::Database(e.to_string()))?;

            for (rel_id, rel_name, rel_type, _rel_props, relation_type) in &outgoing {
                let content = format!(
                    "{} --[{}]--> {} ({})",
                    name, relation_type, rel_name, rel_type
                );
                let rel_chunk_id = Uuid::parse_str(rel_id).unwrap_or(Uuid::nil());

                results.push(RagResult {
                    chunk_id: rel_chunk_id,
                    document_id: Uuid::nil(),
                    content,
                    score: 0.8, // Related entity, slightly lower score
                    source: SearchSource::Graph,
                    metadata: RagChunkMetadata {
                        document_title: None,
                        chunk_index: 0,
                        total_chunks: 0,
                        page_number: None,
                    },
                });
            }

            // Step 3: Find related entities via relations (incoming)
            let mut in_stmt = conn
                .prepare(
                    "SELECT e.id, e.name, e.entity_type, e.properties, r.relation_type
                     FROM graph_relations r
                     JOIN graph_entities e ON e.id = r.from_id
                     WHERE r.to_id = ?1
                     LIMIT ?2",
                )
                .map_err(|e| RagError::Database(e.to_string()))?;

            let incoming: Vec<(String, String, String, String, String)> = in_stmt
                .query_map(rusqlite::params![entity_id, top_k as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })
                .map_err(|e| RagError::Database(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| RagError::Database(e.to_string()))?;

            for (rel_id, rel_name, rel_type, _rel_props, relation_type) in &incoming {
                let content = format!(
                    "{} ({}) --[{}]--> {}",
                    rel_name, rel_type, relation_type, name
                );
                let rel_chunk_id = Uuid::parse_str(rel_id).unwrap_or(Uuid::nil());

                results.push(RagResult {
                    chunk_id: rel_chunk_id,
                    document_id: Uuid::nil(),
                    content,
                    score: 0.8,
                    source: SearchSource::Graph,
                    metadata: RagChunkMetadata {
                        document_title: None,
                        chunk_index: 0,
                        total_chunks: 0,
                        page_number: None,
                    },
                });
            }
        }

        // Sort by score descending and truncate to top_k
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(top_k);

        Ok(results)
    }

    /// Persist a graph entity node.
    pub fn store_entity(&self, entity: &GraphEntity) -> Result<(), RagError> {
        self.ensure_tables()?;

        let conn = self
            .db
            .conn()
            .map_err(|e| RagError::Database(e.to_string()))?;

        let entity_type = format!("{:?}", entity.entity_type).to_lowercase();

        conn.execute(
            "INSERT OR REPLACE INTO graph_entities (id, name, entity_type, properties)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                entity.id.to_string(),
                entity.name,
                entity_type,
                entity.properties.to_string(),
            ],
        )
        .map_err(|e| RagError::Database(e.to_string()))?;

        Ok(())
    }

    /// Persist a directed relation between two graph entities.
    pub fn store_relation(&self, relation: &GraphRelation) -> Result<(), RagError> {
        self.ensure_tables()?;

        let conn = self
            .db
            .conn()
            .map_err(|e| RagError::Database(e.to_string()))?;

        let relation_type = format!("{:?}", relation.relation_type).to_lowercase();

        conn.execute(
            "INSERT OR REPLACE INTO graph_relations (from_id, to_id, relation_type, weight, properties)
             VALUES (?1, ?2, ?3, 1.0, ?4)",
            rusqlite::params![
                relation.from.to_string(),
                relation.to.to_string(),
                relation_type,
                relation.properties.to_string(),
            ],
        )
        .map_err(|e| RagError::Database(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::rag::{EntityType, RelationType};
    use serde_json::json;

    fn make_db() -> Arc<Database> {
        Arc::new(Database::open_in_memory().expect("open in-memory DB"))
    }

    fn make_entity(id: u128, name: &str, entity_type: EntityType) -> GraphEntity {
        GraphEntity {
            id: Uuid::from_u128(id),
            entity_type,
            name: name.to_string(),
            properties: json!({}),
        }
    }

    fn make_relation(from: u128, to: u128, rel_type: RelationType) -> GraphRelation {
        GraphRelation {
            from: Uuid::from_u128(from),
            to: Uuid::from_u128(to),
            relation_type: rel_type,
            properties: json!({}),
        }
    }

    #[test]
    fn store_and_search_entity() {
        let db = make_db();
        let graph = GraphSearch::new(db);

        let aspirin = make_entity(1, "Aspirin", EntityType::Drug);
        graph.store_entity(&aspirin).expect("store entity");

        let results = graph.search("Aspirin", 10).expect("search");
        assert!(!results.is_empty());
        assert!(results[0].content.contains("Aspirin"));
        assert_eq!(results[0].source, SearchSource::Graph);
    }

    #[test]
    fn search_case_insensitive() {
        let db = make_db();
        let graph = GraphSearch::new(db);

        let entity = make_entity(1, "Metformin", EntityType::Drug);
        graph.store_entity(&entity).expect("store entity");

        let results = graph.search("metformin", 10).expect("search");
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Metformin"));
    }

    #[test]
    fn search_partial_match() {
        let db = make_db();
        let graph = GraphSearch::new(db);

        graph
            .store_entity(&make_entity(1, "Metformin", EntityType::Drug))
            .unwrap();
        graph
            .store_entity(&make_entity(2, "Metoprolol", EntityType::Drug))
            .unwrap();
        graph
            .store_entity(&make_entity(3, "Ibuprofen", EntityType::Drug))
            .unwrap();

        let results = graph.search("Met", 10).expect("search");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_includes_related_entities() {
        let db = make_db();
        let graph = GraphSearch::new(db);

        let aspirin = make_entity(1, "Aspirin", EntityType::Drug);
        let headache = make_entity(2, "Headache", EntityType::Condition);

        graph.store_entity(&aspirin).expect("store aspirin");
        graph.store_entity(&headache).expect("store headache");

        let rel = make_relation(1, 2, RelationType::Treats);
        graph.store_relation(&rel).expect("store relation");

        let results = graph.search("Aspirin", 10).expect("search");

        // Should have the entity itself + the related entity (outgoing)
        assert!(
            results.len() >= 2,
            "expected >= 2 results, got {}",
            results.len()
        );

        // One result should mention Headache (the related entity)
        assert!(
            results.iter().any(|r| r.content.contains("Headache")),
            "should contain related entity Headache in results: {:?}",
            results.iter().map(|r| &r.content).collect::<Vec<_>>()
        );
    }

    #[test]
    fn search_includes_incoming_relations() {
        let db = make_db();
        let graph = GraphSearch::new(db);

        let aspirin = make_entity(1, "Aspirin", EntityType::Drug);
        let headache = make_entity(2, "Headache", EntityType::Condition);

        graph.store_entity(&aspirin).expect("store aspirin");
        graph.store_entity(&headache).expect("store headache");

        let rel = make_relation(1, 2, RelationType::Treats);
        graph.store_relation(&rel).expect("store relation");

        // Searching for Headache should find Aspirin via incoming relation
        let results = graph.search("Headache", 10).expect("search");

        assert!(
            results.iter().any(|r| r.content.contains("Aspirin")),
            "should contain incoming related entity Aspirin in results: {:?}",
            results.iter().map(|r| &r.content).collect::<Vec<_>>()
        );
    }

    #[test]
    fn search_respects_top_k() {
        let db = make_db();
        let graph = GraphSearch::new(db);

        for i in 0..10 {
            graph
                .store_entity(&make_entity(i, &format!("Drug{i}"), EntityType::Drug))
                .unwrap();
        }

        let results = graph.search("Drug", 3).expect("search");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let db = make_db();
        let graph = GraphSearch::new(db);

        graph
            .store_entity(&make_entity(1, "Test", EntityType::Drug))
            .unwrap();

        let results = graph.search("", 10).expect("search");
        assert!(results.is_empty());
    }

    #[test]
    fn search_no_match_returns_empty() {
        let db = make_db();
        let graph = GraphSearch::new(db);

        graph
            .store_entity(&make_entity(1, "Aspirin", EntityType::Drug))
            .unwrap();

        let results = graph.search("xyznonexistent", 10).expect("search");
        assert!(results.is_empty());
    }

    #[test]
    fn store_entity_upsert() {
        let db = make_db();
        let graph = GraphSearch::new(db);

        let mut entity = make_entity(1, "Aspirin", EntityType::Drug);
        graph.store_entity(&entity).expect("store");

        // Upsert with updated properties
        entity.properties = json!({"brand": "Bayer"});
        graph.store_entity(&entity).expect("upsert");

        // Should still find exactly one entity
        let results = graph.search("Aspirin", 10).expect("search");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn store_relation_upsert() {
        let db = make_db();
        let graph = GraphSearch::new(db);

        graph
            .store_entity(&make_entity(1, "Aspirin", EntityType::Drug))
            .unwrap();
        graph
            .store_entity(&make_entity(2, "Headache", EntityType::Condition))
            .unwrap();

        let rel = make_relation(1, 2, RelationType::Treats);
        graph.store_relation(&rel).expect("store");

        // Upsert same relation
        graph.store_relation(&rel).expect("upsert");

        let results = graph.search("Aspirin", 10).expect("search");
        // Should have entity + one related, not duplicated
        let headache_count = results
            .iter()
            .filter(|r| r.content.contains("Headache"))
            .count();
        assert_eq!(headache_count, 1, "relation should not be duplicated");
    }
}
