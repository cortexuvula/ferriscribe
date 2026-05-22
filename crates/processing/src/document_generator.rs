//! Prompt builders for referral letters, patient correspondence, and synopses.
//!
//! Each builder accepts an optional custom template override; placeholders
//! (`{recipient_type}`, `{urgency}`, `{letter_type}`) are resolved by
//! `prompt_resolver::resolve_prompt`.

use std::collections::HashMap;

use chrono::Local;

use crate::prompt_resolver::resolve_prompt;

// ---------------------------------------------------------------------------
// Letter audience context
// ---------------------------------------------------------------------------

/// Lightweight prompt-relevant subset of `medical_core::types::LetterAudience`.
///
/// Carries only the fields needed for prompt construction so that callers don't
/// need to pass the full DB entity (with id, timestamps, etc.) into the prompt
/// builder.
#[derive(Debug, Clone)]
pub struct LetterAudienceContext {
    pub name: String,
    pub system_prompt: String,
    pub user_template: Option<String>,
}

fn format_now_for_prompt() -> String {
    Local::now().format("Time %H:%M Date %d %b %Y").to_string()
}

// ---------------------------------------------------------------------------
// Default templates
// ---------------------------------------------------------------------------

pub fn default_referral_prompt() -> &'static str {
    "You are a medical scribe assistant specialising in professional referral letters. \
     Write a formal referral letter addressed to a {recipient_type}. \
     The urgency of this referral is: {urgency}. \
     Use appropriate clinical language, include relevant history and findings from the SOAP \
     note, clearly state the reason for referral, and request the desired action. \
     Format the letter professionally with greeting, body, and closing."
}

pub fn default_letter_prompt() -> &'static str {
    "You are a medical scribe assistant helping to write patient-friendly correspondence. \
     Generate a {letter_type} letter for the patient. \
     Use clear, plain language the patient can understand. \
     Avoid unexplained medical jargon. \
     Be empathetic and professional."
}

pub fn default_synopsis_prompt() -> &'static str {
    "You are a medical scribe assistant. Summarise the provided SOAP note in a \
     concise synopsis of no more than 200 words. \
     Capture the key subjective complaints, objective findings, primary diagnosis, \
     and treatment plan. \
     Write in clear, professional language suitable for a quick clinical overview."
}

// ---------------------------------------------------------------------------
// Referral letter
// ---------------------------------------------------------------------------

/// Build `(system_prompt, user_prompt)` for generating a referral letter.
pub fn build_referral_prompt(
    soap_note: &str,
    recipient_type: &str,
    urgency: &str,
    custom_template: Option<&str>,
) -> (String, String) {
    let template = custom_template
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_referral_prompt());

    let mut placeholders = HashMap::new();
    placeholders.insert("recipient_type", recipient_type.to_string());
    placeholders.insert("urgency", urgency.to_string());

    let system = resolve_prompt(template, &placeholders);

    let time_date = format_now_for_prompt();
    let user = format!(
        "Please write a referral letter to a {recipient_type} with {urgency} urgency based on \
         the following SOAP note:\n\n{time_date}\n\n{soap_note}",
        recipient_type = recipient_type,
        urgency = urgency,
        time_date = time_date,
        soap_note = soap_note,
    );

    (system, user)
}

// ---------------------------------------------------------------------------
// Patient letter
// ---------------------------------------------------------------------------

