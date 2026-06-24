//! The built-in default peer discussion system prompt and the
//! [`build_peer_discussion_prompt`] entry point that resolves placeholders
//! against [`PeerDiscussionPromptConfig`].
//!
//! The default prompt contains:
//! - A RULES block with core anti-fabrication constraints
//! - A FORBIDDEN INFERENCES block naming common hallucination categories
//! - An OUTPUT FORMAT section specifying the 6-section template
//! - FORMATTING RULES
//! - A SELF-CHECK checklist (placed last for LLM recency compliance)

use std::collections::HashMap;

use crate::prompt_resolver::resolve_prompt;

use super::PeerDiscussionPromptConfig;

// ---------------------------------------------------------------------------
// Placeholder resolution
// ---------------------------------------------------------------------------

/// Build the placeholder map for the peer discussion template.
fn peer_discussion_placeholders(
    physician_name: &str,
    specialty: &str,
    reason: &str,
) -> HashMap<&'static str, String> {
    let mut map = HashMap::new();
    map.insert("physician_name", physician_name.to_string());
    map.insert("specialty", specialty.to_string());
    map.insert("reason", reason.to_string());
    map
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// The built-in default peer discussion system prompt.
///
/// Contains placeholder tokens resolved by [`build_peer_discussion_prompt`]:
/// - `{physician_name}` — name of the consulting physician
/// - `{specialty}` — specialty of the consulting physician
/// - `{reason}` — reason for the peer discussion
///
/// # Anti-fabrication structure
///
/// The prompt is structured with layered fabrication guards:
///
/// 1. **RULES** — core constraints (transcript as sole source, no fabrication)
/// 2. **FORBIDDEN INFERENCES** — named categories of common hallucinations
/// 3. **OUTPUT FORMAT** — 6-section template (Header, Clinical Summary,
///    Discussion Points, Assessment, Recommendations, Action Items)
/// 4. **FORMATTING RULES** — plain-text formatting constraints
/// 5. **SELF-CHECK** — categorical checklist (placed last for recency)
pub fn default_peer_discussion_prompt() -> &'static str {
    r#"You are a physician creating a peer discussion note from a clinical consultation transcript.

You are consulting with Dr. {physician_name}, a {specialty} specialist, regarding: {reason}.

RULES:

1. NEVER fabricate, infer, or assume clinical details not in the transcript. If something was not discussed, write "Not discussed."
2. The transcript is the sole source of truth. Every clinical finding, symptom, medication, and diagnosis must be directly traceable to something said during the discussion.
3. Do NOT use medical knowledge to add details not mentioned during the discussion.
4. Say "the patient" — never use names.
5. Replace "VML" with "Valley Medical Laboratories."
6. Write the peer discussion note in first person, as the consulting physician. Use "I" for actions taken (e.g., "I recommended...", "I reviewed..."). Do NOT refer to yourself in the third person.
7. When referring to the consulting specialist, use "Dr. {physician_name}" or "the {specialty} specialist" — do not invent credentials or affiliations not stated in the transcript.

FORBIDDEN INFERENCES — DO NOT include any of these unless the transcript explicitly states them:

- Patient age, sex, gender, race, ethnicity, or occupation. Do not infer demographics from clinical context.
- Past medical conditions. Common comorbidities are NOT defaults — only list conditions named in the transcript.
- Current medications and dosages. If a drug was named without a dose, write the agent only with "dose not specified" — never pick a canonical dose.
- Family history items. Do not invent relatives' conditions or ages.
- Social history specifics. Do not invent diet descriptions, exercise level, tobacco/alcohol status, or living situation.
- Visit modality. Do not call the discussion "telehealth" or "in-person" unless explicitly mentioned.
- General-appearance descriptions when not commented on. Do not write "appears well" or "no acute distress" by default.
- Provider names for referrals. Name the specialty only. Never invent a specific provider's name.
- Follow-up intervals. If no timeframe was stated, write "Follow-up timing not specified" — do not default to any interval.
- Red-flag warnings. Only include warnings actually voiced. Do not add stock warnings.

OUTPUT FORMAT — plain text only, no markdown:

HEADER:
- Consulting physician: Dr. {physician_name}
- Specialty: {specialty}
- Reason for discussion: {reason}
- Date and time: [from transcript if stated; otherwise "Not specified"]

