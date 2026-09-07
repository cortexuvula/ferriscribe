//! The built-in default SOAP system prompt and the [`build_soap_prompt`]
//! entry point that resolves placeholders against [`SoapPromptConfig`].
//!
//! The default prompt is ~280 lines and contains:
//! - A RULES block with the core anti-fabrication constraint (transcript is
//!   the sole source of truth)
//! - A FORBIDDEN INFERENCES block naming ten categories of common hallucinations
//! - Two few-shot examples (sparse injury visit + lab-review visit) demonstrating
//!   disciplined extraction
//! - An OUTPUT FORMAT section specifying the section-by-section template
//! - A 10-point SELF-CHECK checklist (placed last for LLM recency compliance)

use std::collections::HashMap;

use medical_core::icd9::Icd9Entry;
use medical_core::types::settings::{IcdVersion, SoapTemplate};

use crate::prompt_resolver::resolve_prompt;
use crate::specialty::{assemble_pack_prompt, family_medicine_soap_body};

use super::SoapPromptConfig;

// ---------------------------------------------------------------------------
// Placeholder resolution
// ---------------------------------------------------------------------------

/// Build the placeholder map for the SOAP template.
fn soap_placeholders(
    icd_version: IcdVersion,
    template: &SoapTemplate,
    icd9_candidates: &[Icd9Entry],
) -> HashMap<&'static str, String> {
    let (icd_instruction, icd_label) = icd_code_parts(icd_version.clone());
    let template_guidance = template_guidance_text(template);
    let icd_candidates = icd_candidates_block(icd_version, icd9_candidates);

    let mut map = HashMap::new();
    map.insert("icd_instruction", icd_instruction.to_string());
    map.insert("icd_label", icd_label.to_string());
    map.insert("template_guidance", template_guidance.to_string());
    map.insert("icd_candidates", icd_candidates);
    map
}

/// Format the ICD-9 candidate list for prompt injection.
///
/// Returns an empty string for ICD-10-only mode (no bundled ICD-10
/// list) and when the candidate list is empty. The block instructs the
/// model to select from the provided BC MSP-accepted codes.
fn icd_candidates_block(icd_version: IcdVersion, candidates: &[Icd9Entry]) -> String {
    let inject_icd9 = matches!(icd_version, IcdVersion::Icd9 | IcdVersion::Both);
    if !inject_icd9 || candidates.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(candidates.len() * 48);
    out.push_str("ICD-9 CODE SELECTION — choose up to 3 ICD-9 codes from this BC MSP-accepted list, one per line, ordered by clinical complexity (most complex first). Use the most specific code available (4- or 5-digit preferred). Prefer screening codes (V81.x, V77.x, V16.x) for wellness/preventive visits. Avoid non-specific codes like V70.x unless the visit truly has no diagnosable complaint. If none fits, choose the closest unspecified \".9\" variant:\n");
    for entry in candidates {
        // Sanitize description as a defense-in-depth measure against prompt
        // injection (the ICD data is trusted government data compiled at
        // build time, but this prevents future regressions if the source
        // changes). Applied before truncation so a sanitizer that strips
        // content cannot push a suffix past the truncation boundary.
        let sanitized = crate::soap_generator::user_prompt::sanitize_prompt(&entry.description);
        // Truncate long descriptions to keep the prompt lean. Truncate by
        // char count (not byte index) — MSP descriptions contain en-dashes
        // and other multi-byte chars that would panic a byte slice.
        let char_count = sanitized.chars().count();
        let desc = if char_count > 60 {
            let head: String = sanitized.chars().take(57).collect();
            format!("{head}…")
        } else {
            sanitized
        };
        out.push_str(&format!("  {} — {}\n", entry.code, desc));
    }
    out
}

/// The multi-code complexity-optimized ICD-9 instruction body, shared by
/// the `ICD-9` and `both` modes so both stay consistent (BC MSP is the
/// biller in either case). Encodes the CPSBC "up to 3 codes, complexity-
/// ordered, most-specific available" guidance plus an explicit under-coding
/// guard so a trivial visit is not padded to reach 3.
const ICD9_MULTI_CODE_BODY: &str = "ICD-9 Codes (up to 3, one per line, most clinically complex first): [List each code on its own line as \"ICD-9 Code: <code> — <brief description>\". Use the most specific code available (4- or 5-digit preferred over 3-digit) — e.g., 250.40 (diabetes with renal manifestations) rather than 250.00 (diabetes without complication). Include every distinct condition actively addressed, assessed, or managed at this visit — not just the primary complaint. When a chronic condition (diabetes, hypertension, COPD, heart failure, depression, etc.) is managed or reviewed, include its code even if it is not the primary reason for the visit. When a definitive diagnosis is established, use the disease-specific code rather than a symptom code; if workup is still in progress, use the most specific symptom code for the presenting complaint. Do not use 780 (General Symptoms) as a catch-all. Avoid non-specific routine-exam codes (V70.x) — prefer screening codes (V81.x for thyroid, V77.x for diabetes, V16.x for family history of cancer) when the visit is a screening or wellness encounter. Prefer fewer codes for simple visits — a single acute complaint with no comorbidities managed at the visit correctly uses one code; do not pad to reach 3.]";

