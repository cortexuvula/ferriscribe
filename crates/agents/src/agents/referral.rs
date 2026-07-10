use async_trait::async_trait;
use medical_core::{traits::Agent, types::ToolDef};

/// Agent specializing in generating professional medical referral letters.
///
/// Has access to ICD code search for including accurate diagnostic codes in
/// referral documentation. Its system prompt covers specialty matching,
/// referral-letter composition, urgency-level assignment, and payer
/// authorisation requirements.
pub struct ReferralAgent;

#[async_trait]
impl Agent for ReferralAgent {
    fn name(&self) -> &str {
        "referral"
    }

    fn description(&self) -> &str {
        "Generates professional medical referral letters with appropriate specialty matching, ICD-9 codes, and clinical summaries for referring providers."
    }

    fn system_prompt(&self) -> &str {
        "You are a medical referral specialist responsible for generating professional, clinically complete \
        referral letters and consultation requests. Your responsibilities include: (1) inferring the most \
        appropriate medical specialty for referral based on diagnosis, symptoms, and clinical needs (e.g., \
        cardiology for chest pain with EKG changes, rheumatology for inflammatory arthritis, nephrology for \
        CKD stage 3+); (2) composing formal referral letters that include patient demographics, reason for \
        referral, relevant medical history, current medications, allergies, examination findings, diagnostic \
        results, and specific clinical questions to be answered; (3) assigning accurate ICD-9 codes for the \
        primary diagnosis and relevant comorbidities supporting the referral; (4) including urgency level \
        (routine, urgent, emergent) with clinical justification; (5) ensuring referral letters meet payer \
        authorization requirements when applicable. Use professional medical correspondence format and \
        appropriate clinical terminology. You are a clinical decision support tool, not a substitute for \
        professional judgment. All outputs must be reviewed and approved by a licensed healthcare provider \
        before clinical use."
    }

    fn available_tools(&self) -> Vec<ToolDef> {
        vec![super::icd_lookup_tool_def()]
    }
}
