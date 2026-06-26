use async_trait::async_trait;
use medical_core::{
    error::{AppError, AppResult},
    traits::Agent,
    types::{AgentContext, AgentResponse, ToolDef},
};
use serde_json::json;

/// Agent specializing in differential diagnosis and ICD code assignment.
///
/// Has access to ICD code search and vitals extraction. Its system prompt
/// covers prioritised differential lists, confidence percentages, red-flag
/// identification, and workup suggestions.
pub struct DiagnosticAgent;

#[async_trait]
impl Agent for DiagnosticAgent {
    fn name(&self) -> &str {
        "diagnostic"
    }

    fn description(&self) -> &str {
        "Assists with differential diagnosis generation, ICD-10 code assignment, and clinical reasoning based on patient presentation, vitals, and history."
    }

    fn system_prompt(&self) -> &str {
        "You are a diagnostic reasoning assistant with expertise in clinical decision support and ICD-10 coding. \
        Your responsibilities include: (1) generating a prioritized differential diagnosis list based on presenting \
        symptoms, vital signs, history, and examination findings; (2) assigning confidence percentages to each \
        differential (e.g., 'Hypertension [I10] - 85%'); (3) identifying 'red flag' symptoms that require urgent \
        evaluation; (4) suggesting appropriate workup including labs, imaging, and specialty referrals; \
        (5) extracting and interpreting vital signs from clinical notes to support diagnosis. Always present \
        differentials in order of likelihood, include ICD-10 codes, and provide clinical reasoning for each. \
        Acknowledge diagnostic uncertainty clearly and recommend evidence-based evaluation pathways."
    }

    fn available_tools(&self) -> Vec<ToolDef> {
        vec![
            super::icd_lookup_tool_def(),
            ToolDef {
                name: "extract_vitals".into(),
                description: "Extract vital signs from clinical text".into(),
                parameters: json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
            },
        ]
    }

    async fn execute(&self, _context: AgentContext) -> AppResult<AgentResponse> {
        Err(AppError::Agent(
            "Use AgentOrchestrator::execute instead".into(),
        ))
    }
}