/// Build `(system_prompt, user_prompt)` for generating patient correspondence.
///
/// # Resolution order
/// 1. If `audience` is provided AND has `user_template`, use the audience's
///    `system_prompt` and `user_template` (with `{letter_type}`, `{soap_note}`,
///    `{time_date}` placeholders resolved).
/// 2. If `audience` is provided but has no `user_template`, use the audience's
///    `system_prompt` and the default user template with the audience name.
/// 3. If `audience` is `None`, fall back to legacy behaviour: use
///    `custom_template` if provided, otherwise the default letter prompt.
pub fn build_letter_prompt(
    soap_note: &str,
    letter_type: &str,
    audience: Option<&LetterAudienceContext>,
    custom_template: Option<&str>,
) -> (String, String) {
    let time_date = format_now_for_prompt();

    let mut placeholders = HashMap::new();
    placeholders.insert("letter_type", letter_type.to_string());

    if let Some(aud) = audience {
        // Audience provided — use its system prompt
        let system = aud.system_prompt.clone();

        if let Some(ref user_tmpl) = aud.user_template {
            // Case 1: audience with user_template — resolve placeholders
            let user = resolve_audience_user_template(
                user_tmpl, letter_type, &time_date, soap_note,
            );
            return (system, user);
        }

        // Case 2: audience without user_template — default user template with audience name
        let user = format!(
            "Please write a {letter_type} letter for {audience_name} based on the following SOAP \
             note:\n\n{time_date}\n\n{soap_note}",
            letter_type = letter_type,
            audience_name = aud.name,
            time_date = time_date,
            soap_note = soap_note,
        );
        return (system, user);
    }

    // Case 3: no audience — legacy behaviour
    let template = custom_template
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_letter_prompt());

    let system = resolve_prompt(&template, &placeholders);

    let user = format!(
        "Please write a {letter_type} letter for the patient based on the following SOAP \
         note:\n\n{time_date}\n\n{soap_note}",
        letter_type = letter_type,
        time_date = time_date,
        soap_note = soap_note,
    );

    (system, user)
}

/// Resolve `{letter_type}`, `{time_date}`, and `{soap_note}` in an audience user template.
fn resolve_audience_user_template(
    template: &str,
    letter_type: &str,
    time_date: &str,
    soap_note: &str,
) -> String {
    let mut out = template.to_string();
    out = out.replace("{letter_type}", letter_type);
    out = out.replace("{time_date}", time_date);
    out = out.replace("{soap_note}", soap_note);
    out
}

// ---------------------------------------------------------------------------
// Synopsis
// ---------------------------------------------------------------------------

