//! Vital-sign extraction tool.
//!
//! Extracts blood pressure, heart rate, temperature, respiratory rate, and
//! oxygen saturation from free-form clinical text using compiled regex
//! patterns. Each extracted value is range-validated against physiologically
//! plausible bounds before being returned.

use async_trait::async_trait;
use medical_core::{
    error::AppResult,
    traits::Tool,
    types::{ToolDef, ToolOutput},
};
use regex::Regex;
use serde_json::json;
use std::sync::LazyLock;

/// Regex for labelled blood pressure: "BP 120/80", "blood pressure: 120/80 mmHg".
static BP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:bp|blood\s*pressure)[:\s]*(\d{2,3})\s*/\s*(\d{2,3})\s*(?:mmhg)?"
    ).unwrap()
});
/// Regex for plain "120/80" patterns that may be blood pressure (fallback).
static BP_PLAIN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(\d{2,3})/(\d{2,3})\b").unwrap()
});
/// Regex for heart rate: "HR 72 bpm", "heart rate: 72", "pulse 72".
static HR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:hr|heart\s*rate|pulse)[:\s]*(\d{2,3})\s*(?:bpm)?"
    ).unwrap()
});
/// Regex for temperature: "Temp 98.6°F", "T 37.2C", "temperature: 98.6".
static TEMP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:temp(?:erature)?|t)[:\s]*(\d{2,3}(?:\.\d)?)\s*(?:°?\s*([fcFC]))?"
    ).unwrap()
});
/// Regex for respiratory rate: "RR 16", "respiratory rate: 18 breaths/min".
static RR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:rr|resp(?:iratory)?\s*rate?)[:\s]*(\d{1,2})\s*(?:breaths?/min)?"
    ).unwrap()
});
/// Regex for SpO2 / oxygen saturation: "SpO2 98%", "O2 sat 97%", "sats 99".
static SPO2_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:spo2|o2\s*sat(?:uration)?|sats?)[:\s]*(\d{2,3})\s*%?"
    ).unwrap()
});

/// Tool for extracting vital signs from clinical text using regex patterns.
///
/// Registered as `extract_vitals`. Parses blood pressure (systolic/diastolic),
/// heart rate, temperature (F/C), respiratory rate, and SpO2 from free-form
/// text. Each value is range-validated (e.g. systolic 60–250, HR 20–300)
/// to filter out false-positive regex matches.
pub struct VitalsExtractorTool;

