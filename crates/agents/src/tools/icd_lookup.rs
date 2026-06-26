//! ICD diagnostic code lookup tool.
//!
//! Searches the bundled BC MSP ICD-9 list (7,122 codes) and a small
//! hardcoded set of common ICD-10 codes. Honors the `version` parameter
//! to scope results. Matching is case-insensitive and checks the code,
//! description, and (for ICD-10) keyword synonyms.

use async_trait::async_trait;
use medical_core::{
    error::AppResult,
    icd9,
    traits::Tool,
    types::{ToolDef, ToolOutput},
};
use serde_json::json;

/// Tool for looking up ICD diagnostic codes.
///
/// Registered as `search_icd_codes`. Searches the bundled BC MSP ICD-9
/// list and/or a hardcoded ICD-10 table by matching the query against
/// the code, description, and keyword synonyms. Returns matching entries
/// as JSON. The `version` parameter scopes which set is searched.
pub struct IcdLookupTool;

/// Hardcoded list of common ICD-10 codes for the ICD-10 / both modes.
///
/// Each entry is `(code, description, keyword_synonyms)`. BC MSP bills
/// ICD-9, so ICD-10 support here is a best-effort convenience for the
/// chat path only — the SOAP path uses the full MSP ICD-9 list.
const ICD10_CODES: &[(&str, &str, &str)] = &[
    (
        "I10",
        "Essential (primary) hypertension",
        "hypertension high blood pressure",
    ),
    (
        "E11",
        "Type 2 diabetes mellitus",
        "diabetes type 2 blood sugar glucose",
    ),
    (
        "J06.9",
        "Acute upper respiratory infection, unspecified",
        "upper respiratory infection URI cold",
    ),
    ("M54.5", "Low back pain", "back pain lumbar"),
    ("R51.9", "Headache, unspecified", "headache pain head"),
    ("J45", "Asthma", "asthma wheezing bronchospasm"),
    (
        "K21.0",
        "Gastro-esophageal reflux disease with oesophagitis",
        "GERD reflux heartburn acid",
    ),
    (
        "F41.1",
        "Generalized anxiety disorder",
        "GAD anxiety generalized",
    ),
    ("G43", "Migraine", "migraine headache aura"),
    (
        "N39.0",
        "Urinary tract infection, site not specified",
        "UTI urinary tract infection",
    ),
];

/// Maximum ICD-9 results to return. The full list is large; a chat query
/// rarely needs more than this many hits.
const MAX_ICD9_RESULTS: usize = 25;

