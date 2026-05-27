use async_trait::async_trait;
use medical_core::{
    error::{AppError, AppResult},
    traits::Agent,
    types::{AgentContext, AgentResponse, ToolDef},
};
use serde_json::json;

/// Agent specializing in medication safety, drug interactions, and dosage validation.
///
/// Has access to the drug-interaction lookup tool and ICD code search.
/// Its system prompt covers severity ratings, Beers Criteria for elderly
/// patients, therapeutic duplication checks, and dosage validation.
pub struct MedicationAgent;

#[async_trait]
impl Agent for MedicationAgent {
    fn name(&self) -> &str {
        "medication"
    }

    fn description(&self) -> &str {
        "Provides clinical pharmacology support including drug interaction checking, dosage validation, and Beers Criteria assessment for elderly patients."
    }

    fn system_prompt(&self) -> &str {
        "You are a clinical pharmacology assistant specializing in medication safety and pharmacotherapy. \
        Your responsibilities include: (1) identifying clinically significant drug-drug interactions with \
        severity ratings (contraindicated, major, moderate, minor); (2) validating medication dosages against \
        standard references considering renal/hepatic function and patient age; (3) applying Beers Criteria to \
        flag potentially inappropriate medications in elderly patients (≥65 years); (4) reviewing medication \
        lists for therapeutic duplications or class overlaps; (5) providing ICD-10 codes for diagnosed conditions \
        when relevant to medication decisions. Always cite clinical reasoning and recommend monitoring parameters. \
        Do not prescribe — provide decision support for licensed prescribers."
    }

    fn available_tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "lookup_drug_interactions".into(),
                description: "Check for drug-drug interactions among a list of medications".into(),
                parameters: json!({"type": "object", "properties": {"medications": {"type": "array", "items": {"type": "string"}}}, "required": ["medications"]}),
            },
            ToolDef {
                name: "search_icd_codes".into(),
                description: "Search for ICD diagnostic codes".into(),
                parameters: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
            },
        ]
    }

    async fn execute(&self, _context: AgentContext) -> AppResult<AgentResponse> {
        Err(AppError::Agent(
            "Use AgentOrchestrator::execute instead".into(),
        ))
    }
}
