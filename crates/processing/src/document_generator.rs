//! Prompt builders for referral letters, patient correspondence, and synopses.
//!
//! Each builder accepts an optional custom template override; placeholders
//! (`{recipient_type}`, `{urgency}`, `{letter_type}`) are resolved by
//! [`prompt_resolver::resolve_prompt`](crate::prompt_resolver::resolve_prompt).
//!
//! All builders return a `(system_prompt, user_prompt)` tuple ready to be
//! passed to an AI provider's completion API. The system prompt carries the
//! role and formatting instructions; the user prompt carries the SOAP note
//! content and any contextual parameters.
//!
//! # Letter Audience Resolution
//!
//! `build_letter_prompt` supports three resolution paths, in precedence order:
//!
//! 1. Audience with `user_template` — uses the audience's `system_prompt` and
//!    resolves `{letter_type}`, `{time_date}`, `{soap_note}` in the user template.
//! 2. Audience without `user_template` — uses the audience's `system_prompt`
//!    and a default user template referencing the audience name.
//! 3. No audience (legacy) — uses `custom_template` if provided, otherwise the
//!    default letter prompt.

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
/// builder. See [`build_letter_prompt`] for the resolution order when this
/// type is provided.
#[derive(Debug, Clone)]
pub struct LetterAudienceContext {
    /// Display name of the audience (e.g., "Insurance Company", "Employer").
    pub name: String,
    /// System prompt for this audience (role and tone instructions).
    pub system_prompt: String,
    /// Optional user-template override with `{letter_type}`, `{time_date}`,
    /// `{soap_note}` placeholders. When `None`, a default user template
    /// referencing `name` is used.
    pub user_template: Option<String>,
}

fn format_now_for_prompt() -> String {
    Local::now().format("Time %H:%M Date %d %b %Y").to_string()
}

// ---------------------------------------------------------------------------
// Default templates
// ---------------------------------------------------------------------------

/// Returns the built-in default referral letter system prompt template.
///
/// Contains `{recipient_type}` and `{urgency}` placeholders that are resolved
/// by [`build_referral_prompt`].
pub fn default_referral_prompt() -> &'static str {
    "You are a medical scribe assistant specialising in professional referral letters. \
     Write a formal referral letter addressed to a {recipient_type}. \
     The urgency of this referral is: {urgency}. \
     Use appropriate clinical language, include relevant history and findings from the SOAP \
     note, clearly state the reason for referral, and request the desired action. \
     Format the letter professionally with greeting, body, and closing."
}

/// Returns the built-in default patient letter system prompt template.
///
/// Contains a `{letter_type}` placeholder that is resolved by
/// [`build_letter_prompt`] (legacy path only — when no audience is provided).
pub fn default_letter_prompt() -> &'static str {
    "You are a medical scribe assistant helping to write patient-friendly correspondence. \
     Generate a {letter_type} letter for the patient. \
     Use clear, plain language the patient can understand. \
     Avoid unexplained medical jargon. \
     Be empathetic and professional."
}

/// Returns the built-in default synopsis system prompt template.
///
/// The synopsis template has no placeholders. It instructs the model to
/// produce a concise (≤200 word) clinical summary of the SOAP note.
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
///
/// # Template Resolution
///
/// If `custom_template` is provided and non-empty, it is used in place of
/// [`default_referral_prompt`]. The `{recipient_type}` and `{urgency}`
/// placeholders are resolved via [`resolve_prompt`].
///
/// The user prompt includes the current date/time and the full SOAP note.
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
///
/// 1. If `audience` is provided AND has `user_template`, use the audience's
///    `system_prompt` and `user_template` (with `{letter_type}`, `{soap_note}`,
///    `{time_date}` placeholders resolved).
/// 2. If `audience` is provided but has no `user_template`, use the audience's
///    `system_prompt` and the default user template with the audience name.
/// 3. If `audience` is `None`, fall back to legacy behaviour: use
///    `custom_template` if provided, otherwise the default letter prompt.
///
/// **Note:** when an audience is provided, `custom_template` is ignored —
/// audience-specific prompts take precedence.
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