#[async_trait]
impl Tool for IcdLookupTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "search_icd_codes".into(),
            description: "Search for ICD diagnostic codes matching a clinical query. Searches the BC MSP ICD-9 list (7,122 codes) and/or a common ICD-10 set. Returns matching codes with descriptions.".into(),
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
                        "description": "ICD version to search. Defaults to ICD-9 (BC MSP billing standard).",
                        "default": "ICD-9"
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

        let version = arguments
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("ICD-9");

        let mut results = Vec::new();

        let search_icd9 = matches!(version, "ICD-9" | "both");
        let search_icd10 = matches!(version, "ICD-10" | "both");

        if search_icd9 {
            // Score each match so generic queries like "pain" surface the
            // most relevant codes (exact code hit > full-query substring >
            // token overlap) BEFORE capping — otherwise file-order
            // iteration fills the 25-slot cap with low-relevance
            // early-chapter codes before reaching the 780-799 Symptoms
            // chapter.
            let query_tokens: Vec<&str> = query.split_whitespace().collect();
            let mut scored: Vec<(usize, serde_json::Value)> = Vec::new();
            for entry in icd9::entries() {
                let searchable = format!("{} {}", entry.code, entry.description).to_lowercase();
                let mut score: usize = 0;
                // Exact code match is the strongest signal: the query IS
                // the code (e.g. "401.9" or "v70.0").
                if entry.code.to_lowercase() == query {
                    score += 10;
                }
                // Full query as a substring (e.g. "chest pain" in the
                // description) ranks above individual token hits.
                if searchable.contains(&query) {
                    score += 5;
                }
                // Token overlap.
                score += query_tokens
                    .iter()
                    .filter(|w| searchable.contains(*w))
                    .count();
                if score > 0 {
                    scored.push((
                        score,
                        json!({
                            "code": entry.code,
                            "description": entry.description,
                            "category": entry.category,
                            "version": "ICD-9"
                        }),
                    ));
                }
            }
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            for (_, value) in scored.into_iter().take(MAX_ICD9_RESULTS) {
                results.push(value);
            }
        }

        if search_icd10 {
            for (code, description, keywords) in ICD10_CODES {
                let searchable = format!("{} {} {}", code, description, keywords).to_lowercase();
                if searchable.contains(&query)
                    || query.split_whitespace().any(|w| searchable.contains(w))
                {
                    results.push(json!({
                        "code": code,
                        "description": description,
                        "version": "ICD-10"
                    }));
                }
            }
        }

        if results.is_empty() {
            Ok(ToolOutput::success(format!(
                "No {} codes found matching '{}'. Consider refining your search terms.",
                version, query
            )))
        } else {
            let content = serde_json::to_string_pretty(&json!({
                "query": query,
                "version": version,
                "count": results.len(),
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
    async fn lookup_icd9_hypertension_finds_401() {
        let tool = IcdLookupTool;
        let result = tool
            .execute(json!({"query": "hypertension", "version": "ICD-9"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(
            result.content.contains("401"),
            "expected ICD-9 401.x: {}",
            result.content
        );
        assert!(result.content.contains("ICD-9"));
    }

    #[tokio::test]
    async fn lookup_icd10_hypertension_finds_i10() {
        let tool = IcdLookupTool;
        let result = tool
            .execute(json!({"query": "hypertension", "version": "ICD-10"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(
            result.content.contains("I10"),
            "expected ICD-10 I10: {}",
            result.content
        );
        assert!(result.content.contains("ICD-10"));
        // ICD-9 results must NOT appear when version is ICD-10.
        assert!(!result.content.contains("\"version\": \"ICD-9\""));
    }

    #[tokio::test]
    async fn lookup_default_version_is_icd9() {
        // No version param → defaults to ICD-9.
        let tool = IcdLookupTool;
        let result = tool.execute(json!({"query": "asthma"})).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("ICD-9"));
        assert!(
            result.content.contains("493"),
            "expected ICD-9 asthma 493.x"
        );
    }

    #[tokio::test]
    async fn lookup_both_returns_mixed_versions() {
        let tool = IcdLookupTool;
        let result = tool
            .execute(json!({"query": "diabetes", "version": "both"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(
            result.content.contains("ICD-9"),
            "both should include ICD-9"
        );
        assert!(
            result.content.contains("ICD-10"),
            "both should include ICD-10"
        );
    }

    #[tokio::test]
    async fn lookup_unknown_returns_empty() {
        let tool = IcdLookupTool;
        let result = tool
            .execute(json!({"query": "xyzzy_nonexistent_condition_12345", "version": "ICD-9"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(
            result.content.contains("No ICD-9 codes found"),
            "expected empty-message, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn lookup_empty_query_returns_error() {
        let tool = IcdLookupTool;
        let result = tool.execute(json!({"query": ""})).await.unwrap();
        assert!(result.is_error, "empty query should be an error");
    }

    #[tokio::test]
    async fn lookup_icd9_caps_results() {
        // "pain" is generic and should hit many entries; verify the cap.
        let tool = IcdLookupTool;
        let result = tool
            .execute(json!({"query": "pain", "version": "ICD-9"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        // The count field reflects the returned (capped) result count.
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap_or_default();
        let count = parsed.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        assert!(
            count <= MAX_ICD9_RESULTS as u64,
            "ICD-9 results should be capped at {MAX_ICD9_RESULTS}, got {count}"
        );
    }

    #[test]
    fn tool_definition_has_correct_name() {
        let tool = IcdLookupTool;
        assert_eq!(tool.definition().name, "search_icd_codes");
    }
}
