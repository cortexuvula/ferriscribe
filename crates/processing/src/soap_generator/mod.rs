//! System and user prompt builders for SOAP note generation.
//!
//! The system prompt uses a default template with placeholder tokens
//! (`{icd_label}`, `{icd_instruction}`, `{template_guidance}`). A user-supplied
//! `custom_prompt` overrides the default template; placeholders in either are
//! resolved at generation time via [`prompt_resolver::resolve_prompt`](crate::prompt_resolver::resolve_prompt).
//!
//! # Module layout
//!
//! - `prompt_template` — the built-in default prompt and [`build_soap_prompt`].
//! - `user_prompt` — [`build_user_prompt`], plus the `sanitize_prompt` helper.
//! - `postprocess` — markdown cleanup and section formatting on AI output.
//!
//! # Critical Constraint: Anti-Fabrication
//!
//! The default SOAP system prompt is a precision instrument (~280 lines).
//! Background-supplied patient context (medications, allergies, conditions,
//! supplementary notes) populates **historical Subjective fields only** —
//! it must never alter today's Objective findings, Assessment, Differential
//! Diagnosis, or Plan. The prompt contains explicit guards, a FORBIDDEN
//! INFERENCES block, two few-shot examples demonstrating disciplined
//! extraction, and a 10-point self-check checklist.
//!
//! If you modify the prompt, run the full test suite for this module — the
//! tests encode dozens of invariants about prompt structure, section ordering,
//! and fabrication guards.

use medical_core::icd9::Icd9Entry;
use medical_core::types::settings::SoapTemplate;

mod postprocess;
mod prompt_template;
mod user_prompt;

pub mod icd_selector;

pub use postprocess::postprocess_soap;
pub use prompt_template::{build_soap_prompt, default_soap_prompt};
pub use user_prompt::build_user_prompt;

/// Inputs to [`build_soap_prompt`].
///
/// Controls which template variant, ICD version, and optional custom override
/// are used when constructing the SOAP system prompt.
#[derive(Debug, Clone)]
pub struct SoapPromptConfig {
    /// Template variant that selects template-specific guidance text
    /// (e.g., "focus on changes since last visit" for FollowUp).
    pub template: SoapTemplate,
    /// One of `"ICD-9"`, `"ICD-10"`, `"both"` (case-sensitive).
    /// Determines the ICD code label and instruction placeholders.
    pub icd_version: String,
    /// User-supplied override for the entire system prompt. Empty string is
    /// treated as absent and falls back to the default template.
    pub custom_prompt: Option<String>,
    /// Clinically relevant BC MSP ICD-9 candidates selected from the
    /// visit's source text. Injected into the prompt as a constrained
    /// vocabulary so the model selects from accepted codes rather than
    /// fabricating. Empty for ICD-10-only mode.
    pub icd9_candidates: Vec<Icd9Entry>,
}

impl Default for SoapPromptConfig {
    fn default() -> Self {
        Self {
            template: SoapTemplate::FollowUp,
            icd_version: "ICD-10".into(),
            custom_prompt: None,
            icd9_candidates: vec![],
        }
    }
}