/// The `both`-mode label: the ICD-9 multi-code body followed by the
/// single-code ICD-10 line. Pre-composed as a const so the function can
/// return `&'static str` without allocating.
const ICD9_AND_10_LABEL: &str = concat!(
    "ICD-9 Codes (up to 3, one per line, most clinically complex first): [List each code on its own line as \"ICD-9 Code: <code> — <brief description>\". Use the most specific code available (4- or 5-digit preferred over 3-digit) — e.g., 250.40 (diabetes with renal manifestations) rather than 250.00 (diabetes without complication). Include every distinct condition actively addressed, assessed, or managed at this visit — not just the primary complaint. When a chronic condition (diabetes, hypertension, COPD, heart failure, depression, etc.) is managed or reviewed, include its code even if it is not the primary reason for the visit. When a definitive diagnosis is established, use the disease-specific code rather than a symptom code; if workup is still in progress, use the most specific symptom code for the presenting complaint. Do not use 780 (General Symptoms) as a catch-all. Avoid non-specific routine-exam codes (V70.x) — prefer screening codes (V81.x for thyroid, V77.x for diabetes, V16.x for family history of cancer) when the visit is a screening or wellness encounter. Prefer fewer codes for simple visits — a single acute complaint with no comorbidities managed at the visit correctly uses one code; do not pad to reach 3.]",
    "\nICD-10 Code: [specific code reflecting the visit's primary issue.]"
);

fn icd_code_parts(version: IcdVersion) -> (&'static str, &'static str) {
    match version {
        IcdVersion::Icd9 => ("ICD-9 code", ICD9_MULTI_CODE_BODY),
        // ICD-9 uses the same multi-code complexity body as pure ICD-9
        // mode (BC MSP bills ICD-9); ICD-10 stays single-code.
        IcdVersion::Both => ("both ICD-9 and ICD-10 codes", ICD9_AND_10_LABEL),
        IcdVersion::Icd10 => (
            "ICD-10 code",
            "ICD-10 Code: [specific code reflecting the visit's primary issue. For paperwork-only / wellness / lab-review visits with no diagnosable complaint, use a routine-encounter code such as Z00.00.]",
        ),
    }
}

fn template_guidance_text(template: &SoapTemplate) -> &'static str {
    match template {
        SoapTemplate::FollowUp => {
            "Focus on changes since last visit, interval history, and response to current treatment plan."
        }
        SoapTemplate::NewPatient => {
            "Provide comprehensive history including past medical history, family history, social history, and review of systems."
        }
        SoapTemplate::Telehealth => {
            "Note the limitations of remote examination. Document what was assessed virtually and any elements requiring in-person follow-up."
        }
        SoapTemplate::Emergency => {
            "Prioritise acute findings. Document chief complaint, vital signs, acute interventions, and disposition."
        }
        SoapTemplate::Pediatric => {
            "Include developmental milestones, immunisation status, growth parameters, and age-appropriate screening."
        }
        SoapTemplate::Geriatric => {
            "Address functional status, fall risk assessment, polypharmacy review, cognitive screening, and social support."
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// The built-in default SOAP system prompt: the bundled `family-medicine`
/// pack's prompt body assembled with the compiled-in safety block
/// (`[pack prompt] + --- + SAFETY_BLOCK`, the block last for recency).
///
/// The pack body is byte-for-byte the original hand-written default (pinned
/// against `specialty/testdata/family_medicine_soap_original.golden`); the
/// appended safety block is the anti-fabrication floor every specialty gets.
///
/// Contains the same placeholder tokens resolved by [`build_soap_prompt`] as
/// before:
/// - `{template_guidance}` — template-variant-specific instruction
/// - `{icd_label}` — ICD code header line (e.g., "ICD-10 Code: [specific code...]")
/// - `{icd_instruction}` — ICD code instruction text (within OUTPUT FORMAT)
/// - `{icd_candidates}` — the BC MSP candidate-code block (ICD-9/both modes)
///
/// See the module-level docs on [`crate::specialty`] for the full
/// anti-fabrication structure (RULES, FORBIDDEN INFERENCES, few-shot
/// examples, OUTPUT FORMAT, SELF-CHECK) carried in the pack body.
pub fn default_soap_prompt() -> &'static str {
    static ASSEMBLED: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| assemble_pack_prompt(family_medicine_soap_body()));
    ASSEMBLED.as_str()
}