CLINICAL SUMMARY:
- Chief complaint: [from transcript]
- History of present illness: [from transcript]
- Relevant past medical history: [from transcript; otherwise "Not discussed"]
- Current medications: [from transcript; otherwise "Not discussed"]
- Allergies: [from transcript; otherwise "Not discussed"]
- Relevant surgical history: [from transcript; otherwise "Not discussed"]

DISCUSSION POINTS:
- [Each key clinical question or concern raised, as separate dash lines]
- [Include specific questions asked of the specialist]
- [Include specialist's responses and recommendations]

ASSESSMENT:
- [ONE cohesive paragraph summarizing the clinical situation and the specialist's input, written in first person. Include only findings and reasoning from the transcript.]

RECOMMENDATIONS:
- [Each recommendation as a separate dash line — ONLY recommendations explicitly discussed]
- [Include diagnostic recommendations]
- [Include treatment recommendations]
- [Include management strategies discussed]

ACTION ITEMS:
- [Each action item as a separate dash line]
- [Follow-up plans if discussed; otherwise "Follow-up timing not specified"]
- [Referrals if discussed; name specialty only unless a specific provider was named]
- [Pending tests or results if discussed]

FORMATTING RULES:
- Every content line starts with dash (-)
- Include ALL sections even if content is minimal
- One blank line between sections
- Assessment is ONE paragraph, not sub-items
- No decorative characters (no ===, ---, ***, ##)
- Plain text section headers followed by colon

SELF-CHECK BEFORE OUTPUT — for every line you produced, locate the transcript quote that supports it. If you cannot, replace the content with "Not discussed" or remove the line. Then run this category checklist:

1. Demographics check: any line stating age, sex, gender, race, or occupation must have a transcript quote. If absent, remove the detail.
2. Past medical history check: every PMH item must have a transcript quote. If none, write "Not discussed."
3. Medication check: drug name, dose, frequency, and route — every element must be stated in the transcript. If only the drug was named, write the drug name with "dose not specified." Do not invent a canonical dose.
4. Referral check: any specific provider name must have a transcript quote. If only the specialty was discussed, name the specialty only. If no referral was discussed, do not include a referral line.
5. Follow-up interval check: any duration must have a transcript quote. If absent, write "Follow-up timing not specified."
6. Red-flag check: any warning must have a transcript quote. If absent, remove the line.
7. Specialist attribution check: every statement attributed to the consulting specialist must have a transcript quote. Do not put words in the specialist's mouth.
8. Visit modality check: only call the discussion "telehealth" or "in-person" if explicitly stated.
9. Assessment check: does the Assessment paragraph mention details not discussed in the transcript? If so, remove those mentions.
10. Action items completeness check: every action item must trace to an explicit transcript statement. If an action was only implied, do not include it.

Clinical details, medication dosages, follow-up timing, and red-flag warnings are the most common fabrications. If a number, dose, or interval was not stated in the transcript, do not invent one. A short accurate note beats a long partially-fabricated one. Length is not a virtue."#
}

/// Build the peer discussion system prompt: select template (custom or default),
/// then resolve placeholders.
///
/// # Template Selection
///
/// If `config.custom_prompt` is `Some` and non-empty, it replaces the default
/// template entirely. Placeholders (`{physician_name}`, `{specialty}`,
/// `{reason}`) are still resolved in custom templates.
///
/// # Placeholder Resolution
///
/// | Placeholder | Source |
/// |---|---|
/// | `{physician_name}` | `config.physician_name` |
/// | `{specialty}` | `config.specialty` |
/// | `{reason}` | `config.reason` |
pub fn build_peer_discussion_prompt(config: &PeerDiscussionPromptConfig) -> String {
    let template = config
        .custom_prompt
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_peer_discussion_prompt());

    let placeholders =
        peer_discussion_placeholders(&config.physician_name, &config.specialty, &config.reason);
    resolve_prompt(template, &placeholders)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> PeerDiscussionPromptConfig {
        PeerDiscussionPromptConfig {
            physician_name: "Smith".into(),
            specialty: "Cardiology".into(),
            reason: "chest pain evaluation".into(),
            custom_prompt: None,
        }
    }

    #[test]
    fn default_prompt_has_structure_markers() {
        let config = default_config();
        let prompt = build_peer_discussion_prompt(&config);
        // Core section markers
        assert!(prompt.contains("HEADER:"));
        assert!(prompt.contains("CLINICAL SUMMARY:"));
        assert!(prompt.contains("DISCUSSION POINTS:"));
        assert!(prompt.contains("ASSESSMENT:"));
        assert!(prompt.contains("RECOMMENDATIONS:"));
        assert!(prompt.contains("ACTION ITEMS:"));
        // Rules section
        assert!(prompt.contains("RULES:"));
        assert!(prompt.contains("FORMATTING RULES"));
    }

    #[test]
    fn default_prompt_resolves_physician_name_placeholder() {
        let config = default_config();
        let prompt = build_peer_discussion_prompt(&config);
        assert!(prompt.contains("Dr. Smith"));
        assert!(!prompt.contains("{physician_name}"));
    }

    #[test]
    fn default_prompt_resolves_specialty_placeholder() {
        let config = default_config();
        let prompt = build_peer_discussion_prompt(&config);
        assert!(prompt.contains("Cardiology"));
        assert!(!prompt.contains("{specialty}"));
    }

    #[test]
    fn default_prompt_resolves_reason_placeholder() {
        let config = default_config();
        let prompt = build_peer_discussion_prompt(&config);
        assert!(prompt.contains("chest pain evaluation"));
        assert!(!prompt.contains("{reason}"));
    }

    #[test]
    fn custom_prompt_overrides_default() {
        let config = PeerDiscussionPromptConfig {
            physician_name: "Jones".into(),
            specialty: "Neurology".into(),
            reason: "headache evaluation".into(),
            custom_prompt: Some("Custom template consulting with Dr. {physician_name}".into()),
        };
        let prompt = build_peer_discussion_prompt(&config);
        // Custom template is used, and placeholders are still resolved
        assert!(prompt.starts_with("Custom template consulting with Dr. Jones"));
        assert!(!prompt.contains("{physician_name}"));
    }

    #[test]
    fn empty_custom_prompt_falls_back_to_default() {
        let config = PeerDiscussionPromptConfig {
            physician_name: "Smith".into(),
            specialty: "Cardiology".into(),
            reason: "chest pain".into(),
            custom_prompt: Some("".into()),
        };
        let prompt = build_peer_discussion_prompt(&config);
        // Empty string should not be treated as a real custom prompt
        assert!(prompt.contains("You are a physician creating a peer discussion note"));
    }

    #[test]
    fn default_prompt_includes_self_check_block() {
        let prompt = build_peer_discussion_prompt(&default_config());
        assert!(prompt.contains("SELF-CHECK"));
        assert!(prompt.contains("locate the transcript quote"));
    }

    #[test]
    fn self_check_block_is_at_end_for_recency() {
        let prompt = build_peer_discussion_prompt(&default_config());
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
    fn default_prompt_includes_forbidden_inferences_block() {
        let prompt = build_peer_discussion_prompt(&default_config());
        assert!(prompt.contains("FORBIDDEN INFERENCES"));
        // Demographics
        assert!(prompt.contains("Patient age, sex, gender"));
        // Common comorbidities
        assert!(prompt.contains("Common comorbidities"));
        // Default-dose fill
        assert!(prompt.contains("never pick a canonical dose"));
        // Invented provider names
        assert!(prompt.contains("Provider names for referrals"));
        // Default follow-up interval
        assert!(prompt.contains("Follow-up timing not specified"));
        // Stock red-flag warnings
        assert!(prompt.contains("Red-flag warnings"));
    }

    #[test]
    fn self_check_lists_category_checks() {
        let prompt = build_peer_discussion_prompt(&default_config());
        assert!(prompt.contains("Demographics check"));
        assert!(prompt.contains("Past medical history check"));
        assert!(prompt.contains("Medication check"));
        assert!(prompt.contains("Referral check"));
        assert!(prompt.contains("Follow-up interval check"));
        assert!(prompt.contains("Red-flag check"));
        assert!(prompt.contains("Specialist attribution check"));
        assert!(prompt.contains("Visit modality check"));
        assert!(prompt.contains("Assessment check"));
        assert!(prompt.contains("Action items completeness check"));
    }

    #[test]
    fn default_prompt_mandates_first_person_voice() {
        let prompt = build_peer_discussion_prompt(&default_config());
        assert!(
            prompt.contains("first person"),
            "system prompt must mandate first-person voice"
        );
    }
}
