//! Builds the manifest.json that accompanies a training-corpus export.

use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct Manifest {
    pub schema_version: u32,
    pub exported_at: String, // RFC3339
    pub ferri_scribe_version: String,
    pub corpus_size: CorpusSize,
    pub base_model_filter: Vec<String>,
    pub prompt_template_filter: Vec<String>,
    pub redaction_strictness: String, // 'standard' | 'aggressive'
    pub redaction_rules_applied: Vec<String>,
    pub warnings: Vec<Warning>,
}

#[derive(Serialize, Clone)]
pub struct CorpusSize {
    pub pairs: u32,
    pub input_tokens_est: u64,
    pub output_tokens_est: u64,
}

#[derive(Serialize, Clone)]
pub struct Warning {
    pub row_index: u32,
    pub reason: String,
}

/// Cheap token estimate: ~1 token per 4 characters of UTF-8 text.
/// Accurate enough for the manifest's "estimated tokens" field.
pub fn estimate_tokens(s: &str) -> u64 {
    (s.chars().count() as f64 / 4.0).ceil() as u64
}

pub fn write_manifest(manifest: &Manifest, path: &std::path::Path) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn manifest_serializes_with_pretty_format() {
        let m = Manifest {
            schema_version: 1,
            exported_at: "2026-05-11T22:30:00Z".to_string(),
            ferri_scribe_version: "0.10.56".to_string(),
            corpus_size: CorpusSize {
                pairs: 10,
                input_tokens_est: 1000,
                output_tokens_est: 500,
            },
            base_model_filter: vec!["llama3:70b".to_string()],
            prompt_template_filter: vec!["soap-default".to_string()],
            redaction_strictness: "standard".to_string(),
            redaction_rules_applied: vec!["SSN".to_string(), "PT_NAME".to_string()],
            warnings: vec![],
        };
        let json = serde_json::to_string_pretty(&m).unwrap();
        assert!(json.contains("\"schema_version\""));
        assert!(json.contains("\"pairs\": 10"));
    }
}
