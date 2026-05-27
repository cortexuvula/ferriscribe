use async_trait::async_trait;
use medical_core::{
    error::{AppError, AppResult},
    traits::Agent,
    types::{AgentContext, AgentResponse, ToolDef},
};
use serde_json::json;

/// Agent specializing in clinical documentation compliance and SOAP note auditing.
///
/// Has access to the checklist generation tool. Its system prompt covers
/// SOAP documentation standards, ICD-10 code verification, E/M guideline
/// compliance, and claim-denial risk identification.
pub struct ComplianceAgent;

#[async_trait]
impl Agent for ComplianceAgent {
    fn name(&self) -> &str {
        "compliance"
    }

    fn description(&self) -> &str {
        "Audits clinical documentation for compliance with SOAP note standards, coding requirements, and regulatory documentation guidelines."
    }

    fn system_prompt(&self) -> &str {
        "You are a clinical documentation compliance specialist with expertise in SOAP note auditing and \
        medical coding standards. Your responsibilities include: (1) auditing clinical notes against SOAP \
        (Subjective, Objective, Assessment, Plan) documentation standards and identifying missing elements; \
        (2) verifying that diagnoses are supported by documented findings and that ICD-10 codes are correctly \
        assigned; (3) checking for completeness of required documentation elements including chief complaint, \
        HPI, ROS, physical exam, medical decision making, and time-based billing if applicable; \
        (4) identifying documentation deficiencies that could result in claim denials or audit risk; \
        (5) generating compliance checklists for specific encounter types. Always provide specific, actionable \
        feedback referencing documentation standards such as E/M guidelines. Flag critical deficiencies that \
        could affect patient safety or reimbursement."
    }

    fn available_tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "generate_checklist".into(),
            description: "Generate a clinical documentation checklist for compliance review".into(),
            parameters: json!({"type": "object", "properties": {"procedure": {"type": "string"}, "context": {"type": "string"}}, "required": ["procedure"]}),
        }]
    }

    async fn execute(&self, _context: AgentContext) -> AppResult<AgentResponse> {
        Err(AppError::Agent(
            "Use AgentOrchestrator::execute instead".into(),
        ))
    }
}
