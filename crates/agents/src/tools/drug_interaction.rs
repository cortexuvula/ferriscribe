//! Drug-drug interaction lookup tool.
//!
//! Checks all pairs in a medication list against a hardcoded table of known
//! interactions. Each interaction has a severity level (CONTRAINDICATED,
//! MAJOR, MODERATE) and a clinical guidance description.

use async_trait::async_trait;
use medical_core::{
    error::AppResult,
    traits::Tool,
    types::{ToolDef, ToolOutput},
};
use serde_json::json;

/// Tool for looking up drug-drug interactions.
///
/// Registered as `lookup_drug_interactions`. Accepts a list of medication
/// names, checks all pairs against a hardcoded interaction table (8 known
/// pairs), and returns any matches with severity and clinical guidance.
pub struct DrugInteractionTool;

/// Known drug interaction pairs (normalized lowercase), severity, and description.
///
/// Each entry is `(drug_a_pattern, drug_b_pattern, severity, description)`.
/// Matching uses bidirectional substring containment so "ibuprofen nsaid"
/// matches the pattern "nsaid".
const KNOWN_INTERACTIONS: &[(&str, &str, &str, &str)] = &[
    (
        "warfarin",
        "aspirin",
        "MAJOR",
        "Concurrent use of warfarin and aspirin significantly increases bleeding risk. Monitor INR closely.",
    ),
    (
        "metformin",
        "contrast",
        "MAJOR",
        "Iodinated contrast media can cause lactic acidosis when combined with metformin. Hold metformin 48h before/after contrast.",
    ),
    (
        "ssri",
        "maoi",
        "CONTRAINDICATED",
        "SSRIs and MAOIs together can cause life-threatening serotonin syndrome. Do not combine; allow washout period.",
    ),
    (
        "ace",
        "potassium",
        "MODERATE",
        "ACE inhibitors combined with potassium supplements or potassium-sparing diuretics can cause hyperkalemia.",
    ),
    (
        "statin",
        "grapefruit",
        "MODERATE",
        "Grapefruit inhibits CYP3A4 metabolism of certain statins (lovastatin, simvastatin, atorvastatin), increasing myopathy risk.",
    ),
    (
        "methotrexate",
        "nsaid",
        "MAJOR",
        "NSAIDs reduce renal clearance of methotrexate, increasing toxicity risk. Avoid combination or monitor closely.",
    ),
    (
        "lithium",
        "nsaid",
        "MAJOR",
        "NSAIDs reduce renal clearance of lithium, potentially causing lithium toxicity. Monitor lithium levels.",
    ),
    (
        "warfarin",
        "nsaid",
        "MAJOR",
        "NSAIDs combined with warfarin increase bleeding risk through platelet inhibition and GI irritation.",
    ),
];

/// Lowercase and trim a drug name for comparison.
fn normalize(s: &str) -> String {
    s.to_lowercase().trim().to_string()
}

/// Word-boundary match: returns true if `pattern` matches `drug` as a complete
/// word/token (case-insensitive), NOT as a substring.
///
/// This allows "ibuprofen nsaid" to match the pattern "nsaid" (because "nsaid"
/// is a token in "ibuprofen nsaid") but prevents "ace" from matching
/// "acetaminophen" (a dangerous false positive that would raise a spurious
/// ACE-inhibitor interaction warning).
fn drugs_match(drug: &str, pattern: &str) -> bool {
    let drug_lower = normalize(drug);
    let pattern_lower = normalize(pattern);

    // Exact match (case-insensitive).
    if drug_lower == pattern_lower {
        return true;
    }

    // Word-boundary matching: the pattern must appear as a complete word
    // in the drug name, not as a substring. E.g., "ace" should match
    // "ace inhibitor" but NOT "acetaminophen".
    drug_lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == pattern_lower)
}

/// A single detected drug-drug interaction between two medications.
#[derive(Debug, Clone)]
pub struct DrugInteraction {
    pub drug_a: String,
    pub drug_b: String,
    pub severity: String,
    pub description: String,
}

