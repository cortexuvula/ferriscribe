//! Knowledge-graph storage backed by CozoDB.
//!
//! CozoDB is a pure-Rust embedded graph database that uses Datalog as its query
//! language. We use it to store medical entities (drugs, conditions, procedures,
//! symptoms, lab tests) and their relationships.

use std::collections::BTreeMap;
use std::path::Path;

use cozo::{DataValue, DbInstance, ScriptMutability};
use medical_core::types::rag::{EntityType, GraphEntity, GraphRelation, RelationType};

use crate::{DbError, DbResult};

// ---------------------------------------------------------------------------
// Helpers for converting enum variants to/from strings
// ---------------------------------------------------------------------------

fn entity_type_to_str(et: &EntityType) -> &'static str {
    match et {
        EntityType::Drug => "drug",
        EntityType::Condition => "condition",
        EntityType::Procedure => "procedure",
        EntityType::Symptom => "symptom",
        EntityType::LabTest => "lab_test",
    }
}

fn relation_type_to_str(rt: &RelationType) -> &'static str {
    match rt {
        RelationType::Treats => "treats",
        RelationType::Contraindicates => "contraindicates",
        RelationType::Causes => "causes",
        RelationType::Diagnoses => "diagnoses",
        RelationType::Indicates => "indicates",
    }
}

// ---------------------------------------------------------------------------
// Schema creation scripts
// ---------------------------------------------------------------------------

/// CozoDB Datalog script to create the `entity` stored relation.
/// The key is `id` (a string UUID). Value columns store the entity metadata.
const CREATE_ENTITY_RELATION: &str = r#"
:create entity {
    id: String
    =>
    name: String,
    entity_type: String,
    properties: String
}
"#;

/// CozoDB Datalog script to create the `relation` stored relation.
/// Composite key is (from_id, to_id, relation_type) to allow multiple
/// relationship types between the same pair.
const CREATE_RELATION_RELATION: &str = r#"
:create relation {
    from_id: String,
    to_id: String,
    relation_type: String
    =>
    properties: String
}
"#;

// ---------------------------------------------------------------------------
// GraphRepo
// ---------------------------------------------------------------------------

/// A CozoDB-backed knowledge graph repository.
///
/// Stores medical entities and their relationships using Datalog-queryable
/// stored relations. Thread-safe (`DbInstance` is `Clone + Send + Sync`).
pub struct GraphRepo {
    db: DbInstance,
}

impl GraphRepo {
    /// Open (or create) a CozoDB database backed by Sled at the given path.
    ///
    /// Uses the Sled embedded storage engine (pure Rust, no native sqlite3 link
    /// conflict with rusqlite). Runs schema creation idempotently — if the
    /// relations already exist the errors are silently ignored.
    pub fn open(path: &Path) -> DbResult<Self> {
        let db = DbInstance::new("sled", path, "")
            .map_err(|e| DbError::Graph(format!("failed to open CozoDB: {e}")))?;
        let repo = Self { db };
        repo.ensure_schema()?;
        Ok(repo)
    }

    /// Open an in-memory CozoDB instance. Primarily useful for tests.
    pub fn open_in_memory() -> DbResult<Self> {
        let db = DbInstance::new("mem", "", "")
            .map_err(|e| DbError::Graph(format!("failed to open in-memory CozoDB: {e}")))?;
        let repo = Self { db };
        repo.ensure_schema()?;
        Ok(repo)
    }

    /// Run the schema creation scripts. `:create` in CozoDB errors if the
    /// relation already exists, so we swallow those errors.
    fn ensure_schema(&self) -> DbResult<()> {
        // Ignore errors — the relation may already exist.
        let _ = self
            .db
            .run_script(CREATE_ENTITY_RELATION, BTreeMap::new(), ScriptMutability::Mutable);
        let _ = self
            .db
            .run_script(CREATE_RELATION_RELATION, BTreeMap::new(), ScriptMutability::Mutable);
        Ok(())
    }

