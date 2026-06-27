use async_trait::async_trait;
use medical_core::{traits::Agent, types::ToolDef};
use serde_json::json;

/// Agent providing step-by-step clinical workflow guidance and procedure checklists.
///
/// Has access to the checklist generation tool. Its system prompt covers
/// evidence-based care pathways, pre/post-procedure checklists, documentation
/// requirements, and setting-specific workflow adaptation (inpatient,
/// outpatient, emergency).
pub struct WorkflowAgent;

#[async_trait]
impl Agent for WorkflowAgent {
    fn name(&self) -> &str {
        "workflow"
    }

    fn description(&self) -> &str {
        "Guides clinicians through step-by-step clinical workflows, procedure checklists, and evidence-based care pathways for common medical encounters."
    }

    fn system_prompt(&self) -> &str {
        "You are a clinical workflow assistant that provides step-by-step procedural guidance and care pathway \
        support for healthcare providers. Your responsibilities include: (1) generating detailed, actionable \
        checklists for clinical procedures, patient encounters, and administrative tasks; (2) walking clinicians \
        through evidence-based care pathways for common conditions (e.g., sepsis bundle, ACS protocol, \
        hypertensive urgency management); (3) providing pre-procedure and post-procedure checklists with safety \
        verification steps; (4) outlining documentation requirements for each step of a clinical workflow; \
        (5) adapting workflows to specific clinical contexts such as inpatient, outpatient, or emergency settings. \
        Present guidance in numbered steps with clear actions, responsible parties, and time-sensitive elements \
        highlighted. Always note when steps require licensed provider authorization or co-signature."
    }

    fn available_tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "generate_checklist".into(),
            description:
                "Generate a step-by-step clinical checklist for a procedure or encounter type"
                    .into(),
            parameters: json!({"type": "object", "properties": {"procedure": {"type": "string"}, "context": {"type": "string"}}, "required": ["procedure"]}),
        }]
    }
}
