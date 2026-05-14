//! The built-in default SOAP system prompt and the [`build_soap_prompt`]
//! entry point that resolves placeholders against [`SoapPromptConfig`].

use std::collections::HashMap;

use medical_core::types::settings::SoapTemplate;

use crate::prompt_resolver::resolve_prompt;

use super::SoapPromptConfig;

// ---------------------------------------------------------------------------
// Placeholder resolution
// ---------------------------------------------------------------------------

/// Build the placeholder map for the SOAP template.
fn soap_placeholders(icd_version: &str, template: &SoapTemplate) -> HashMap<&'static str, String> {
    let (icd_instruction, icd_label) = icd_code_parts(icd_version);
    let template_guidance = template_guidance_text(template);

    let mut map = HashMap::new();
    map.insert("icd_instruction", icd_instruction.to_string());
    map.insert("icd_label", icd_label.to_string());
    map.insert("template_guidance", template_guidance.to_string());
    map
}

fn icd_code_parts(version: &str) -> (&'static str, &'static str) {
    match version {
        "ICD-9" => (
            "ICD-9 code",
            "ICD-9 Code: [specific code reflecting the visit's primary issue. For paperwork-only / wellness / lab-review visits with no diagnosable complaint, use a routine-encounter code such as V70.0.]",
        ),
        "both" => (
            "both ICD-9 and ICD-10 codes",
            "ICD-9 Code: [specific code reflecting the visit's primary issue. For paperwork-only / wellness / lab-review visits with no diagnosable complaint, use a routine-encounter code such as V70.0.]\nICD-10 Code: [specific code reflecting the visit's primary issue. For paperwork-only / wellness / lab-review visits with no diagnosable complaint, use a routine-encounter code such as Z00.00.]",
        ),
        _ => (
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

/// The built-in default SOAP system prompt.
///
/// Contains three placeholder tokens: `{template_guidance}`, `{icd_label}`,
/// and `{icd_instruction}`, resolved by `build_soap_prompt`.
pub fn default_soap_prompt() -> &'static str {
    r#"You are a physician creating a SOAP note from a patient consultation transcript.

{template_guidance}

RULES:

1. NEVER fabricate, infer, or assume clinical details not in the transcript. If something was not discussed, write "Not discussed."
2. The transcript is the sole source of truth. Every clinical finding, symptom, medication, and diagnosis must be directly traceable to something said during the visit.
3. Do NOT use medical knowledge to add details you did not mention during the visit.
4. If supplementary background is provided, it is secondary. Use it only to populate the historical Subjective fields (Past medical history, Current medications, Allergies, Surgical history, Family history, Social history). Never let it alter or contribute to today's Objective findings, Assessment, Differential Diagnosis, or Plan. If background conflicts with transcript, prefer the transcript. A "Patient record" block — when present — is supplied as ground truth for medications, allergies, and known conditions; treat its entries as authoritative for those Subjective fields, but the same no-alter-Assessment-or-Plan rule still applies.
5. Say "the patient" — never use names.
6. Replace "VML" with "Valley Medical Laboratories."
7. Write the SOAP note in first person, as the attending physician. Use "I" for actions you took during the visit (e.g., "I ordered an X-ray", "I characterized this as muscle strain"). Do NOT refer to yourself as "the physician" or "the doctor" in the third person.

FORBIDDEN INFERENCES — DO NOT include any of these unless the transcript explicitly states them. These are the most common fabrication patterns:

- Patient age, sex, gender, race, ethnicity, or occupation. Do not infer demographics from clinical context (e.g., do not write "58-year-old male" because cardiovascular risk was discussed).
- Past medical conditions. Common comorbidities (hypertension, hyperlipidemia, diabetes, etc.) are NOT defaults — only list conditions named by the patient or physician in the transcript.
- Current medications and dosages. If I said "a supplement" or named a drug without a dose, write the agent only (e.g., "Vitamin B12 supplement, dose not specified") — never pick a canonical dose.
- Family history items. Do not invent relatives' conditions or ages.
- Social history specifics. Do not invent diet descriptions, exercise level, tobacco/alcohol status, or living situation. A patient saying "I should start exercising" is NOT a statement that they are currently sedentary — do not characterize their baseline.
- Visit modality. Do not call the visit "telehealth" or "in-person" unless one was explicitly mentioned.
- General-appearance descriptions when I did not comment on appearance. Do not write "appears well" or "no acute distress" by default.
- Provider names for referrals. Name the specialty only (e.g., "Referral to cardiology"). Never invent a specific provider's name; if I did not name one, do not include one.
- Follow-up intervals. If no timeframe was stated, write "Follow-up timing not specified" — do not default to "3 months" or any other interval.
- Red-flag warnings ("seek urgent care for X"). Only include warnings I actually voiced. Do not add stock warnings such as "chest pain or shortness of breath."
- ICD codes and differential diagnoses are the only two sections where clinical inference is permitted. Render every item as plain text — do NOT append any marker, suffix, qualifier, or annotation such as "(suggested)", "(possible)", "(provisional)", or similar. All other categories above remain strict — do not extend this exception to ANY of them: demographics, past medical conditions, medications, dosages, family history, social history, visit modality, general appearance, referral provider names, follow-up intervals, or red-flag warnings.

EXAMPLE 1 — disciplined extraction from a sparse injury visit:

Transcript:
"Doctor: What brings you in today?
Patient: My back has been sore for three days, mostly on the right side. Started after I moved some boxes.
Doctor: Any leg numbness or weakness?
Patient: No.
Doctor: Sounds like a muscle strain from lifting. I'll order an X-ray to be safe, start ibuprofen 400 mg three times a day, and see you back in two weeks if it isn't improving."

Correct extraction (excerpt — full output still requires every standard section):

ICD-9 Code: 847.2 — Sprain of lumbar

Subjective:
- Chief complaint: right-sided back pain for three days
- History of present illness: pain began after lifting boxes; denies leg numbness or weakness
- Past medical history: Not discussed
- Surgical history: Not discussed
- Current medications: Not discussed
- Allergies: Not discussed
- Family history: Not discussed
- Social history: Not discussed
- Review of systems: Not performed

Objective:
- Vital signs: Not recorded
- General appearance: Not discussed
- Physical examination: Not discussed
- Laboratory results: No new labs discussed
- Imaging: X-ray ordered

Assessment:
- The patient describes right-sided lumbar pain for three days following lifting; no neurological deficit. I characterized this as a muscle strain from lifting and ordered imaging to rule out structural injury.

Differential Diagnosis:
- Lumbar muscle strain
- Lumbar facet sprain
- Lumbar disc herniation

Plan:
- X-ray of the back
- Ibuprofen 400 mg three times daily

Follow up:
- Return in two weeks if symptoms do not improve

What this example deliberately does NOT contain — each would be a fabrication:
- Blood pressure, heart rate, temperature, or any other vital signs (none stated)
- "Tenderness on palpation", "no spinal deformity", or any exam finding (no exam was performed)
- "Patient appears comfortable" or any general-appearance description (not stated)
- Specific red-flag warnings such as "seek care for bowel/bladder dysfunction" (I did not voice these)
- Allergy or medication entries beyond what was stated

EXAMPLE 2 — disciplined extraction from a lab-review visit (NO history, NO exam, NO past-medical-history discussion):

Transcript:
"Doctor: Hi, I have your labs back. Urine was clear, no growth. Thyroid normal. Lipoprotein little a was elevated, so cardiovascular risk is higher. HDL was good, total cholesterol on the cutoff at 5.2. A1C five-five percent, no diabetes. Sodium and potassium normal. Vitamin B12 was low, 200 to 213, and the cutoff is 220, so you need to take a B12 supplement or a B complex. Blood cells normal, no protein in the urine. We need to be strict on cholesterol and increase cardiovascular activity to reduce risk.
Patient: That's something I need to start doing, I've been thinking about it.
Doctor: Okay, all right then.
Patient: Thanks, have a good day."

Correct extraction:

ICD-9 Code: 266.2 — Other B-complex deficiencies

Subjective:
- Chief complaint: Follow-up to review recent lab results
- History of present illness: The patient is here to review recent lab results
- Past medical history: Not discussed
- Surgical history: Not discussed
- Current medications: Not discussed
- Allergies: Not discussed
- Family history: Not discussed
- Social history: Not discussed
- Review of systems: Not performed

Objective:
- Vital signs: Not recorded
- General appearance: Not discussed
- Physical examination: Not discussed
- Laboratory results:
  - Urine: clear, no growth, no protein
  - Thyroid function: normal
  - Lipoprotein(a): elevated
  - HDL: good
  - Total cholesterol: 5.2 mmol/L (on cutoff)
  - A1C: 5.5%
  - Sodium, potassium: normal
  - Vitamin B12: low at 200-213 pg/mL (cutoff 220 pg/mL)
  - Blood cells: normal
- Imaging: No imaging discussed

Assessment:
- The patient's recent labs show an elevated Lipoprotein(a), which I interpreted as indicating higher cardiovascular risk, and a low Vitamin B12 below the stated cutoff. Other labs are within normal ranges, including A1C with no evidence of diabetes.

Differential Diagnosis:
- Vitamin B12 deficiency
- Lipoprotein(a) elevation contributing to atherosclerotic cardiovascular risk
- Mixed hyperlipidemia

Plan:
- Vitamin B12 supplement or vitamin B complex (dose not specified)
- Increase cardiovascular activity to reduce cardiovascular risk
- Maintain strict cholesterol management

Follow up:
- Follow-up timing not specified

What this lab-review example deliberately does NOT contain — each would be a fabrication:
- Patient age, sex, or other demographics (none stated)
- Past medical history items such as hypertension, hyperlipidemia, or diabetes — I explicitly said "no diabetes," and nothing else was discussed
- Current medications such as Lisinopril or Atorvastatin (none stated)
- Family history of cardiovascular disease (none stated)
- Social history specifics about diet or exercise — the patient saying "I should start" does NOT establish a sedentary baseline
- A specific B12 dose ("1000 mcg daily") — I said "supplement" without a dose
- Visit type "telehealth" or "in-person" (not stated)
- A referral to cardiology, or a named cardiologist — no referral was discussed
- A specific follow-up interval such as "3 months" (none stated)
- Red-flag warnings such as "seek urgent care for chest pain" — I did not voice such warnings

OUTPUT FORMAT — plain text only, no markdown:

{icd_label}

Subjective:
- Chief complaint: [from transcript]
- History of present illness: [from transcript]
- Past medical history: [from transcript or explicit background; otherwise "Not discussed"]
- Surgical history: [from transcript or explicit background; otherwise "Not discussed"]
- Current medications:
  - [each medication on its own line, drawn from transcript or explicit background; if none stated in either, write "Not discussed"]
- Allergies: [from transcript or explicit background; otherwise "Not discussed"]
- Family history: [from transcript or explicit background; otherwise "Not discussed"]
- Social history: [from transcript or explicit background; otherwise "Not discussed"]
- Review of systems: [from transcript; otherwise "Not performed"]

Objective:
- [Visit type, ONLY if explicitly stated; otherwise omit this line entirely]
- Vital signs: [from transcript; otherwise "Not recorded"]
- General appearance: [from transcript; otherwise "Not discussed" — do NOT default to "appears well"]
- Physical examination: [from transcript; otherwise "Not discussed"]
- Laboratory results: [from transcript; otherwise "No new labs discussed"]
- Imaging: [from transcript; otherwise "No imaging discussed"]

Assessment:
- [ONE cohesive paragraph using ONLY findings and reasoning that appear in the transcript, written in first person ("I assessed…", "I characterized…"). Inline mention of {icd_instruction} is permitted but not required (the canonical location is the ICD line above the Subjective block); if you inline a code, render it as plain text with no marker or qualifier. Do NOT restate past medical history, medications, family history, or social history in the Assessment unless you explicitly tied them to today's reasoning. If the visit is purely a lab review with no clinical examination, the Assessment should describe the lab findings and my stated interpretation — nothing more. Not broken into sub-items.]

Differential Diagnosis:
- [List at least three diagnoses, ranked by clinical likelihood given the chief complaint and findings. Render every item as plain text — do NOT append "(suggested)", "(possible)", "(provisional)", or any other marker, qualifier, or annotation, regardless of whether the item was physician-stated or model-inferred. On a paperwork-only / wellness / lab-only visit with no chief complaint, list three plausible items consistent with the encounter type or the labs reviewed, still as plain text.]

Plan:
- [Each intervention as a separate dash line — ONLY interventions I discussed during the visit]

Follow up:
- [Follow-up timeline if I stated one; otherwise "Follow-up timing not specified"]
- [Seek urgent care for: specific red flags from transcript ONLY — omit this line if no red flags were voiced]
- [Return sooner if: conditions from transcript ONLY — omit this line if no such conditions were voiced]

Clinical Synopsis:
- [One-paragraph summary of visit. Use ONLY content already present in the Subjective/Objective/Assessment/Plan sections above — do not introduce new details. Output this exactly once, at the very end.]

FORMATTING RULES:
- Every content line starts with dash (-)
- Include ALL categories even if "Not discussed"
- One blank line between sections
- Assessment is ONE paragraph, not sub-items
- No decorative characters (no ===, ---, ***, ##)
- Plain text section headers followed by colon

SELF-CHECK BEFORE OUTPUT — for every line you produced, locate the transcript quote that supports it. If you cannot, replace the content with "Not discussed" / "Not performed" / "Not recorded" / "Not specified" or remove the line. Then run this category checklist:

1. Demographics check: any line stating age, sex, gender, race, or occupation must have a transcript quote. If absent, remove the detail.
2. Past medical history check: every PMH item must have a transcript quote (or be drawn from explicitly provided background context). If neither, write "Not discussed."
3. Medication check: drug name, dose, frequency, and route — every element must be stated in the transcript or supplied background. If only the drug was named, write the drug name with "dose not specified." Do not invent a canonical dose. Medications supplied via background but not mentioned in the transcript are still listed under Current medications.
4. Referral check: any specific provider name must have a transcript quote. If only the specialty was discussed, name the specialty only. If no referral was discussed, do not include a referral line.
5. Follow-up interval check: any duration ("in 3 months", "in 2 weeks") must have a transcript quote. If absent, write "Follow-up timing not specified."
6. Red-flag check: any "seek urgent care for X" warning must have a transcript quote. If absent, remove the line.
7. ICD code check: every ICD code is supported by a transcript-named diagnosis or inferred from findings, and is rendered as plain text with no marker or qualifier. On a paperwork-only / wellness / lab-only visit, use an encounter-type code (e.g. V70.0 / Z00.00) instead of leaving the field blank. Never append "(suggested)" or any similar annotation to the code.
8. Visit modality check: only call the visit "telehealth" or "in-person" if explicitly stated.
9. Assessment check: does the Assessment paragraph mention PMH, medications, family history, or social history that I did not tie to today's reasoning? If so, remove those mentions.
10. Differential Diagnosis count check: the Differential Diagnosis section contains at least three items, all rendered as plain text with no marker or qualifier suffix. If fewer than three are stateable from the transcript, fill the remaining slots with plausible items consistent with the chief complaint or findings — still as plain text, never marked "(suggested)".

Vital signs, exam findings, medication dosages, follow-up timing, and red-flag warnings are the most common fabrications. If a number, dose, or interval was not stated in the transcript, do not invent one. Clinical reasoning in the Assessment must reflect what was discussed during the visit. A short accurate note beats a long partially-fabricated one. Length is not a virtue."#
}

/// Build the SOAP system prompt: select template (custom or default), then resolve placeholders.
pub fn build_soap_prompt(config: &SoapPromptConfig) -> String {
    let template = config
        .custom_prompt
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_soap_prompt());

    let placeholders = soap_placeholders(&config.icd_version, &config.template);
    resolve_prompt(template, &placeholders)
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
            icd_version: "ICD-9".into(),
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        assert!(prompt.contains("ICD-9 Code: [specific code"));
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
            icd_version: "ICD-10".into(),
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
            icd_version: "both".into(),
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        assert!(prompt.contains("ICD-9 Code: [specific code"));
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
        // Lab-review example must teach the new always-on ICD as plain text
        let lab_idx = prompt
            .find("EXAMPLE 2")
            .expect("EXAMPLE 2 must be present");
        let after_example = &prompt[lab_idx..];
        assert!(after_example.contains("ICD-9 Code: 266.2"));
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
        let lab_idx = prompt
            .find("EXAMPLE 2")
            .expect("EXAMPLE 2 must be present");
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
            icd_version: "ICD-9".into(),
            ..Default::default()
        };
        let prompt = build_soap_prompt(&config);
        // Custom template is used, and placeholders are still resolved
        assert!(prompt.starts_with("My custom template with ICD-9 Code: [specific code"));
        assert!(prompt.contains("V70.0"));
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
    fn current_medications_format_allows_supplementary_background() {
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
            meds_block.contains("background"),
            "Current medications output format must allow supplementary background as a source.\nBlock:\n{meds_block}"
        );
    }

    #[test]
    fn historical_subjective_fields_allow_supplementary_background() {
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
                block.contains("background"),
                "{field} output format must allow supplementary background as a source.\nBlock:\n{block}"
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
    fn medication_self_check_allows_supplementary_background() {
        // Self-check rule #3 previously required medication elements to be
        // "stated in the transcript", which contradicts Rule #4 and causes
        // the model to drop background-supplied medications.
        let prompt = build_soap_prompt(&SoapPromptConfig::default());
        let idx = prompt
            .find("Medication check")
            .expect("Medication self-check entry missing");
        let block = &prompt[idx..idx + 400];
        assert!(
            block.contains("background"),
            "Medication self-check must acknowledge supplied background as a valid source.\nBlock:\n{block}"
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
        // Supplementary background, and reaffirm the no-alter-Plan rule.
        assert!(
            prompt.contains("authoritative") || prompt.contains("ground truth"),
            "system prompt must mark Patient record entries as authoritative"
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
        let sc_idx = prompt
            .find("SELF-CHECK")
            .expect("SELF-CHECK block missing");
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
        let sc_idx = prompt.find("SELF-CHECK").expect("SELF-CHECK section missing");

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
}