    /// Upsert an entity into the knowledge graph.
    pub fn insert_entity(&self, entity: &GraphEntity) -> DbResult<()> {
        let mut params = BTreeMap::new();
        params.insert(
            "id".to_string(),
            DataValue::Str(entity.id.to_string().into()),
        );
        params.insert(
            "name".to_string(),
            DataValue::Str(entity.name.clone().into()),
        );
        params.insert(
            "entity_type".to_string(),
            DataValue::Str(entity_type_to_str(&entity.entity_type).into()),
        );
        params.insert(
            "properties".to_string(),
            DataValue::Str(entity.properties.to_string().into()),
        );

        self.db
            .run_script(
                "?[id, name, entity_type, properties] <- [[$id, $name, $entity_type, $properties]]
                 :put entity {id => name, entity_type, properties}",
                params,
                ScriptMutability::Mutable,
            )
            .map_err(|e| DbError::Graph(format!("insert_entity failed: {e}")))?;

        Ok(())
    }

    /// Upsert a relation between two entities.
    pub fn insert_relation(&self, rel: &GraphRelation) -> DbResult<()> {
        let mut params = BTreeMap::new();
        params.insert(
            "from_id".to_string(),
            DataValue::Str(rel.from.to_string().into()),
        );
        params.insert(
            "to_id".to_string(),
            DataValue::Str(rel.to.to_string().into()),
        );
        params.insert(
            "relation_type".to_string(),
            DataValue::Str(relation_type_to_str(&rel.relation_type).into()),
        );
        params.insert(
            "properties".to_string(),
            DataValue::Str(rel.properties.to_string().into()),
        );

        self.db
            .run_script(
                "?[from_id, to_id, relation_type, properties] <- [[$from_id, $to_id, $relation_type, $properties]]
                 :put relation {from_id, to_id, relation_type => properties}",
                params,
                ScriptMutability::Mutable,
            )
            .map_err(|e| DbError::Graph(format!("insert_relation failed: {e}")))?;

        Ok(())
    }

    /// Find entities related to the given entity name.
    ///
    /// Returns up to `top_k` tuples of `(name, entity_type, relation_type)`.
    /// Searches in both directions (outgoing and incoming relations).
    pub fn query_related(
        &self,
        entity_name: &str,
        top_k: usize,
    ) -> DbResult<Vec<(String, String, String)>> {
        let mut params = BTreeMap::new();
        params.insert(
            "name".to_string(),
            DataValue::Str(entity_name.into()),
        );
        params.insert(
            "top_k".to_string(),
            DataValue::from(top_k as i64),
        );

        // Query: find the entity by name, then traverse outgoing and incoming
        // relations to find related entities.
        let script = r#"
            source[id] := *entity{id, name: $name}
            outgoing[related_name, related_type, rel_type] :=
                source[src_id],
                *relation{from_id: src_id, to_id, relation_type: rel_type},
                *entity{id: to_id, name: related_name, entity_type: related_type}
            incoming[related_name, related_type, rel_type] :=
                source[src_id],
                *relation{from_id, to_id: src_id, relation_type: rel_type},
                *entity{id: from_id, name: related_name, entity_type: related_type}
            ?[name, entity_type, relation_type] :=
                outgoing[name, entity_type, relation_type]
            ?[name, entity_type, relation_type] :=
                incoming[name, entity_type, relation_type]
            :limit $top_k
        "#;

        let result = self
            .db
            .run_script(script, params, ScriptMutability::Immutable)
            .map_err(|e| DbError::Graph(format!("query_related failed: {e}")))?;

        let mut out = Vec::new();
        for row in &result.rows {
            if let [DataValue::Str(name), DataValue::Str(etype), DataValue::Str(rtype), ..] =
                row.as_slice()
            {
                out.push((
                    name.to_string(),
                    etype.to_string(),
                    rtype.to_string(),
                ));
            }
        }

        Ok(out)
    }

    /// Search entities whose name contains the given substring (case-insensitive).
    ///
    /// Returns up to `top_k` tuples of `(id, name, entity_type)`.
    pub fn search_by_name(
        &self,
        query: &str,
        top_k: usize,
    ) -> DbResult<Vec<(String, String, String)>> {
        let mut params = BTreeMap::new();
        params.insert(
            "query".to_string(),
            DataValue::Str(query.to_lowercase().into()),
        );
        params.insert(
            "top_k".to_string(),
            DataValue::from(top_k as i64),
        );

        // Use CozoDB's `lowercase` and `str_includes` built-in functions
        // for case-insensitive substring matching.
        let script = r#"
            ?[id, name, entity_type] :=
                *entity{id, name, entity_type},
                lower_name = lowercase(name),
                str_includes(lower_name, $query)
            :limit $top_k
        "#;

        let result = self
            .db
            .run_script(script, params, ScriptMutability::Immutable)
            .map_err(|e| DbError::Graph(format!("search_by_name failed: {e}")))?;

        let mut out = Vec::new();
        for row in &result.rows {
            if let [DataValue::Str(id), DataValue::Str(name), DataValue::Str(etype), ..] =
                row.as_slice()
            {
                out.push((
                    id.to_string(),
                    name.to_string(),
                    etype.to_string(),
                ));
            }
        }

        Ok(out)
    }
}

