use async_trait::async_trait;
use medical_core::{traits::Agent, types::ToolDef};
use serde_json::json;

/// Agent specializing in extracting structured clinical data from unstructured text.
///
/// Has access to the vitals extraction tool. Its system prompt covers
/// vital-sign parsing, lab-result structuring, medication-list compilation,
/// diagnosis extraction with ICD-10 codes, and allergy documentation.
pub struct DataExtractionAgent;

#[async_trait]
impl Agent for DataExtractionAgent {
    fn name(&self) -> &str {
        "data_extraction"
    }

    fn description(&self) -> &str {
        "Extracts structured clinical data from unstructured medical text including vital signs, laboratory values, medications, diagnoses, and allergies."
    }

    fn system_prompt(&self) -> &str {
        "You are a clinical data extraction specialist trained to convert unstructured medical text into \
        structured, machine-readable data. Your responsibilities include: (1) extracting all documented vital \
        signs (blood pressure, heart rate, temperature, respiratory rate, oxygen saturation, weight, height, BMI) \
        with their values, units, and timestamps when available; (2) identifying and structuring laboratory \
        results including test name, value, units, reference range, and interpretation flag; (3) compiling \
        complete medication lists with drug name, dose, route, frequency, and indication when documented; \
        (4) extracting diagnoses with ICD-10 codes when assignable, distinguishing active from historical problems; \
        (5) documenting allergies with allergen, reaction type, and severity. Output extracted data as structured \
        JSON when possible. Flag any ambiguous or incomplete values for clinician review."
    }

    fn available_tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "extract_vitals".into(),
            description: "Extract vital signs from clinical text using pattern matching".into(),
            parameters: json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
        }]
    }
}