/// Build `(system_prompt, user_prompt)` for generating a brief SOAP synopsis.
pub fn build_synopsis_prompt(
    soap_note: &str,
    custom_template: Option<&str>,
) -> (String, String) {
    let template = custom_template
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_synopsis_prompt());

    // Synopsis template has no placeholders; pass empty map.
    let system = resolve_prompt(template, &HashMap::new());

    let time_date = format_now_for_prompt();
    let user = format!(
        "Please summarise the following SOAP note in under 200 words:\n\n{time_date}\n\n{soap_note}",
        time_date = time_date,
        soap_note = soap_note,
    );

    (system, user)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referral_default_contains_recipient_and_urgency() {
        let soap = "S: Chest pain\nO: BP 140/90\nA: Hypertension\nP: Refer to Cardiology";
        let (system, user) = build_referral_prompt(soap, "Cardiologist", "urgent", None);

        assert!(system.contains("Cardiologist"));
        assert!(system.contains("urgent"));
        assert!(!system.contains("{recipient_type}"));
        assert!(!system.contains("{urgency}"));
        assert!(user.contains("Chest pain"));
        assert!(user.contains("Time") && user.contains("Date"));
    }

    #[test]
    fn referral_custom_template_overrides() {
        let soap = "S: foo";
        let custom = "CUSTOM: Refer to {recipient_type} ({urgency})";
        let (system, _user) = build_referral_prompt(soap, "Neurology", "routine", Some(custom));

        assert!(system.starts_with("CUSTOM: Refer to Neurology (routine)"));
    }

    #[test]
    fn referral_empty_custom_falls_back_to_default() {
        let soap = "S: foo";
        let (system, _user) = build_referral_prompt(soap, "Derm", "routine", Some(""));
        assert!(system.contains("professional referral letters"));
    }

    #[test]
    fn letter_default_contains_type() {
        let soap = "S: Anxiety\nO: HR 90\nA: GAD\nP: CBT referral";
        let (system, user) = build_letter_prompt(soap, "results", None, None);

        assert!(system.contains("results"));
        assert!(!system.contains("{letter_type}"));
        assert!(user.contains("Anxiety"));
        assert!(user.contains("Time") && user.contains("Date"));
    }

    #[test]
    fn letter_custom_template_overrides() {
        let soap = "S: foo";
        let custom = "CUSTOM: {letter_type} letter";
        let (system, _user) = build_letter_prompt(soap, "follow-up", None, Some(custom));
        assert!(system.starts_with("CUSTOM: follow-up letter"));
    }

    #[test]
    fn letter_with_audience_uses_audience_prompts() {
        let soap = "S: Chest tightness\nO: ECG normal\nA: Musculoskeletal chest pain\nP: Reassure";
        let audience = LetterAudienceContext {
            name: "Insurance Company".into(),
            system_prompt: "You are writing to an insurance company. Be factual and concise.".into(),
            user_template: Some(
                "Generate a {letter_type} letter for the insurance company.\n\
                 Reference date: {time_date}\n\nSOAP note:\n{soap_note}"
                    .into(),
            ),
        };
        let (system, user) = build_letter_prompt(soap, "medical report", Some(&audience), None);

        assert!(system.contains("insurance company"));
        assert!(system.contains("factual and concise"));
        assert!(user.contains("medical report"));
        assert!(user.contains("insurance company"));
        assert!(user.contains("Chest tightness"));
        assert!(user.contains("Time"));
    }

    #[test]
    fn letter_with_audience_no_user_template_uses_default() {
        let soap = "S: Headache\nO: Neuro exam normal\nA: Tension headache\nP: Analgesia";
        let audience = LetterAudienceContext {
            name: "Employer".into(),
            system_prompt: "Write a professional letter to an employer regarding fitness for work.".into(),
            user_template: None,
        };
        let (system, user) = build_letter_prompt(soap, "fitness", Some(&audience), None);

        assert!(system.contains("fitness for work"));
        // Default user template should include audience name
        assert!(user.contains("for Employer"));
        assert!(user.contains("fitness"));
        assert!(user.contains("Headache"));
    }

    #[test]
    fn letter_without_audience_uses_legacy_behavior() {
        let soap = "S: Back pain\nO: Limited flexion\nA: Lumbar strain\nP: Physio";
        // audience=None, custom_template=None -> default
        let (system, user) = build_letter_prompt(soap, "results", None, None);
        assert!(system.contains("patient-friendly"));
        assert!(user.contains("for the patient"));
        assert!(user.contains("results"));

        // audience=None, custom_template=Some -> custom
        let custom = "LEGACY CUSTOM: {letter_type}";
        let (system2, _user2) = build_letter_prompt(soap, "summary", None, Some(custom));
        assert!(system2.starts_with("LEGACY CUSTOM: summary"));
    }

    #[test]
    fn letter_audience_ignores_custom_template() {
        let soap = "S: Dizziness\nO: CN intact\nA: BPPV\nP: Epley manoeuvre";
        let audience = LetterAudienceContext {
            name: "Specialist".into(),
            system_prompt: "Write to a specialist colleague.".into(),
            user_template: Some("AUDIENCE USER: {letter_type} re {soap_note}".into()),
        };
        let custom = "THIS CUSTOM TEMPLATE SHOULD BE IGNORED";

        // Even though custom_template is provided, audience takes precedence
        let (system, user) = build_letter_prompt(soap, "referral", Some(&audience), Some(custom));

        assert!(system.contains("specialist colleague"));
        assert!(!system.contains("THIS CUSTOM TEMPLATE"));
        assert!(user.starts_with("AUDIENCE USER:"));
        assert!(user.contains("referral"));
        assert!(user.contains("Dizziness"));
    }

    #[test]
    fn synopsis_default_mentions_word_limit() {
        let soap = "S: Patient reports fatigue\nO: Haemoglobin 9.0\nA: Iron deficiency anaemia";
        let (system, user) = build_synopsis_prompt(soap, None);
        assert!(system.contains("200 words") || system.contains("200-word"));
        assert!(user.contains("Iron deficiency anaemia"));
        assert!(user.contains("Time") && user.contains("Date"));
    }

    #[test]
    fn synopsis_custom_template_overrides() {
        let soap = "S: foo";
        let (system, _user) = build_synopsis_prompt(soap, Some("CUSTOM SYNOPSIS"));
        assert!(system.starts_with("CUSTOM SYNOPSIS"));
    }
}
