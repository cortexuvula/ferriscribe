//! System and user prompt builders for SOAP note generation.
//!
//! The system prompt uses a default template with placeholder tokens
//! (`{icd_label}`, `{icd_instruction}`, `{template_guidance}`). A user-supplied
//! `custom_prompt` overrides the default template; placeholders in either are
//! resolved at generation time via `prompt_resolver::resolve_prompt`.
//!
//! Module layout:
//! - [`prompt_template`] — the built-in default prompt and `build_soap_prompt`.
//! - [`user_prompt`] — `build_user_prompt`, plus the `sanitize_prompt` helper.
//! - [`postprocess`] — markdown cleanup and section formatting on AI output.

use medical_core::types::settings::SoapTemplate;

mod postprocess;
mod prompt_template;
mod user_prompt;

pub use postprocess::postprocess_soap;
pub use prompt_template::{build_soap_prompt, default_soap_prompt};
pub use user_prompt::build_user_prompt;

/// Inputs to [`build_soap_prompt`].
#[derive(Debug, Clone)]
pub struct SoapPromptConfig {
    pub template: SoapTemplate,
    /// One of "ICD-9", "ICD-10", "both" (case-sensitive).
    pub icd_version: String,
    /// User-supplied override; empty string is treated as absent.
    pub custom_prompt: Option<String>,
}

impl Default for SoapPromptConfig {
    fn default() -> Self {
        Self {
            template: SoapTemplate::FollowUp,
            icd_version: "ICD-10".into(),
            custom_prompt: None,
        }
    }
}
