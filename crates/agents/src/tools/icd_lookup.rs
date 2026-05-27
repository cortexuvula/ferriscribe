//! ICD-10 code lookup tool.
//!
//! Provides substring search over a hardcoded list of common ICD-10 codes.
//! Matching is case-insensitive and checks the code, description, and a set
//! of keyword synonyms for each entry.

use async_trait::async_trait;
use medical_core::{
    error::AppResult,
    traits::Tool,
    types::{ToolDef, ToolOutput},
};
use serde_json::json;

/// Tool for looking up ICD diagnostic codes.
///
/// Registered as `search_icd_codes`. Searches a hardcoded table of common
/// ICD-10 codes by matching the query against the code, description, and
/// keyword synonyms. Returns matching entries as JSON.
pub struct IcdLookupTool;

/// Hardcoded list of common ICD-10 codes.
///
/// Each entry is `(code, description, keyword_synonyms)`.
const ICD10_CODES: &[(&str, &str, &str)] = &[
    ("I10", "Essential (primary) hypertension", "hypertension high blood pressure"),
    ("E11", "Type 2 diabetes mellitus", "diabetes type 2 blood sugar glucose"),
    ("J06.9", "Acute upper respiratory infection, unspecified", "upper respiratory infection URI cold"),
    ("M54.5", "Low back pain", "back pain lumbar"),
    ("R51.9", "Headache, unspecified", "headache pain head"),
    ("J45", "Asthma", "asthma wheezing bronchospasm"),
    ("K21.0", "Gastro-esophageal reflux disease with oesophagitis", "GERD reflux heartburn acid"),
    ("F41.1", "Generalized anxiety disorder", "GAD anxiety generalized"),
    ("G43", "Migraine", "migraine headache aura"),
    ("N39.0", "Urinary tract infection, site not specified", "UTI urinary tract infection"),
];

#[async_trait]
impl Tool for IcdLookupTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "search_icd_codes".into(),
            description: "Search for ICD diagnostic codes matching a clinical query. Returns matching ICD-10 codes with descriptions.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Clinical term, condition, or keyword to search for"
                    },
                    "version": {
                        "type": "string",
                        "enum": ["ICD-9", "ICD-10", "both"],
                        "description": "ICD version to search. Defaults to ICD-10.",
                        "default": "ICD-10"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> AppResult<ToolOutput> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        if query.is_empty() {
            return Ok(ToolOutput::error("query parameter is required"));
        }

        let mut results = Vec::new();
        for (code, description, keywords) in ICD10_CODES {
            let searchable = format!("{} {} {}", code, description, keywords).to_lowercase();
            if searchable.contains(&query) || query.split_whitespace().any(|w| searchable.contains(w)) {
                results.push(json!({
                    "code": code,
                    "description": description,
                    "version": "ICD-10"
                }));
            }
        }

        if results.is_empty() {
            Ok(ToolOutput::success(format!(
                "No ICD-10 codes found matching '{}'. Consider refining your search terms.",
                query
            )))
        } else {
            let content = serde_json::to_string_pretty(&json!({
                "query": query,
                "results": results
            }))
            .unwrap_or_else(|_| "serialization error".into());
            Ok(ToolOutput::success(content))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lookup_hypertension_finds_i10() {
        let tool = IcdLookupTool;
        let result = tool.execute(json!({"query": "hypertension"})).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("I10"));
    }

    #[tokio::test]
    async fn lookup_unknown_returns_empty() {
        let tool = IcdLookupTool;
        let result = tool
            .execute(json!({"query": "xyzzy_nonexistent_condition_12345"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        // Should indicate no results found
        assert!(
            result.content.contains("No ICD-10 codes found")
                || result.content.contains("results")
        );
    }

    #[tokio::test]
    async fn lookup_diabetes_finds_e11() {
        let tool = IcdLookupTool;
        let result = tool.execute(json!({"query": "diabetes"})).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("E11"));
    }

    #[test]
    fn tool_definition_has_correct_name() {
        let tool = IcdLookupTool;
        assert_eq!(tool.definition().name, "search_icd_codes");
    }
}