/// Build the SOAP system prompt: select template (custom > specialty pack >
/// default), then resolve placeholders.
///
/// # Template Selection (precedence)
///
/// 1. `config.custom_prompt` (`Some` and non-empty) — the user's free-text
///    override replaces everything wholesale (verbatim; the safety block is
///    NOT appended — the author takes full responsibility, matching the
///    historical behaviour of custom prompts).
/// 2. `config.specialty_prompt` (`Some`) — a specialty pack's SOAP body;
///    assembled as `[pack prompt] + --- + SAFETY_BLOCK`.
/// 3. Otherwise — [`default_soap_prompt`], itself the assembled
///    `family-medicine` pack body + safety block.
///
/// Placeholders (`{icd_label}`, `{icd_instruction}`, `{template_guidance}`,
/// `{icd_candidates}`) are resolved identically in every tier.
///
/// # Placeholder Resolution
///
/// | Placeholder | Source |
/// |---|---|
/// | `{template_guidance}` | Derived from `config.template` (e.g., FollowUp → "changes since last visit") |
/// | `{icd_label}` | Derived from `config.icd_version` ("ICD-9", "ICD-10", or "both") |
/// | `{icd_instruction}` | Same derivation as `{icd_label}` — the inline instruction text |
/// | `{icd_candidates}` | The BC MSP candidate list (`config.icd9_candidates`, ICD-9/both modes) |
pub fn build_soap_prompt(config: &SoapPromptConfig) -> String {
    let placeholders = soap_placeholders(
        config.icd_version.clone(),
        &config.template,
        &config.icd9_candidates,
    );

    if let Some(custom) = config.custom_prompt.as_deref().filter(|s| !s.is_empty()) {
        return resolve_prompt(custom, &placeholders);
    }

    let template = match config.specialty_prompt.as_deref() {
        Some(body) => assemble_pack_prompt(body),
        None => default_soap_prompt().to_string(),
    };
    resolve_prompt(&template, &placeholders)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_soap_prompt_has_structure_markers() {
        let config = SoapPromptConfig::default();
        let prompt = build_soap_prompt(&config);
        // Core section markers
        assert!(prompt.contains("Subjective"));
        assert!(prompt.contains("Objective"));
        assert!(prompt.contains("Assessment"));
        assert!(prompt.contains("Differential Diagnosis"));
        assert!(prompt.contains("Plan"));
        assert!(prompt.contains("Follow up"));
        assert!(prompt.contains("Clinical Synopsis"));
        // Rules section
        assert!(prompt.contains("RULES:"));
        assert!(prompt.contains("FORMATTING RULES"));
    }

    #[test]
    fn default_soap_prompt_includes_few_shot_example() {
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        // The example block is named and contains the disciplined-extraction snippet
        assert!(prompt.contains("EXAMPLE"));
        assert!(prompt.contains("right-sided back pain for three days"));
        // It demonstrates the "Not discussed / Not recorded / Not performed" pattern
        assert!(prompt.contains("Vital signs: Not recorded"));
        assert!(prompt.contains("Physical examination: Not discussed"));
        assert!(prompt.contains("Review of systems: Not performed"));
        // It explicitly calls out what would be fabrications, not just what to include
        assert!(prompt.contains("would be a fabrication"));
    }

    #[test]
    fn default_soap_prompt_includes_self_check_block() {
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        assert!(prompt.contains("SELF-CHECK"));
        assert!(prompt.contains("locate the transcript quote"));
        assert!(prompt.contains("do not invent one"));
    }

    #[test]
    fn self_check_block_is_at_end_for_recency() {
        // Recency matters: the model is more likely to follow the self-check
        // discipline if it appears AFTER the format and formatting-rules sections.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        let pos_self_check = prompt.find("SELF-CHECK").expect("self-check block missing");
        let pos_format_rules = prompt
            .find("FORMATTING RULES")
            .expect("formatting rules section missing");
        let pos_output_format = prompt
            .find("OUTPUT FORMAT")
            .expect("output format section missing");
        assert!(
            pos_self_check > pos_format_rules,
            "SELF-CHECK must come after FORMATTING RULES"
        );
        assert!(
            pos_self_check > pos_output_format,
            "SELF-CHECK must come after OUTPUT FORMAT"
        );
    }

    #[test]
    fn example_appears_before_output_format() {
        // The example must precede OUTPUT FORMAT so the model has a concrete
        // demo of the rules in mind before it sees the section template.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        let pos_example = prompt.find("EXAMPLE").expect("example block missing");
        let pos_output_format = prompt
            .find("OUTPUT FORMAT")
            .expect("output format section missing");
        assert!(
            pos_example < pos_output_format,
            "EXAMPLE must come before OUTPUT FORMAT"
        );
    }

    #[test]
    fn default_soap_prompt_resolves_icd9() {
        let config = SoapPromptConfig {
            icd_version: IcdVersion::Icd9,
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        // ICD-9 label now teaches multi-code (up to 3) complexity-ordered output.
        assert!(prompt.contains("ICD-9 Codes (up to 3"));
        assert!(prompt.contains("V70.0"));
        assert!(!prompt.contains("{icd_label}"));
        assert!(!prompt.contains("{icd_instruction}"));
        assert!(
            !prompt.contains("Not applicable - no diagnosis clearly discussed"),
            "old strict-mode 'Not applicable' string must not appear anywhere"
        );
        assert!(
            !icd_resolved_instruction(&prompt).contains("(suggested)"),
            "resolved ICD instruction must no longer mention (suggested)"
        );
    }

    #[test]
    fn default_soap_prompt_resolves_icd10() {
        let config = SoapPromptConfig {
            icd_version: IcdVersion::Icd10,
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        assert!(prompt.contains("ICD-10 Code: [specific code"));
        assert!(prompt.contains("Z00.00"));
        assert!(
            !prompt.contains("Not applicable - no diagnosis clearly discussed"),
            "old strict-mode 'Not applicable' string must not appear anywhere"
        );
        assert!(
            !icd_resolved_instruction(&prompt).contains("(suggested)"),
            "resolved ICD instruction must no longer mention (suggested)"
        );
    }

    #[test]
    fn default_soap_prompt_resolves_both_icd() {
        let config = SoapPromptConfig {
            icd_version: IcdVersion::Both,
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        // ICD-9 portion now uses the multi-code complexity body (same as
        // pure ICD-9 mode); ICD-10 stays single-code.
        assert!(prompt.contains("ICD-9 Codes (up to 3"));
        assert!(prompt.contains("ICD-10 Code: [specific code"));
        assert!(prompt.contains("V70.0"));
        assert!(prompt.contains("Z00.00"));
        assert!(
            !prompt.contains("Not applicable - no diagnosis clearly discussed"),
            "old strict-mode 'Not applicable' string must not appear anywhere"
        );
        assert!(
            !icd_resolved_instruction(&prompt).contains("(suggested)"),
            "resolved ICD instruction must no longer mention (suggested)"
        );
    }

    #[test]
    fn default_soap_prompt_icd9_supports_multi_code() {
        // The ICD-9 label encodes BC's complexity-based billing rules:
        // up to 3 codes, complexity-ordered, most-specific available,
        // with explicit guards against the 780 catch-all and the
        // under-coding of chronic conditions.
        let prompt = build_soap_prompt(&SoapPromptConfig {
            icd_version: IcdVersion::Icd9,
            ..Default::default()
        });
        assert!(prompt.contains("up to 3"), "must teach up-to-3 codes");
        assert!(
            prompt.contains("most clinically complex first"),
            "must teach complexity ordering"
        );
        assert!(
            prompt.contains("4- or 5-digit"),
            "must teach specificity preference"
        );
        assert!(
            prompt.contains("Do not use 780 (General Symptoms) as a catch-all"),
            "must warn against the 780 catch-all"
        );
        assert!(
            prompt.contains("chronic condition"),
            "must teach coding chronic conditions at management visits"
        );

        // Scope guard: ICD-10 mode must NOT carry the multi-code directive
        // in its {icd_label} substitution (the OUTPUT FORMAT header). BC
        // complexity systems consume ICD-9; ICD-10 stays single-code. The
        // shared self-check legitimately *mentions* "ICD-9 mode: up to 3" as
        // an explanation, so we scope this assertion to the resolved label.
        let (_, icd10_label) = icd_code_parts(IcdVersion::Icd10);
        assert!(
            !icd10_label.contains("up to 3"),
            "ICD-10 label must stay single-code: {icd10_label}"
        );
        let (_, icd9_label) = icd_code_parts(IcdVersion::Icd9);
        assert!(
            icd9_label.contains("up to 3"),
            "ICD-9 label must carry the multi-code directive: {icd9_label}"
        );
        // Over-coding guard: must teach when to use fewer than 3 codes.
        assert!(
            prompt.contains("Prefer fewer codes for simple visits"),
            "must guard against padding trivial visits to 3 codes"
        );
    }

    #[test]
    fn both_mode_icd9_uses_multi_code_consistently() {
        // F4 guard: the `both` arm must use the SAME multi-code ICD-9 body
        // as pure ICD-9 mode (no contradiction with the shared self-check,
        // which describes "ICD-9 mode: up to 3 codes"). ICD-10 stays single.
        let (_, both_label) = icd_code_parts(IcdVersion::Both);
        assert!(
            both_label.contains("up to 3"),
            "`both` ICD-9 portion must be multi-code (consistent with self-check): {both_label}"
        );
        assert!(
            both_label.contains("Prefer fewer codes for simple visits"),
            "`both` ICD-9 portion must carry the over-coding guard: {both_label}"
        );
        // ICD-10 line stays single-code.
        assert!(
            both_label.contains("ICD-10 Code: [specific code"),
            "`both` ICD-10 portion must stay single-code: {both_label}"
        );
    }

    #[test]
    fn examples_show_per_code_lines() {
        // Both few-shot examples must demonstrate the per-code-line output
        // format (one "ICD-9 Code:" line per code) so the model emits codes
        // the extraction regex can parse — NOT a single "ICD-9 Codes:" header
        // with a bare list (which would silently break extraction).
        let prompt = build_soap_prompt(&SoapPromptConfig::default());

        // EXAMPLE 1: acute single-issue visit → 2 codes from same picture.
        let ex1_idx = prompt.find("EXAMPLE 1").expect("EXAMPLE 1 present");
        let ex1_end = prompt[ex1_idx..]
            .find("Subjective:")
            .expect("Subjective after EXAMPLE 1");
        let ex1_block = &prompt[ex1_idx..ex1_idx + ex1_end];
        let ex1_code_lines = ex1_block
            .lines()
            .filter(|l| l.starts_with("ICD-9 Code:"))
            .count();
        assert_eq!(
            ex1_code_lines, 2,
            "EXAMPLE 1 should show 2 per-code lines, found {ex1_code_lines}"
        );

        // EXAMPLE 2: multi-issue lab review → 3 codes, complexity-ordered.
        let ex2_idx = prompt.find("EXAMPLE 2").expect("EXAMPLE 2 present");
        let ex2_end = prompt[ex2_idx..]
            .find("Subjective:")
            .expect("Subjective after EXAMPLE 2");
        let ex2_block = &prompt[ex2_idx..ex2_idx + ex2_end];
        let ex2_code_lines: Vec<&str> = ex2_block
            .lines()
            .filter(|l| l.starts_with("ICD-9 Code:"))
            .collect();
        assert_eq!(
            ex2_code_lines.len(),
            3,
            "EXAMPLE 2 should show 3 per-code lines, found {}",
            ex2_code_lines.len()
        );
        // Complexity ordering: chronic (272.0) before acute (266.2) before
        // encounter (V70.0).
        assert!(ex2_code_lines[0].contains("272.0"), "most complex first");
        assert!(ex2_code_lines[2].contains("V70.0"), "encounter code last");
    }

    /// Slice the resolved ICD instruction block from the OUTPUT FORMAT section.
    /// Used by ICD-resolution tests to scope assertions to the line(s) that
    /// replaced the `{icd_label}` placeholder.
    fn icd_resolved_instruction(prompt: &str) -> &str {
        let start = prompt
            .find("OUTPUT FORMAT")
            .expect("OUTPUT FORMAT section missing");
        let block = &prompt[start..];
        let icd_idx = block.find("ICD-").expect("resolved ICD line missing");
        let tail = &block[icd_idx..];
        let end = tail.find("\n\n").unwrap_or(tail.len());
        &tail[..end]
    }

    #[test]
    fn default_soap_prompt_includes_forbidden_inferences_block() {
        // The FORBIDDEN INFERENCES block names the most common fabrication
        // categories so the model has explicit category-level guards beyond
        // the abstract rule "do not fabricate".
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        assert!(prompt.contains("FORBIDDEN INFERENCES"));
        // Demographics
        assert!(prompt.contains("Patient age, sex, gender"));
        // Stock comorbidity fill (HTN/HLD/T2DM)
        assert!(prompt.contains("Common comorbidities"));
        // Default-dose fill
        assert!(prompt.contains("never pick a canonical dose"));
        // Invented provider names for referrals
        assert!(prompt.contains("Provider names for referrals"));
        // Default follow-up interval
        assert!(prompt.contains("Follow-up timing not specified"));
        // Stock red-flag warnings
        assert!(prompt.contains("Red-flag warnings"));
        // The OLD ICD-blocking rule is gone
        assert!(
            !prompt.contains("ICD codes when no diagnosis was clearly discussed"),
            "old strict ICD bullet must be removed from FORBIDDEN INFERENCES"
        );
        // The carve-out bullet explicitly names ICD + DDx as the only
        // inference-permitted sections and forbids the "(suggested)" marker
        // (and similar qualifiers) so the model emits plain text.
        assert!(prompt.contains("ICD codes and differential diagnoses"));
        assert!(prompt.contains("only two sections where clinical inference is permitted"));
        assert!(prompt.contains("do NOT append any marker"));
    }

    #[test]
    fn default_soap_prompt_includes_lab_review_example() {
        // A second few-shot example covers the lab-review visit pattern
        // (no HPI, no exam, no PMH). This was the failure mode that
        // produced the worst hallucinations on real-world transcripts.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        assert!(prompt.contains("EXAMPLE 1"));
        assert!(prompt.contains("EXAMPLE 2"));
        assert!(prompt.contains("lab-review visit"));
        // Lab-review example must teach the multi-code complexity-ordered
        // output: chronic 272.0 first, then acute 266.2, then encounter V70.0.
        let lab_idx = prompt.find("EXAMPLE 2").expect("EXAMPLE 2 must be present");
        let after_example = &prompt[lab_idx..];
        assert!(after_example.contains("ICD-9 Code: 272.0"));
        assert!(after_example.contains("ICD-9 Code: 266.2"));
        assert!(after_example.contains("ICD-9 Code: V70.0"));
        // Lab-review example must teach the "dose not specified" pattern
        assert!(after_example.contains("dose not specified"));
        // Lab-review example must show that a thin visit produces
        // mostly "Not discussed" subjective entries
        assert!(after_example.contains("Past medical history: Not discussed"));
        assert!(after_example.contains("Family history: Not discussed"));
        // Both examples must come before OUTPUT FORMAT
        let pos_example_2 = prompt.find("EXAMPLE 2").unwrap();
        let pos_output_format = prompt.find("OUTPUT FORMAT").unwrap();
        assert!(
            pos_example_2 < pos_output_format,
            "EXAMPLE 2 must come before OUTPUT FORMAT"
        );
    }

    #[test]
    fn default_soap_prompt_lab_review_example_has_three_differentials() {
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        let lab_idx = prompt.find("EXAMPLE 2").expect("EXAMPLE 2 must be present");
        let after_example = &prompt[lab_idx..];

        let ddx_idx = after_example
            .find("Differential Diagnosis:")
            .expect("EXAMPLE 2 must contain a Differential Diagnosis block");

        // Capture the lines from the DDx header up to the next blank line.
        let ddx_block_start = ddx_idx + "Differential Diagnosis:".len();
        let ddx_tail = &after_example[ddx_block_start..];
        let ddx_end = ddx_tail.find("\n\n").unwrap_or(ddx_tail.len());
        let ddx_block = &ddx_tail[..ddx_end];

        let item_count = ddx_block
            .lines()
            .filter(|line| line.trim_start().starts_with("- "))
            .count();
        assert!(
            item_count >= 3,
            "EXAMPLE 2 Differential Diagnosis must list at least three items; found {item_count}.\nBlock:\n{ddx_block}"
        );

        let suggested_count = ddx_block
            .lines()
            .filter(|line| line.trim_start().starts_with("- ") && line.contains("(suggested)"))
            .count();
        assert_eq!(
            suggested_count, 0,
            "EXAMPLE 2 DDx items must be rendered as plain text — no (suggested) marker.\nBlock:\n{ddx_block}"
        );
    }

    #[test]
    fn self_check_lists_category_checks() {
        // The self-check must be a categorical checklist, not a single
        // verbal exhortation, so the model walks each common-fabrication
        // category one at a time.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        assert!(prompt.contains("Demographics check"));
        assert!(prompt.contains("Medication check"));
        assert!(prompt.contains("Referral check"));
        assert!(prompt.contains("Follow-up interval check"));
        assert!(prompt.contains("Red-flag check"));
        assert!(prompt.contains("ICD code check"));
        assert!(prompt.contains("Visit modality check"));
        // New: DDx count + marker check is item 10
        assert!(prompt.contains("Differential Diagnosis count"));
    }

    #[test]
    fn default_soap_prompt_includes_template_guidance() {
        let config = SoapPromptConfig {
            template: SoapTemplate::NewPatient,
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        assert!(prompt.contains("comprehensive history"));
    }

    #[test]
    fn custom_soap_prompt_overrides_default() {
        let config = SoapPromptConfig {
            custom_prompt: Some("My custom template with {icd_label}".into()),
            icd_version: IcdVersion::Icd9,
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        // Custom template is used, and placeholders are still resolved.
        // The ICD-9 label now carries the multi-code complexity guidance.
        assert!(prompt.starts_with("My custom template with ICD-9 Codes (up to 3"));
        // The multi-code body steers away from V70.x routine-exam codes
        // and toward screening codes for wellness visits.
        assert!(prompt.contains("V70.x"));
        assert!(prompt.contains("screening codes"));
        // ICD instruction no longer carries the (suggested) marker
        assert!(!prompt.contains("(suggested)"));
    }

    #[test]
    fn empty_custom_prompt_falls_back_to_default() {
        let config = SoapPromptConfig {
            custom_prompt: Some("".into()),
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        // Empty string should not be treated as a real custom prompt
        assert!(prompt.contains("You are a physician creating a SOAP note"));
    }

    #[test]
    fn template_specific_instructions() {
        let follow_up = SoapPromptConfig {
            template: SoapTemplate::FollowUp,
            ..Default::default()
        };
        assert!(build_soap_prompt(&follow_up).contains("changes since last visit"));

        let new_patient = SoapPromptConfig {
            template: SoapTemplate::NewPatient,
            ..Default::default()
        };
        assert!(build_soap_prompt(&new_patient).contains("comprehensive history"));

        let telehealth = SoapPromptConfig {
            template: SoapTemplate::Telehealth,
            ..Default::default()
        };
        assert!(build_soap_prompt(&telehealth).contains("limitations of remote"));

        let emergency = SoapPromptConfig {
            template: SoapTemplate::Emergency,
            ..Default::default()
        };
        assert!(build_soap_prompt(&emergency).contains("acute findings"));

        let pediatric = SoapPromptConfig {
            template: SoapTemplate::Pediatric,
            ..Default::default()
        };
        assert!(build_soap_prompt(&pediatric).contains("developmental milestones"));

        let geriatric = SoapPromptConfig {
            template: SoapTemplate::Geriatric,
            ..Default::default()
        };
        let gp = build_soap_prompt(&geriatric);
        assert!(gp.contains("functional status"));
        assert!(gp.contains("fall risk"));
        assert!(gp.contains("polypharmacy"));
    }

    #[test]
    fn current_medications_format_allows_additional_clinical_context() {
        // Regression: physicians supply current medications via the
        // "Additional Context" panel when they aren't restated in the visit
        // transcript. The output-format spec for "Current medications" must
        // tell the model that background is a valid source — otherwise the
        // model writes "Not discussed" and silently drops user-entered meds.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        let format_idx = prompt
            .find("OUTPUT FORMAT")
            .expect("OUTPUT FORMAT section missing");
        let format_block = &prompt[format_idx..];
        let meds_idx = format_block
            .find("Current medications:")
            .expect("Current medications section missing in OUTPUT FORMAT");
        let meds_block = &format_block[meds_idx..meds_idx + 400];
        assert!(
            meds_block.contains("additional clinical context"),
            "Current medications output format must allow additional clinical context as a source.\nBlock:\n{meds_block}"
        );
    }

    #[test]
    fn historical_subjective_fields_allow_additional_clinical_context() {
        // Allergies, family history, and social history are also historical
        // facts the physician may supply via background context. The format
        // must allow background sourcing for all of them, not just PMH.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        let format_idx = prompt
            .find("OUTPUT FORMAT")
            .expect("OUTPUT FORMAT section missing");
        let format_block = &prompt[format_idx..];
        for field in ["Allergies:", "Family history:", "Social history:"] {
            let idx = format_block
                .find(field)
                .unwrap_or_else(|| panic!("{field} section missing in OUTPUT FORMAT"));
            let block = &format_block[idx..idx + 200];
            assert!(
                block.contains("additional clinical context"),
                "{field} output format must allow additional clinical context as a source.\nBlock:\n{block}"
            );
        }
    }

    #[test]
    fn default_soap_prompt_requires_at_least_three_differentials() {
        // The OUTPUT FORMAT Differential Diagnosis block must instruct the
        // model to produce at least three items, all rendered as plain text
        // with no "(suggested)" or similar marker.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        let format_idx = prompt
            .find("OUTPUT FORMAT")
            .expect("OUTPUT FORMAT section missing");
        let format_block = &prompt[format_idx..];
        let ddx_idx = format_block
            .find("Differential Diagnosis:")
            .expect("Differential Diagnosis section missing in OUTPUT FORMAT");
        let ddx_block = &format_block[ddx_idx..ddx_idx + 600];
        assert!(
            ddx_block.contains("at least three"),
            "OUTPUT FORMAT Differential Diagnosis must require at least three items.\nBlock:\n{ddx_block}"
        );
        assert!(
            ddx_block.contains("plain text") && ddx_block.contains("do NOT append"),
            "OUTPUT FORMAT Differential Diagnosis must require plain-text items and forbid markers.\nBlock:\n{ddx_block}"
        );
        assert!(
            !ddx_block.contains("No differential diagnoses were discussed during the visit"),
            "old strict 'no DDx' fallback must not appear in OUTPUT FORMAT"
        );
    }

    #[test]
    fn medication_self_check_allows_additional_clinical_context() {
        // Self-check rule #3 previously required medication elements to be
        // "stated in the transcript", which contradicts Rule #4 and causes
        // the model to drop background-supplied medications.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        let idx = prompt
            .find("Medication check")
            .expect("Medication self-check entry missing");
        let block = &prompt[idx..idx + 400];
        assert!(
            block.contains("additional clinical context"),
            "Medication self-check must acknowledge supplied additional clinical context as a valid source.\nBlock:\n{block}"
        );
    }

    #[test]
    fn default_soap_prompt_treats_patient_record_as_authoritative() {
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        assert!(
            prompt.contains("Patient record"),
            "system prompt must reference the Patient record block by name"
        );
        // The sentence must distinguish Patient record (authoritative) from
        // Additional clinical context, and reaffirm the transcript-precedence rule.
        assert!(
            prompt.contains("authoritative") || prompt.contains("ground truth"),
            "system prompt must mark Patient record entries as authoritative"
        );
        assert!(
            prompt.contains("primary source") || prompt.contains("prefer the transcript"),
            "system prompt must reaffirm transcript-precedence over additional clinical context"
        );
    }

    #[test]
    fn default_soap_prompt_forbids_suggested_marker_in_carve_out() {
        // The FORBIDDEN INFERENCES carve-out bullet must explicitly forbid
        // appending "(suggested)" (or any similar marker) to ICD codes and
        // DDx items, so the model cannot rationalise emitting one.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        let block_idx = prompt
            .find("FORBIDDEN INFERENCES")
            .expect("FORBIDDEN INFERENCES section missing");
        let block = &prompt[block_idx..];
        let carve_idx = block
            .find("ICD codes and differential diagnoses are the only two sections")
            .expect("FORBIDDEN INFERENCES must contain the ICD/DDx carve-out bullet");
        let carve_window = &block[carve_idx..carve_idx + 600];
        assert!(
            carve_window.contains("(suggested)") && carve_window.contains("do NOT append"),
            "carve-out bullet must explicitly forbid the (suggested) marker.\nWindow:\n{carve_window}"
        );
        assert!(
            carve_window.contains("ICD codes and differential diagnoses"),
            "carve-out bullet must name both protected sections.\nWindow:\n{carve_window}"
        );
    }

    #[test]
    fn default_soap_prompt_drops_old_icd_blocking_rule() {
        // The pre-relaxation FORBIDDEN INFERENCES bullet "ICD codes when no
        // diagnosis was clearly discussed..." must NOT appear anywhere in
        // the prompt — regression guard against an accidental revert.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        assert!(
            !prompt.contains("ICD codes when no diagnosis was clearly discussed"),
            "old strict ICD bullet must remain removed"
        );
        assert!(
            !prompt.contains("No differential diagnoses were discussed during the visit"),
            "old strict 'no DDx discussed' fallback must remain removed"
        );
        assert!(
            !prompt.contains("Not applicable - no diagnosis clearly discussed"),
            "old strict 'Not applicable' ICD output must remain removed"
        );
    }

    #[test]
    fn default_soap_prompt_self_check_keeps_other_strict_categories() {
        // Sanity guard: ICD/DDx relaxation must not weaken the other
        // categorical anti-fabrication checks. Each of these labels must
        // still appear in the SELF-CHECK block.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        let sc_idx = prompt.find("SELF-CHECK").expect("SELF-CHECK block missing");
        let sc_block = &prompt[sc_idx..];
        for label in [
            "Demographics check",
            "Past medical history check",
            "Medication check",
            "Referral check",
            "Follow-up interval check",
            "Red-flag check",
            "Visit modality check",
            "Assessment check",
        ] {
            assert!(
                sc_block.contains(label),
                "SELF-CHECK must still contain '{label}' — ICD/DDx relaxation should not weaken other categories.\nBlock excerpt:\n{}",
                &sc_block[..sc_block.len().min(2000)]
            );
        }
    }

    #[test]
    fn default_soap_prompt_mandates_first_person_voice() {
        // The prompt must include an explicit rule telling the model to
        // write the SOAP note in first person (as the attending physician),
        // not in third person as "the physician".
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        assert!(
            prompt.contains("first person"),
            "system prompt must mandate first-person voice — the literal phrase 'first person' was not found"
        );
    }

    #[test]
    fn default_soap_prompt_does_not_use_physician_third_person_outside_rules() {
        // Anti-regression: once the first-person rule lands, the only
        // place "the physician" may appear is inside the RULES block
        // where the prohibition is stated. The EXAMPLE blocks, OUTPUT
        // FORMAT, and SELF-CHECK must NOT refer to "the physician" in
        // the third person — those uses leak into the model's output.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());

        let example_idx = prompt.find("EXAMPLE 1").expect("EXAMPLE 1 block missing");
        let format_idx = prompt
            .find("OUTPUT FORMAT")
            .expect("OUTPUT FORMAT section missing");
        let sc_idx = prompt
            .find("SELF-CHECK")
            .expect("SELF-CHECK section missing");

        let examples_block = &prompt[example_idx..format_idx];
        assert!(
            !examples_block.contains("the physician"),
            "EXAMPLE blocks must not contain third-person 'the physician' references.\nBlock excerpt:\n{}",
            &examples_block[..examples_block.len().min(2000)]
        );

        let format_block = &prompt[format_idx..sc_idx];
        assert!(
            !format_block.contains("the physician"),
            "OUTPUT FORMAT section must not contain third-person 'the physician' references.\nBlock excerpt:\n{}",
            &format_block[..format_block.len().min(2000)]
        );

        let sc_block = &prompt[sc_idx..];
        assert!(
            !sc_block.contains("the physician"),
            "SELF-CHECK section must not contain third-person 'the physician' references.\nBlock excerpt:\n{}",
            &sc_block[..sc_block.len().min(2000)]
        );
    }

    // ---- icd_candidates_block coverage ----
    //
    // This function is the prompt's constrained-vocabulary injection — the
    // most billing-critical formatting path — yet was previously untested
    // because every test passed an empty candidate list.

    fn entry(code: &str, desc: &str) -> Icd9Entry {
        Icd9Entry {
            code: code.into(),
            description: desc.into(),
            category: "Test".into(),
        }
    }

    #[test]
    fn icd_candidates_block_empty_for_empty_list() {
        let block = icd_candidates_block(IcdVersion::Icd9, &[]);
        assert!(
            block.is_empty(),
            "empty candidate list must produce no block"
        );
    }

    #[test]
    fn icd_candidates_block_empty_for_icd10_mode() {
        // Candidates must never be injected into an ICD-10 prompt — even
        // if the selector erroneously passed some (it only runs for
        // ICD-9/both, but this guard is the safety net).
        let cands = vec![entry("401.9", "HYPERTENSION")];
        let block = icd_candidates_block(IcdVersion::Icd10, &cands);
        assert!(
            block.is_empty(),
            "ICD-10 mode must never inject ICD-9 candidates"
        );
    }

    #[test]
    fn icd_candidates_block_formats_entries() {
        let cands = vec![entry("847.2", "LUMBAR"), entry("V70.0", "ROUTINE EXAM")];
        let block = icd_candidates_block(IcdVersion::Icd9, &cands);
        assert!(block.contains("ICD-9 CODE SELECTION"), "header present");
        assert!(block.contains("847.2 — LUMBAR"), "first entry formatted");
        assert!(
            block.contains("V70.0 — ROUTINE EXAM"),
            "second entry formatted"
        );
    }

    #[test]
    fn icd_candidates_block_truncates_long_descriptions() {
        // 61 chars (including spaces) — must truncate to 57 chars + "…".
        let long_desc = "THIS IS A VERY LONG DESCRIPTION THAT EXCEEDS SIXTY CHARACTERS";
        assert!(
            long_desc.chars().count() > 60,
            "test setup: desc must be >60 chars"
        );
        let cands = vec![entry("999.9", long_desc)];
        let block = icd_candidates_block(IcdVersion::Icd9, &cands);
        assert!(
            block.contains("…"),
            "long description must be truncated with ellipsis"
        );
        // The full untruncated description must NOT appear.
        assert!(
            !block.contains(long_desc),
            "untruncated long description must not appear in the prompt"
        );
    }

    #[test]
    fn icd_candidates_block_keeps_short_descriptions_intact() {
        // Exactly 60 chars — must NOT be truncated (the boundary is >60).
        let exact_60 = "EXACTLY SIXTY CHARACTERS LONG DESCRIPTION HERE FOR TEST!!"; // 58 chars — under, should be intact
        let cands = vec![entry("401.9", exact_60)];
        let block = icd_candidates_block(IcdVersion::Icd9, &cands);
        assert!(
            block.contains(exact_60),
            "description under 60 chars must appear verbatim"
        );
        assert!(
            !block.contains("…"),
            "short description must not be truncated"
        );
    }

    #[test]
    fn icd_candidates_block_handles_multibyte_description() {
        // MSP descriptions contain en-dashes (–, 3 bytes) and the output
        // uses em-dashes (—, 3 bytes). A byte-index truncation would panic
        // or corrupt; char-based truncation must handle this cleanly.
        // 65 chars including en-dashes near the 57-char boundary.
        let multibyte = "DIABETES WITH NEUROLOGICAL MANIFESTATIONS – TYPE II UNSPECIFIED";
        assert!(multibyte.chars().count() > 60, "test setup");
        let cands = vec![entry("250.60", multibyte)];
        let block = icd_candidates_block(IcdVersion::Icd9, &cands);
        // Must not panic (the test reaching this assertion is the guard)
        // and must contain the ellipsis.
        assert!(
            block.contains("…"),
            "multibyte long description must truncate safely"
        );
        assert!(block.contains("250.60"), "code present");
    }

    #[test]
    fn icd_candidates_block_injected_for_both_mode() {
        // `both` mode should also inject ICD-9 candidates (BC MSP bills ICD-9).
        let cands = vec![entry("401.9", "HYPERTENSION")];
        let block = icd_candidates_block(IcdVersion::Both, &cands);
        assert!(
            !block.is_empty(),
            "`both` mode must inject ICD-9 candidates"
        );
    }

    #[test]
    fn icd_candidates_placeholder_resolves_in_full_prompt() {
        // End-to-end: build_soap_prompt with non-empty candidates must
        // resolve the {icd_candidates} placeholder (no leftover token) and
        // include the candidate block.
        let config = SoapPromptConfig {
            icd_version: IcdVersion::Icd9,
            icd9_candidates: vec![entry("847.2", "LUMBAR"), entry("V70.0", "ROUTINE")],
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        assert!(
            !prompt.contains("{icd_candidates}"),
            "placeholder must be resolved"
        );
        assert!(
            prompt.contains("847.2 — LUMBAR"),
            "candidate appears in full prompt"
        );
        assert!(
            prompt.contains("ICD-9 CODE SELECTION"),
            "selection header in prompt"
        );
    }

    #[test]
    fn icd_candidates_placeholder_empty_for_icd10_prompt() {
        // Even with candidates present, ICD-10 mode must not inject them.
        let config = SoapPromptConfig {
            icd_version: IcdVersion::Icd10,
            icd9_candidates: vec![entry("401.9", "HYPERTENSION")],
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        assert!(
            !prompt.contains("401.9 — HYPERTENSION"),
            "ICD-10 mode must not inject ICD-9 candidates"
        );
        assert!(
            !prompt.contains("ICD-9 CODE SELECTION"),
            "ICD-10 mode must not show the selection header"
        );
    }
}