/// Check all pairs in a medication list against the known interaction table.
///
/// Returns one `DrugInteraction` per matching `(pattern_a, pattern_b)` rule.
/// A given drug pair may produce more than one result if multiple rules match.
pub fn check_interactions(medications: &[&str]) -> Vec<DrugInteraction> {
    let mut found = Vec::new();
    if medications.len() < 2 {
        return found;
    }
    for i in 0..medications.len() {
        for j in (i + 1)..medications.len() {
            let drug_a = medications[i];
            let drug_b = medications[j];
            for (pattern_a, pattern_b, severity, description) in KNOWN_INTERACTIONS {
                let ab_match = drugs_match(drug_a, pattern_a) && drugs_match(drug_b, pattern_b);
                let ba_match = drugs_match(drug_a, pattern_b) && drugs_match(drug_b, pattern_a);
                if ab_match || ba_match {
                    found.push(DrugInteraction {
                        drug_a: drug_a.to_string(),
                        drug_b: drug_b.to_string(),
                        severity: severity.to_string(),
                        description: description.to_string(),
                    });
                }
            }
        }
    }
    found
}

#[async_trait]
impl Tool for DrugInteractionTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "lookup_drug_interactions".into(),
            description: "Check for known drug-drug interactions among a list of medications. Returns severity and clinical guidance for any identified interactions.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "medications": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "List of medication names to check for interactions",
                        "minItems": 2
                    }
                },
                "required": ["medications"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> AppResult<ToolOutput> {
        let medications = match arguments.get("medications").and_then(|v| v.as_array()) {
            Some(m) => m
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            None => {
                return Ok(ToolOutput::error(
                    "medications parameter must be an array of strings",
                ));
            }
        };

        if medications.len() < 2 {
            return Ok(ToolOutput::error(
                "At least 2 medications are required to check for interactions",
            ));
        }

        let meds_refs: Vec<&str> = medications.iter().map(|s| s.as_str()).collect();
        let interactions = check_interactions(&meds_refs);

        let interactions_found: Vec<serde_json::Value> = interactions
            .iter()
            .map(|i| {
                json!({
                    "drug_a": i.drug_a,
                    "drug_b": i.drug_b,
                    "severity": i.severity,
                    "description": i.description
                })
            })
            .collect();

        let content = serde_json::to_string_pretty(&json!({
            "medications_checked": medications,
            "interactions_found": interactions_found.len(),
            "interactions": interactions_found
        }))
        .unwrap_or_else(|_| "serialization error".into());

        Ok(ToolOutput::success(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detects_warfarin_aspirin() {
        let tool = DrugInteractionTool;
        let result = tool
            .execute(json!({"medications": ["warfarin", "aspirin"]}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("warfarin") || result.content.contains("MAJOR"));
        assert!(result.content.contains("interactions_found"));
        // Should find at least 1 interaction
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["interactions_found"].as_u64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn no_interaction_safe_combo() {
        let tool = DrugInteractionTool;
        let result = tool
            .execute(json!({"medications": ["amoxicillin", "acetaminophen"]}))
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["interactions_found"].as_u64().unwrap(), 0);
    }

    #[tokio::test]
    async fn detects_lithium_nsaid() {
        let tool = DrugInteractionTool;
        let result = tool
            .execute(json!({"medications": ["lithium", "ibuprofen nsaid"]}))
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["interactions_found"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn no_false_positive_acetaminophen_ace() {
        // "ace" should NOT match "acetaminophen" — this is a false positive
        // that could cause unnecessary alarm about ACE inhibitor interactions.
        let results = check_interactions(&["acetaminophen", "potassium chloride"]);
        // acetaminophen is NOT an ACE inhibitor, so no ACE-potassium interaction.
        let ace_interactions: Vec<_> = results
            .iter()
            .filter(|r| r.description.contains("ACE") || r.description.contains("Hyperkalemia"))
            .collect();
        assert!(
            ace_interactions.is_empty(),
            "acetaminophen should not trigger ACE interaction"
        );
    }

    #[test]
    fn ace_inhibitor_word_still_matches() {
        // "ace" SHOULD still match the phrase "ace inhibitor" (word token),
        // so a real ACE inhibitor + potassium combo is still flagged.
        let results = check_interactions(&["ace inhibitor", "potassium chloride"]);
        let ace_interactions: Vec<_> = results
            .iter()
            .filter(|r| r.description.contains("ACE") || r.description.contains("Hyperkalemia"))
            .collect();
        assert!(
            !ace_interactions.is_empty(),
            "ace inhibitor + potassium should still be flagged"
        );
    }

    #[test]
    fn tool_definition_has_correct_name() {
        let tool = DrugInteractionTool;
        assert_eq!(tool.definition().name, "lookup_drug_interactions");
    }
}
