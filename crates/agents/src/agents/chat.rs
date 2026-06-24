use async_trait::async_trait;
use medical_core::{
    error::{AppError, AppResult},
    traits::Agent,
    types::{AgentContext, AgentResponse, ToolDef},
};
use serde_json::json;

/// General-purpose conversational medical AI agent with access to all tools.
///
/// The `ChatAgent` declares all five built-in tools (ICD lookup, drug
/// interactions, vitals extraction, knowledge-base search, and checklist
/// generation), making it the most capable agent for ad-hoc clinical
/// questions. Use it as the default agent for the chat UI.
pub struct ChatAgent;

#[async_trait]
impl Agent for ChatAgent {
    fn name(&self) -> &str {
        "chat"
    }

    fn description(&self) -> &str {
        "General-purpose conversational medical assistant with full tool access for answering clinical questions, looking up information, and supporting a wide range of clinical tasks."
    }

    fn system_prompt(&self) -> &str {
        "You are a knowledgeable, conversational medical assistant with broad clinical expertise and access to \
        a full suite of clinical tools. You assist healthcare providers with a wide range of tasks including: \
        answering clinical questions with evidence-based responses; looking up ICD-10 diagnostic codes for \
        conditions; checking drug-drug interactions for patient medication lists; extracting and interpreting \
        vital signs from clinical notes; searching the medical knowledge base for relevant clinical evidence; \
        and generating procedure-specific checklists. Maintain a professional, clear, and concise communication \
        style appropriate for clinical settings. When uncertain, acknowledge the limits of your knowledge and \
        recommend consulting primary literature or subspecialty colleagues. Always prioritize patient safety: \
        flag dangerous drug interactions, critical vital sign abnormalities, and red-flag symptoms prominently. \
        Never provide definitive diagnoses or prescribe treatments — support clinical decision making while \
        deferring final decisions to licensed providers."
    }

    fn available_tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "search_icd_codes".into(),
                description: "Search for ICD diagnostic codes matching clinical terms".into(),
                parameters: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
            },
            ToolDef {
                name: "lookup_drug_interactions".into(),
                description: "Check for drug-drug interactions among a list of medications".into(),
                parameters: json!({"type": "object", "properties": {"medications": {"type": "array", "items": {"type": "string"}}}, "required": ["medications"]}),
            },
            ToolDef {
                name: "extract_vitals".into(),
                description: "Extract vital signs from clinical text".into(),
                parameters: json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
            },
            ToolDef {
                name: "search_knowledge_base".into(),
                description: "Search the medical knowledge base for relevant clinical evidence"
                    .into(),
                parameters: json!({"type": "object", "properties": {"query": {"type": "string"}, "top_k": {"type": "integer", "default": 5}}, "required": ["query"]}),
            },
            ToolDef {
                name: "generate_checklist".into(),
                description: "Generate a clinical checklist for a procedure or encounter type"
                    .into(),
                parameters: json!({"type": "object", "properties": {"procedure": {"type": "string"}, "context": {"type": "string"}}, "required": ["procedure"]}),
            },
        ]
    }

    async fn execute(&self, _context: AgentContext) -> AppResult<AgentResponse> {
        Err(AppError::Agent(
            "Use AgentOrchestrator::execute instead".into(),
        ))
    }
}
