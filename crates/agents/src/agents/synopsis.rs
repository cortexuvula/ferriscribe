use async_trait::async_trait;
use medical_core::{
    error::{AppError, AppResult},
    traits::Agent,
    types::{AgentContext, AgentResponse, ToolDef},
};

/// Agent specializing in generating concise SOAP note summaries.
///
/// Has **no tools** — it works purely from the text provided in the context.
/// Its system prompt mandates output under 200 words, structured as
/// Subjective / Objective / Assessment / Plan, with standard medical
/// abbreviations for maximum information density.
pub struct SynopsisAgent;

#[async_trait]
impl Agent for SynopsisAgent {
    fn name(&self) -> &str {
        "synopsis"
    }

    fn description(&self) -> &str {
        "Generates concise SOAP note summaries under 200 words, distilling the key clinical information from longer clinical documentation."
    }

    fn system_prompt(&self) -> &str {
        "You are a clinical documentation synthesis specialist responsible for creating concise, high-quality \
        SOAP note summaries from detailed clinical encounters. Your output must always be under 200 words while \
        preserving all clinically significant information. Structure every summary as: SUBJECTIVE (chief complaint, \
        relevant history, patient-reported symptoms in 1-2 sentences), OBJECTIVE (pertinent vital signs and \
        examination findings in 1-2 sentences), ASSESSMENT (primary diagnosis with ICD-10 code and key \
        differentials if relevant), and PLAN (top 3-5 action items: medications, orders, referrals, follow-up). \
        Omit non-essential details and normal findings unless clinically relevant. Use standard medical \
        abbreviations to maximize information density. Prioritize accuracy over completeness — never infer \
        information not documented in the source material. Flag any critical safety information (allergies, \
        drug interactions, abnormal vitals) prominently."
    }

    fn available_tools(&self) -> Vec<ToolDef> {
        // Synopsis agent works from provided text only — no tools needed
        vec![]
    }

    async fn execute(&self, _context: AgentContext) -> AppResult<AgentResponse> {
        Err(AppError::Agent(
            "Use AgentOrchestrator::execute instead".into(),
        ))
    }
}
