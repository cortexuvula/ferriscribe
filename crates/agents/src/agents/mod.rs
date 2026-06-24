//! Agent implementations for clinical AI chat.
//!
//! Each agent specialises in a clinical domain and declares which tools it
//! may invoke. All agents share the same execution path — they are driven by
//! [`AgentOrchestrator`](crate::orchestrator::AgentOrchestrator), **not** by
//! calling their `execute()` method directly.
//!
//! | Agent | Tools | Purpose |
//! |---|---|---|
//! | [`ChatAgent`] | All 5 | General-purpose conversational assistant |
//! | [`MedicationAgent`] | drug interactions, ICD lookup | Drug safety and pharmacotherapy |
//! | [`DiagnosticAgent`] | ICD lookup, vitals extraction | Differential diagnosis and ICD-10 coding |
//! | [`ComplianceAgent`] | checklist | SOAP note auditing and compliance |
//! | [`DataExtractionAgent`] | vitals extraction | Structured data from unstructured text |
//! | [`WorkflowAgent`] | checklist | Step-by-step clinical workflow guidance |
//! | [`ReferralAgent`] | ICD lookup | Referral letter generation |
//! | [`SynopsisAgent`] | _(none)_ | Concise SOAP note summaries (<200 words) |

pub mod chat;
pub mod compliance;
pub mod data_extraction;
pub mod diagnostic;
pub mod medication;
pub mod referral;
pub mod synopsis;
pub mod workflow;

pub use chat::ChatAgent;
pub use compliance::ComplianceAgent;
pub use data_extraction::DataExtractionAgent;
pub use diagnostic::DiagnosticAgent;
pub use medication::MedicationAgent;
pub use referral::ReferralAgent;
pub use synopsis::SynopsisAgent;
pub use workflow::WorkflowAgent;

use medical_core::traits::Agent;

/// Returns all 8 medical agents as boxed trait objects.
///
/// Useful for registering the full agent catalogue with a dispatcher or
/// for iterating over agents in tests and UI code.
pub fn all_agents() -> Vec<Box<dyn Agent>> {
    vec![
        Box::new(MedicationAgent),
        Box::new(DiagnosticAgent),
        Box::new(ComplianceAgent),
        Box::new(DataExtractionAgent),
        Box::new(WorkflowAgent),
        Box::new(ReferralAgent),
        Box::new(SynopsisAgent),
        Box::new(ChatAgent),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_agents_have_unique_names() {
        let agents = all_agents();
        assert_eq!(agents.len(), 8, "Expected exactly 8 agents");

        let mut names = std::collections::HashSet::new();
        for agent in &agents {
            let name = agent.name().to_string();
            assert!(
                names.insert(name.clone()),
                "Duplicate agent name: '{}'",
                name
            );
        }
        assert_eq!(names.len(), 8);
    }

    #[test]
    fn chat_agent_has_all_tools() {
        let agent = ChatAgent;
        let tools = agent.available_tools();
        assert!(
            tools.len() >= 5,
            "ChatAgent should have at least 5 tools, found {}",
            tools.len()
        );

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            tool_names.contains(&"search_icd_codes"),
            "Missing search_icd_codes"
        );
        assert!(
            tool_names.contains(&"lookup_drug_interactions"),
            "Missing lookup_drug_interactions"
        );
        assert!(
            tool_names.contains(&"extract_vitals"),
            "Missing extract_vitals"
        );
        assert!(
            tool_names.contains(&"search_knowledge_base"),
            "Missing search_knowledge_base"
        );
        assert!(
            tool_names.contains(&"generate_checklist"),
            "Missing generate_checklist"
        );
    }

    #[test]
    fn synopsis_agent_has_no_tools() {
        let agent = SynopsisAgent;
        let tools = agent.available_tools();
        assert!(
            tools.is_empty(),
            "SynopsisAgent should have no tools, found {}",
            tools.len()
        );
    }

    #[test]
    fn all_agents_have_system_prompts() {
        let agents = all_agents();
        for agent in &agents {
            let prompt = agent.system_prompt();
            assert!(
                !prompt.is_empty(),
                "Agent '{}' has an empty system prompt",
                agent.name()
            );
            assert!(
                prompt.len() > 50,
                "Agent '{}' system prompt is too short ({} chars), expected >50",
                agent.name(),
                prompt.len()
            );
        }
    }

    #[test]
    fn all_agents_have_descriptions() {
        let agents = all_agents();
        for agent in &agents {
            let desc = agent.description();
            assert!(
                !desc.is_empty(),
                "Agent '{}' has an empty description",
                agent.name()
            );
        }
    }

    #[test]
    fn medication_agent_tools() {
        let agent = MedicationAgent;
        let tools = agent.available_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"lookup_drug_interactions"));
        assert!(names.contains(&"search_icd_codes"));
    }

    #[test]
    fn diagnostic_agent_tools() {
        let agent = DiagnosticAgent;
        let tools = agent.available_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"search_icd_codes"));
        assert!(names.contains(&"extract_vitals"));
    }
}
