//! Clinical checklist generation tool.
//!
//! Generates numbered, step-by-step checklists for common clinical encounter
//! types. Recognises "new patient" and "follow-up" keywords to select a
//! specialised template; all other procedures fall back to a general
//! clinical checklist.

use async_trait::async_trait;
use medical_core::{
    error::AppResult,
    traits::Tool,
    types::{ToolDef, ToolOutput},
};
use serde_json::json;

/// Tool for generating procedure-specific clinical checklists.
///
/// Registered as `generate_checklist`. Returns a numbered JSON checklist
/// for the given procedure or encounter type. Three templates are available:
///
/// - **New Patient Visit** (15 steps) — triggered by "new patient", "new pt", "intake"
/// - **Follow-Up Visit** (13 steps) — triggered by "follow", "f/u", "followup"
/// - **General Clinical Checklist** (8 steps) — fallback for all other inputs
pub struct ChecklistTool;

/// 15-step checklist for a new patient intake visit.
fn new_patient_checklist() -> Vec<&'static str> {
    vec![
        "Verify patient identity (name, DOB, MRN)",
        "Review chief complaint and reason for visit",
        "Obtain complete medication list (including OTC and supplements)",
        "Document known allergies and reactions",
        "Take complete medical and surgical history",
        "Review family history for relevant hereditary conditions",
        "Obtain social history (smoking, alcohol, occupation, living situation)",
        "Perform review of systems (ROS)",
        "Measure and record vital signs (BP, HR, Temp, RR, SpO2, weight, height)",
        "Perform physical examination relevant to chief complaint",
        "Order appropriate baseline labs and diagnostics",
        "Document assessment and differential diagnosis",
        "Establish plan of care and patient education",
        "Schedule follow-up appointment as appropriate",
        "Ensure medication reconciliation is complete",
    ]
}

/// 13-step checklist for a follow-up visit.
fn follow_up_checklist() -> Vec<&'static str> {
    vec![
        "Verify patient identity and confirm reason for follow-up",
        "Review interval history since last visit",
        "Check adherence to previously prescribed medications",
        "Review results of any pending labs or diagnostics",
        "Measure and record current vital signs",
        "Assess response to treatment (improvement, worsening, side effects)",
        "Update problem list with any new diagnoses or resolved issues",
        "Review medication list for changes or adjustments needed",
        "Perform focused physical exam relevant to ongoing conditions",
        "Update care plan based on current findings",
        "Address any new concerns raised by patient",
        "Document patient education provided",
        "Schedule next follow-up as clinically indicated",
    ]
}

/// 8-step general clinical checklist (fallback for unrecognised procedures).
fn general_checklist() -> Vec<&'static str> {
    vec![
        "Identify the clinical goal and relevant patient information",
        "Review applicable guidelines and evidence-based protocols",
        "Verify patient allergies and contraindications",
        "Confirm informed consent where applicable",
        "Document all clinical decisions with rationale",
        "Ensure care coordination with relevant specialties",
        "Provide clear patient instructions and education",
        "Arrange appropriate follow-up",
    ]
}

#[async_trait]
impl Tool for ChecklistTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "generate_checklist".into(),
            description: "Generate a step-by-step clinical checklist for common medical procedures and encounters including new patient visits, follow-ups, and other clinical workflows.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "procedure": {
                        "type": "string",
                        "description": "The procedure or encounter type for which to generate a checklist (e.g., 'new patient', 'follow-up', 'medication reconciliation')"
                    },
                    "context": {
                        "type": "string",
                        "description": "Additional clinical context to tailor the checklist (optional)"
                    }
                },
                "required": ["procedure"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> AppResult<ToolOutput> {
        let procedure = match arguments.get("procedure").and_then(|v| v.as_str()) {
            Some(p) => p.to_lowercase(),
            None => return Ok(ToolOutput::error("procedure parameter is required")),
        };

        let context = arguments
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let (checklist_type, items): (&str, Vec<&str>) =
            if procedure.contains("new patient") || procedure.contains("new pt") || procedure.contains("intake") {
                ("New Patient Visit", new_patient_checklist())
            } else if procedure.contains("follow") || procedure.contains("f/u") || procedure.contains("followup") {
                ("Follow-Up Visit", follow_up_checklist())
            } else {
                ("General Clinical Checklist", general_checklist())
            };

        let numbered_items: Vec<serde_json::Value> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                json!({
                    "step": i + 1,
                    "action": item,
                    "completed": false
                })
            })
            .collect();

        let mut response = json!({
            "checklist_type": checklist_type,
            "procedure": procedure,
            "total_steps": numbered_items.len(),
            "checklist": numbered_items
        });

        if !context.is_empty() {
            response["context"] = json!(context);
        }

        let content = serde_json::to_string_pretty(&response)
            .unwrap_or_else(|_| "serialization error".into());

        Ok(ToolOutput::success(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn checklist_for_new_patient() {
        let tool = ChecklistTool;
        let result = tool
            .execute(json!({"procedure": "new patient visit"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["checklist_type"], "New Patient Visit");
        let steps = parsed["total_steps"].as_u64().unwrap();
        assert!(steps >= 10, "New patient checklist should have at least 10 steps");
        // Verify step structure
        let checklist = parsed["checklist"].as_array().unwrap();
        assert!(!checklist.is_empty());
        assert!(checklist[0]["step"].as_u64().unwrap() == 1);
        assert!(checklist[0]["action"].is_string());
    }

    #[tokio::test]
    async fn checklist_for_follow_up() {
        let tool = ChecklistTool;
        let result = tool
            .execute(json!({"procedure": "follow-up visit", "context": "diabetes management"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["checklist_type"], "Follow-Up Visit");
        assert_eq!(parsed["context"], "diabetes management");
    }

    #[tokio::test]
    async fn checklist_general_fallback() {
        let tool = ChecklistTool;
        let result = tool
            .execute(json!({"procedure": "medication reconciliation"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["checklist_type"], "General Clinical Checklist");
    }

    #[test]
    fn tool_definition() {
        let tool = ChecklistTool;
        let def = tool.definition();
        assert_eq!(def.name, "generate_checklist");
        assert!(!def.description.is_empty());
        let params = &def.parameters;
        assert_eq!(params["required"][0], "procedure");
    }
}
