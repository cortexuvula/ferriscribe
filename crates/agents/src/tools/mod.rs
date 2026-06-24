//! Tool implementations and the [`ToolRegistry`].
//!
//! Each tool implements the [`Tool`] trait from
//! `medical-core`, providing a JSON Schema [`ToolDef`] for the AI provider
//! and an `execute()` method that processes the model's arguments.
//!
//! # Built-in tools
//!
//! | Tool | Registered name | Purpose |
//! |---|---|---|
//! | [`IcdLookupTool`] | `search_icd_codes` | Substring search over common ICD-10 codes |
//! | [`DrugInteractionTool`] | `lookup_drug_interactions` | Pairwise drug-interaction check |
//! | [`VitalsExtractorTool`] | `extract_vitals` | Regex extraction of vital signs from text |
//! | [`RagSearchTool`] | `search_knowledge_base` | Hybrid vector + BM25 knowledge-base search |
//! | [`ChecklistTool`] | `generate_checklist` | Numbered clinical checklists |
//!
//! # [`ToolRegistry`]
//!
//! A name-keyed map of `Arc<dyn Tool>` instances. Use
//! [`ToolRegistry::with_defaults()`] to get all five tools pre-loaded, or
//! build your own with [`ToolRegistry::new()`] + [`register()`](ToolRegistry::register).

pub mod checklist;
pub mod drug_interaction;
pub mod icd_lookup;
pub mod rag_search;
pub mod vitals_extractor;

pub use checklist::ChecklistTool;
pub use drug_interaction::DrugInteractionTool;
pub use icd_lookup::IcdLookupTool;
pub use rag_search::RagSearchTool;
pub use vitals_extractor::VitalsExtractorTool;

use std::collections::HashMap;
use std::sync::Arc;

use medical_core::{traits::Tool, types::ToolDef};

/// Registry that holds all available tools by name.
///
/// Tools are stored as `Arc<dyn Tool>` so the registry is cheaply cloneable
/// and tools can be shared across threads. Lookups are by the tool's
/// registered name (the `name` field of its [`ToolDef`]).
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    ///
    /// Call [`register()`](Self::register) to add tools individually, or use
    /// [`with_defaults()`](Self::with_defaults) for a pre-loaded registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool in the registry.
    ///
    /// The tool's [`ToolDef::name`](medical_core::types::ToolDef::name) is
    /// used as the registry key. Registering a tool with the same name as an
    /// existing tool replaces it.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.definition().name.clone();
        self.tools.insert(name, tool);
    }

    /// Retrieve a tool by its name.
    ///
    /// Returns `None` if no tool with the given name is registered. The
    /// orchestrator uses this to silently skip tools that an agent declares
    /// but that are not in the registry.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Return the definitions of all registered tools.
    ///
    /// Used by the orchestrator to build the tool list sent to the AI
    /// provider on each completion request.
    pub fn list_definitions(&self) -> Vec<ToolDef> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    /// Create a registry pre-loaded with all 5 default medical tools.
    ///
    /// The [`RagSearchTool`] is registered with its stub constructor
    /// (`RagSearchTool::new()`), so it returns a "not connected" message
    /// until the caller replaces it with a `with_rag(...)` instance.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(IcdLookupTool));
        registry.register(Arc::new(DrugInteractionTool));
        registry.register(Arc::new(VitalsExtractorTool));
        registry.register(Arc::new(RagSearchTool::new()));
        registry.register(Arc::new(ChecklistTool));
        registry
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_new_is_empty() {
        let registry = ToolRegistry::new();
        assert!(registry.list_definitions().is_empty());
    }

    #[test]
    fn registry_with_defaults_has_five_tools() {
        let registry = ToolRegistry::with_defaults();
        assert_eq!(registry.list_definitions().len(), 5);
    }

    #[test]
    fn registry_get_known_tool() {
        let registry = ToolRegistry::with_defaults();
        assert!(registry.get("search_icd_codes").is_some());
        assert!(registry.get("lookup_drug_interactions").is_some());
        assert!(registry.get("extract_vitals").is_some());
        assert!(registry.get("search_knowledge_base").is_some());
        assert!(registry.get("generate_checklist").is_some());
    }

    #[test]
    fn registry_get_unknown_returns_none() {
        let registry = ToolRegistry::with_defaults();
        assert!(registry.get("nonexistent_tool").is_none());
    }

    #[test]
    fn registry_default_same_as_with_defaults() {
        let registry = ToolRegistry::default();
        assert_eq!(registry.list_definitions().len(), 5);
    }

    #[test]
    fn registry_register_custom_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(IcdLookupTool));
        assert_eq!(registry.list_definitions().len(), 1);
        assert!(registry.get("search_icd_codes").is_some());
    }
}