impl Default for GraphRepo {
    /// Creates an in-memory `GraphRepo`. Panics if CozoDB initialization fails.
    fn default() -> Self {
        Self::open_in_memory().expect("failed to create default in-memory GraphRepo")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn make_entity(name: &str, etype: EntityType) -> GraphEntity {
        GraphEntity {
            id: Uuid::new_v4(),
            entity_type: etype,
            name: name.to_string(),
            properties: json!({}),
        }
    }

    #[test]
    fn insert_and_search_by_name() {
        let repo = GraphRepo::open_in_memory().expect("open in-memory");

        let aspirin = make_entity("Aspirin", EntityType::Drug);
        repo.insert_entity(&aspirin).expect("insert aspirin");

        let results = repo.search_by_name("aspirin", 10).expect("search");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "Aspirin");
        assert_eq!(results[0].2, "drug");
    }

    #[test]
    fn insert_entities_and_relation_then_query_related() {
        let repo = GraphRepo::open_in_memory().expect("open in-memory");

        let aspirin = make_entity("Aspirin", EntityType::Drug);
        let headache = make_entity("Headache", EntityType::Condition);

        repo.insert_entity(&aspirin).expect("insert aspirin");
        repo.insert_entity(&headache).expect("insert headache");

        let rel = GraphRelation {
            from: aspirin.id,
            to: headache.id,
            relation_type: RelationType::Treats,
            properties: json!({"efficacy": "high"}),
        };
        repo.insert_relation(&rel).expect("insert relation");

        // Query from the Aspirin side
        let related = repo.query_related("Aspirin", 10).expect("query related");
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].0, "Headache");
        assert_eq!(related[0].1, "condition");
        assert_eq!(related[0].2, "treats");

        // Query from the Headache side (incoming relation)
        let related = repo.query_related("Headache", 10).expect("query related");
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].0, "Aspirin");
        assert_eq!(related[0].1, "drug");
        assert_eq!(related[0].2, "treats");
    }

    #[test]
    fn upsert_entity_does_not_error() {
        let repo = GraphRepo::open_in_memory().expect("open in-memory");

        let mut aspirin = make_entity("Aspirin", EntityType::Drug);
        repo.insert_entity(&aspirin).expect("insert aspirin");

        // Update properties and re-insert (upsert)
        aspirin.properties = json!({"brand": "Bayer"});
        repo.insert_entity(&aspirin).expect("upsert aspirin");

        // Should still find exactly one entity
        let results = repo.search_by_name("aspirin", 10).expect("search");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_returns_partial_matches() {
        let repo = GraphRepo::open_in_memory().expect("open in-memory");

        repo.insert_entity(&make_entity("Metformin", EntityType::Drug))
            .expect("insert");
        repo.insert_entity(&make_entity("Metoprolol", EntityType::Drug))
            .expect("insert");
        repo.insert_entity(&make_entity("Ibuprofen", EntityType::Drug))
            .expect("insert");

        let results = repo.search_by_name("met", 10).expect("search");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_respects_top_k() {
        let repo = GraphRepo::open_in_memory().expect("open in-memory");

        for i in 0..5 {
            repo.insert_entity(&make_entity(&format!("Drug{i}"), EntityType::Drug))
                .expect("insert");
        }

        let results = repo.search_by_name("drug", 3).expect("search");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn default_creates_in_memory() {
        let repo = GraphRepo::default();
        let entity = make_entity("Test", EntityType::Symptom);
        repo.insert_entity(&entity).expect("insert");
        let results = repo.search_by_name("test", 5).expect("search");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn file_backed_persistence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("graph.db");

        // Create and populate
        {
            let repo = GraphRepo::open(&db_path).expect("open");
            repo.insert_entity(&make_entity("Amoxicillin", EntityType::Drug))
                .expect("insert");
        }

        // Re-open and verify data persists
        {
            let repo = GraphRepo::open(&db_path).expect("reopen");
            let results = repo.search_by_name("amoxicillin", 5).expect("search");
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].1, "Amoxicillin");
        }
    }
}