// NOTE: see also `postprocess::clean_text` which strips similar markdown patterns
// for SOAP output. If consolidating in the future, extract shared helpers.

/// Remove common markdown syntax from AI-generated text.
///
/// Converts headings to uppercase, replaces bullets with `•`, strips bold/italic
/// markers, inline code backticks, link syntax, and horizontal rules. Intended
/// as a safety net when prompts request plain text but the model produces markdown.
pub fn strip_markdown(text: &str) -> String {
    use regex::Regex;
    use std::sync::LazyLock;

    static HEADING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^#{1,6}\s+(.+)$").unwrap());
    static BOLD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());
    static ITALIC_STAR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\*([^*]+)\*").unwrap());
    static ITALIC_UNDER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b_([^_]+?)_\b").unwrap());
    static INLINE_CODE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
    static LINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]+\)").unwrap());
    static BULLET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^(\s*)[*-]\s+").unwrap());
    static HR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^[-*_]{3,}\s*$").unwrap());

    let mut out = text.to_string();

    // Convert headings to uppercase lines
    out = HEADING
        .replace_all(&out, |caps: &regex::Captures| {
            caps[1].to_uppercase()
        })
        .into_owned();

    // Strip bold
    out = BOLD.replace_all(&out, "$1").into_owned();

    // Strip italic (star and underscore)
    out = ITALIC_STAR.replace_all(&out, "$1").into_owned();
    out = ITALIC_UNDER.replace_all(&out, "$1").into_owned();

    // Strip inline code
    out = INLINE_CODE.replace_all(&out, "$1").into_owned();

    // Strip links (keep text)
    out = LINK.replace_all(&out, "$1").into_owned();

    // Replace bullets with bullet character
    out = BULLET.replace_all(&out, "${1}• ").into_owned();

    // Remove horizontal rules (line entirely)
    out = HR.replace_all(&out, "").into_owned();

    // Collapse runs of 3+ blank lines to 2
    static MULTI_BLANK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
    out = MULTI_BLANK.replace_all(&out, "\n\n").into_owned();

    out
}

// ---------------------------------------------------------------------------
// Synopsis
// ---------------------------------------------------------------------------

/// Build `(system_prompt, user_prompt)` for generating a brief SOAP synopsis.
///
/// If `custom_template` is provided and non-empty, it replaces
/// [`default_synopsis_prompt`]. The synopsis template has no placeholders.
///
/// The user prompt includes the current date/time and the full SOAP note,
/// with an instruction to summarise in under 200 words.
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

    #[test]
    fn strip_markdown_removes_bold() {
        assert_eq!(strip_markdown("**important**"), "important");
    }

    #[test]
    fn strip_markdown_removes_italic() {
        assert_eq!(strip_markdown("*emphasis*"), "emphasis");
        assert_eq!(strip_markdown("_emphasis_"), "emphasis");
    }

    #[test]
    fn strip_markdown_converts_heading_to_uppercase() {
        assert_eq!(strip_markdown("## Reason for Referral"), "REASON FOR REFERRAL");
    }

    #[test]
    fn strip_markdown_converts_bullets() {
        assert_eq!(strip_markdown("- First item"), "• First item");
        assert_eq!(strip_markdown("* First item"), "• First item");
    }

    #[test]
    fn strip_markdown_removes_inline_code() {
        assert_eq!(strip_markdown("use `metric` units"), "use metric units");
    }

    #[test]
    fn strip_markdown_removes_links() {
        assert_eq!(strip_markdown("[click here](http://example.com)"), "click here");
    }

    #[test]
    fn strip_markdown_removes_horizontal_rules() {
        let input = "Above\n\n---\n\nBelow";
        assert_eq!(strip_markdown(input), "Above\n\nBelow");
    }

    #[test]
    fn strip_markdown_preserves_plain_text() {
        let input = "Dear Dr Smith,\n\nI am writing to refer the patient.\n\nSincerely,\nDr Jones";
        assert_eq!(strip_markdown(input), input);
    }

    #[test]
    fn strip_markdown_preserves_underscores_in_identifiers() {
        assert_eq!(strip_markdown("patient_id and some_variable"), "patient_id and some_variable");
    }
}