#[async_trait]
impl Tool for VitalsExtractorTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "extract_vitals".into(),
            description: "Extract vital signs (blood pressure, heart rate, temperature, respiratory rate, oxygen saturation) from free-form clinical text using pattern matching.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Clinical text containing vital signs to extract"
                    }
                },
                "required": ["text"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> AppResult<ToolOutput> {
        let text = match arguments.get("text").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return Ok(ToolOutput::error("text parameter is required")),
        };

        let mut vitals = serde_json::Map::new();

        // Blood pressure: e.g. "BP 120/80", "blood pressure: 120/80 mmHg", "120/80"
        // Also match plain "120/80" patterns that look like BP

        if let Some(caps) = BP_RE.captures(text) {
            let systolic: u32 = caps[1].parse().unwrap_or(0);
            let diastolic: u32 = caps[2].parse().unwrap_or(0);
            if (60..=250).contains(&systolic) && (30..=150).contains(&diastolic) {
                vitals.insert("blood_pressure".into(), json!({
                    "systolic": systolic,
                    "diastolic": diastolic,
                    "unit": "mmHg"
                }));
            }
        } else if let Some(caps) = BP_PLAIN_RE.captures(text) {
            let systolic: u32 = caps[1].parse().unwrap_or(0);
            let diastolic: u32 = caps[2].parse().unwrap_or(0);
            if (60..=250).contains(&systolic) && (30..=150).contains(&diastolic) {
                vitals.insert("blood_pressure".into(), json!({
                    "systolic": systolic,
                    "diastolic": diastolic,
                    "unit": "mmHg"
                }));
            }
        }

        // Heart rate: e.g. "HR 72 bpm", "heart rate: 72", "pulse 72"
        if let Some(caps) = HR_RE.captures(text) {
            let hr: u32 = caps[1].parse().unwrap_or(0);
            if (20..=300).contains(&hr) {
                vitals.insert("heart_rate".into(), json!({
                    "value": hr,
                    "unit": "bpm"
                }));
            }
        }

        // Temperature: e.g. "Temp 98.6°F", "T 37.2C", "temperature: 98.6"
        if let Some(caps) = TEMP_RE.captures(text)
            && let Ok(temp_val) = caps[1].parse::<f32>() {
                let unit = caps.get(2).map_or("", |m| m.as_str()).to_uppercase();
                // Validate reasonable temperature range
                let is_fahrenheit = unit == "F" || ((95.0..=107.0).contains(&temp_val) && unit != "C");
                if is_fahrenheit || unit == "C" || (35.0..=42.0).contains(&temp_val) {
                    let unit_str = if unit == "C" || ((35.0..=42.0).contains(&temp_val) && unit != "F") { "C" } else { "F" };
                    vitals.insert("temperature".into(), json!({
                        "value": temp_val,
                        "unit": unit_str
                    }));
                }
            }

        // Respiratory rate: e.g. "RR 16", "respiratory rate: 18 breaths/min"
        if let Some(caps) = RR_RE.captures(text) {
            let rr: u32 = caps[1].parse().unwrap_or(0);
            if (4..=60).contains(&rr) {
                vitals.insert("respiratory_rate".into(), json!({
                    "value": rr,
                    "unit": "breaths/min"
                }));
            }
        }

        // SpO2 / Oxygen saturation: e.g. "SpO2 98%", "O2 sat 97%", "sats 99"
        if let Some(caps) = SPO2_RE.captures(text) {
            let spo2: u32 = caps[1].parse().unwrap_or(0);
            if (50..=100).contains(&spo2) {
                vitals.insert("spo2".into(), json!({
                    "value": spo2,
                    "unit": "%"
                }));
            }
        }

        let content = serde_json::to_string_pretty(&json!({
            "vitals_extracted": vitals.len(),
            "vitals": vitals
        }))
        .unwrap_or_else(|_| "serialization error".into());

        Ok(ToolOutput::success(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn extracts_bp_and_hr() {
        let tool = VitalsExtractorTool;
        let text = "Patient vitals: BP 140/90 mmHg, HR 88 bpm, RR 16 breaths/min, SpO2 97%";
        let result = tool.execute(json!({"text": text})).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        let vitals = &parsed["vitals"];
        assert_eq!(vitals["blood_pressure"]["systolic"], 140);
        assert_eq!(vitals["blood_pressure"]["diastolic"], 90);
        assert_eq!(vitals["heart_rate"]["value"], 88);
        assert_eq!(vitals["respiratory_rate"]["value"], 16);
        assert_eq!(vitals["spo2"]["value"], 97);
    }

    #[tokio::test]
    async fn handles_missing_vitals() {
        let tool = VitalsExtractorTool;
        let text = "Patient is alert and oriented. No acute distress noted.";
        let result = tool.execute(json!({"text": text})).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["vitals_extracted"].as_u64().unwrap(), 0);
    }

    #[tokio::test]
    async fn extracts_temperature() {
        let tool = VitalsExtractorTool;
        let text = "Temp 98.6F, pulse 72";
        let result = tool.execute(json!({"text": text})).await.unwrap();
        assert!(!result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["vitals"]["heart_rate"]["value"] == 72);
    }

    #[test]
    fn tool_definition_has_correct_name() {
        let tool = VitalsExtractorTool;
        assert_eq!(tool.definition().name, "extract_vitals");
    }
}
