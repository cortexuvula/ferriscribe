//! Application configuration types.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::agent::AgentSettings;

/// How speech-to-text is performed.
///
/// Selected in the settings UI. [`Local`](SttMode::Local) uses in-process
/// whisper-rs; [`Remote`](SttMode::Remote) sends audio to an
/// OpenAI-compatible Whisper server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SttMode {
    /// In-process whisper-rs on this machine.
    #[default]
    Local,
    /// HTTP POST to an OpenAI-compatible Whisper server.
    Remote,
}

/// AI providers supported at runtime.
///
/// Used by [`AppConfig::migrate`] to reject stale values left over from
/// older versions of the app that supported cloud providers. Only
/// `"lmstudio"` and `"ollama"` are valid.
pub const SUPPORTED_AI_PROVIDERS: &[&str] = &["lmstudio", "ollama"];

/// Supported TTS provider names. Only local TTS is supported — AGENTS.md
/// forbids hosted AI APIs, so cloud providers (e.g. ElevenLabs) were removed.
pub const SUPPORTED_TTS_PROVIDERS: &[&str] = &["local"];

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// UI theme preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Theme {
    Dark,
    #[default]
    Light,
}

/// ICD coding version used when generating SOAP notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum IcdVersion {
    #[default]
    Icd9,
    Icd10,
    Both,
}

/// The SOAP note template style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SoapTemplate {
    #[default]
    FollowUp,
    NewPatient,
    Telehealth,
    Emergency,
    Pediatric,
    Geriatric,
}

// ---------------------------------------------------------------------------
// ContextTemplate
// ---------------------------------------------------------------------------

/// A named snippet of clinical context text the user can apply to the
/// Patient Context field at recording time.
///
/// Stored in [`AppConfig::custom_context_templates`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTemplate {
    /// Display name shown in the template picker.
    pub name: String,
    /// The context text to insert.
    pub body: String,
}

// ---------------------------------------------------------------------------
// SoapNoteSettings
// ---------------------------------------------------------------------------

/// ICD coding settings for SOAP generation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoapNoteSettings {
    pub icd_code_version: IcdVersion,
}

// ---------------------------------------------------------------------------
// Default helpers
// ---------------------------------------------------------------------------

fn default_language() -> String {
    "en-US".into()
}

fn default_true() -> bool {
    true
}

fn default_conditions() -> Vec<String> {
    [
        "Hypertension",
        "Type 2 diabetes",
        "Hyperlipidemia",
        "Asthma",
        "COPD",
        "Hypothyroidism",
        "Atrial fibrillation",
        "Coronary artery disease",
        "CKD (chronic kidney disease)",
        "GERD",
        "Anxiety",
        "Depression",
        "Osteoarthritis",
        "Obesity",
        "Sleep apnea",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_sample_rate() -> u32 {
    44100
}

fn default_channels() -> u16 {
    1
}

fn default_ai_provider() -> String {
    "lmstudio".into()
}

fn default_ai_model() -> String {
    "gpt-4o".into()
}

fn default_whisper_model() -> String {
    "large-v3-turbo".into()
}

fn default_tts_provider() -> String {
    "local".into()
}

fn default_tts_voice() -> String {
    "default".into()
}

fn default_temperature() -> f32 {
    0.2
}

fn default_icd_version() -> IcdVersion {
    IcdVersion::default()
}

fn default_soap_template() -> SoapTemplate {
    SoapTemplate::default()
}

fn default_embedding_model() -> String {
    "text-embedding-3-small".into()
}

fn default_search_top_k() -> u32 {
    5
}

fn default_mmr_lambda() -> f32 {
    0.7
}

fn default_autosave_enabled() -> bool {
    true
}

fn default_autosave_interval_secs() -> u64 {
    60
}

fn default_quick_continue_mode() -> bool {
    true
}

fn default_max_background_workers() -> u32 {
    2
}

fn default_show_processing_notifications() -> bool {
    true
}

fn default_auto_retry_failed() -> bool {
    true
}

fn default_auto_generate_soap() -> bool {
    false
}

fn default_max_retry_attempts() -> u32 {
    3
}

fn default_window_width() -> u32 {
    1200
}

fn default_window_height() -> u32 {
    800
}

fn default_auto_index_rag() -> bool {
    true
}

fn default_lmstudio_host() -> String {
    "localhost".into()
}

fn default_lmstudio_port() -> u16 {
    1234
}

fn default_stt_remote_port() -> u16 {
    8080
}

fn default_stt_remote_model() -> String {
    "whisper-1".into()
}

fn default_ollama_host() -> String {
    "localhost".into()
}

fn default_ollama_port() -> u16 {
    11434
}

fn default_vocabulary_enabled() -> bool {
    true
}

fn default_medical_dict_enabled() -> bool {
    true
}

fn default_rsvp_wpm() -> u32 {
    300
}

fn default_rsvp_font_size() -> u32 {
    48
}

fn default_rsvp_chunk_size() -> u8 {
    1
}

fn default_rsvp_dark_theme() -> bool {
    true
}

fn default_rsvp_show_context() -> bool {
    false
}

fn default_rsvp_audio_cue() -> bool {
    false
}

fn default_rsvp_auto_start() -> bool {
    true
}

fn default_rsvp_remember_sections() -> bool {
    false
}

fn default_rsvp_remembered_sections() -> Vec<String> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// AppConfig
// ---------------------------------------------------------------------------

/// The full application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    // General
    #[serde(default)]
    pub theme: Theme,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub storage_path: Option<String>,

    // Audio
    #[serde(default)]
    pub input_device: Option<String>,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_channels")]
    pub channels: u16,

    // Providers
    #[serde(default = "default_ai_provider")]
    pub ai_provider: String,
    #[serde(default = "default_ai_model")]
    pub ai_model: String,
    /// Vision-capable model name for OCR (e.g. "glm-ocr").
    /// If None, falls back to `ai_model` for OCR.
    #[serde(default)]
    pub ocr_model: Option<String>,
    #[serde(default = "default_whisper_model")]
    pub whisper_model: String,
    #[serde(default = "default_tts_provider")]
    pub tts_provider: String,
    #[serde(default = "default_tts_voice")]
    pub tts_voice: String,
    // LM Studio remote server
    #[serde(default = "default_lmstudio_host")]
    pub lmstudio_host: String,
    #[serde(default = "default_lmstudio_port")]
    pub lmstudio_port: u16,

    // STT mode selection
    #[serde(default)]
    pub stt_mode: SttMode,
    // Remote Whisper server (when stt_mode == Remote)
    #[serde(default)]
    pub stt_remote_host: String,
    #[serde(default = "default_stt_remote_port")]
    pub stt_remote_port: u16,
    #[serde(default = "default_stt_remote_model")]
    pub stt_remote_model: String,

    // Ollama server (local or remote on LAN)
    #[serde(default = "default_ollama_host")]
    pub ollama_host: String,
    #[serde(default = "default_ollama_port")]
    pub ollama_port: u16,

    // Temperature
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    // Processing
    #[serde(default)]
    pub auto_generate_referral: bool,
    #[serde(default)]
    pub auto_generate_letter: bool,
    #[serde(default = "default_auto_generate_soap")]
    pub auto_generate_soap: bool,
    #[serde(default = "default_auto_index_rag")]
    pub auto_index_rag: bool,
    #[serde(default = "default_vocabulary_enabled")]
    pub vocabulary_enabled: bool,
    #[serde(default = "default_medical_dict_enabled")]
    pub medical_dict_enabled: bool,
    // Diarization: cap the number of speaker clusters. When set, the greedy
    // clustering algorithm merges the most-similar centroids until the count
    // is at or below this value. `None` means no limit (auto-detect).
    #[serde(default)]
    pub max_speakers: Option<u32>,
    // RSVP speed-reader
    #[serde(default = "default_rsvp_wpm")]
    pub rsvp_wpm: u32,
    #[serde(default = "default_rsvp_font_size")]
    pub rsvp_font_size: u32,
    #[serde(default = "default_rsvp_chunk_size")]
    pub rsvp_chunk_size: u8,
    #[serde(default = "default_rsvp_dark_theme")]
    pub rsvp_dark_theme: bool,
    #[serde(default = "default_rsvp_show_context")]
    pub rsvp_show_context: bool,
    #[serde(default = "default_rsvp_audio_cue")]
    pub rsvp_audio_cue: bool,
    #[serde(default = "default_rsvp_auto_start")]
    pub rsvp_auto_start: bool,
    #[serde(default = "default_rsvp_remember_sections")]
    pub rsvp_remember_sections: bool,
    #[serde(default = "default_rsvp_remembered_sections")]
    pub rsvp_remembered_sections: Vec<String>,
    #[serde(default)]
    pub custom_context_templates: Vec<ContextTemplate>,
    #[serde(default = "default_icd_version")]
    pub icd_version: IcdVersion,

    // Templates
    #[serde(default = "default_soap_template")]
    pub soap_template: SoapTemplate,
    #[serde(default)]
    pub custom_soap_prompt: Option<String>,
    #[serde(default)]
    pub custom_referral_prompt: Option<String>,
    #[serde(default)]
    pub custom_letter_prompt: Option<String>,
    #[serde(default)]
    pub custom_synopsis_prompt: Option<String>,
    #[serde(default)]
    pub custom_peer_discussion_prompt: Option<String>,

    // RAG
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    #[serde(default = "default_search_top_k")]
    pub search_top_k: u32,
    #[serde(default = "default_mmr_lambda")]
    pub mmr_lambda: f32,

    // Autosave
    #[serde(default = "default_autosave_enabled")]
    pub autosave_enabled: bool,
    #[serde(default = "default_autosave_interval_secs")]
    pub autosave_interval_secs: u64,

    // Features
    #[serde(default = "default_quick_continue_mode")]
    pub quick_continue_mode: bool,
    #[serde(default = "default_max_background_workers")]
    pub max_background_workers: u32,
    #[serde(default = "default_show_processing_notifications")]
    pub show_processing_notifications: bool,
    #[serde(default = "default_auto_retry_failed")]
    pub auto_retry_failed: bool,
    #[serde(default = "default_max_retry_attempts")]
    pub max_retry_attempts: u32,

    // Window
    #[serde(default = "default_window_width")]
    pub window_width: u32,
    #[serde(default = "default_window_height")]
    pub window_height: u32,

    // Sub-configs
    #[serde(default)]
    pub soap_note_settings: SoapNoteSettings,
    #[serde(default)]
    pub agent_settings: HashMap<String, AgentSettings>,

    // Endpoint policy
    /// Opt-out for the local-only AI/STT endpoint allowlist. When `true`,
    /// `validate_local_endpoint` accepts public hosts. Default `false`.
    /// See `crates/core/src/endpoint_policy.rs`.
    #[serde(default)]
    pub allow_public_endpoint: bool,

    // Onboarding
    /// `true` once the user has completed (or skipped through) the first-run
    /// onboarding wizard. The wizard is shown only when this is `false` AND no
    /// prior config existed (existing installs are auto-marked in `get_settings`).
    #[serde(default)]
    pub onboarding_completed: bool,

    // Updates
    /// When true, the app checks GitHub Releases for updates on launch and
    /// every 12h. The user consents to this in onboarding and can toggle it in
    /// Settings → About. Default true (the recommended path). The check is an
    /// anonymous GET for latest.json — no PHI transmitted.
    #[serde(default = "default_true")]
    pub auto_update_check: bool,

    // Quick-add condition chips
    /// User-customizable list of conditions shown as clickable chips under
    /// "Known conditions" in both the Record tab sidebar and the Generate tab.
    /// Default is a set of common chronic conditions; the user can add/remove
    /// to match their practice. Empty vec for fresh installs is backfilled by
    /// the frontend when the list is empty (see default_conditions).
    #[serde(default = "default_conditions")]
    pub custom_conditions: Vec<String>,

    // Training corpus
    /// When true, every successful SOAP generation is captured into the
    /// `generations` table for the training-corpus feature. Defaults to
    /// false — captures are opt-in. See
    /// docs/superpowers/specs/2026-05-11-training-corpus-design.md.
    #[serde(default)]
    pub capture_for_training: bool,

    // Condition chip sync
    /// When true, condition chip presets sync two-way between this machine
    /// and the paired server via the vocab API. Defaults to false — each
    /// machine keeps its own list unless the user opts in.
    #[serde(default)]
    pub sync_condition_chips: bool,

    // Content sync
    /// When true, patient content (transcripts, SOAP notes, letters, peer
    /// discussions, audio) syncs two-way between this machine and the paired
    /// server over Tailscale. Requires Tailscale on both machines. Defaults
    /// to false.
    #[serde(default)]
    pub sync_content: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        serde_json::from_str("{}").expect("AppConfig default deserialization should never fail")
    }
}

impl AppConfig {
    /// Migrate deserialized config values to match the current supported set.
    ///
    /// Run after deserialization; silently corrects values that are no
    /// longer valid (e.g. cloud provider names like `"openai"` or
    /// `"anthropic"` left over from older versions are migrated to
    /// `"lmstudio"`).
    pub fn migrate(&mut self) {
        if !SUPPORTED_AI_PROVIDERS.contains(&self.ai_provider.as_str()) {
            tracing::warn!(
                stale = %self.ai_provider,
                "ai_provider migrated to 'lmstudio' (cloud providers are no longer supported)"
            );
            self.ai_provider = "lmstudio".into();
        }

        // Migrate the removed cloud TTS provider to local. AGENTS.md forbids
        // hosted AI APIs; the ElevenLabs provider was deleted, so any config
        // still naming it (or any non-local value) is migrated via a positive
        // allowlist (mirrors the ai_provider migration pattern).
        if !SUPPORTED_TTS_PROVIDERS.contains(&self.tts_provider.as_str()) {
            tracing::warn!(
                stale = %self.tts_provider,
                "tts_provider migrated to 'local' (cloud TTS providers are no longer supported)"
            );
            self.tts_provider = "local".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = AppConfig::default();
        assert_eq!(config.theme, Theme::Light);
        assert_eq!(config.language, "en-US");
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.channels, 1);
        assert_eq!(config.ai_provider, "lmstudio");
        assert_eq!(config.ai_model, "gpt-4o");
        assert_eq!(config.whisper_model, "large-v3-turbo");
        assert_eq!(config.tts_provider, "local");
        assert_eq!(config.tts_voice, "default");
        assert!((config.temperature - 0.2).abs() < f32::EPSILON);
        assert!(!config.auto_generate_referral);
        assert!(!config.auto_generate_letter);
        assert!(!config.auto_generate_soap);
        assert!(config.auto_index_rag);
        assert_eq!(config.icd_version, IcdVersion::Icd9);
        assert_eq!(config.soap_template, SoapTemplate::FollowUp);
        assert_eq!(config.embedding_model, "text-embedding-3-small");
        assert_eq!(config.search_top_k, 5);
        assert!((config.mmr_lambda - 0.7).abs() < f32::EPSILON);
        assert!(config.autosave_enabled);
        assert_eq!(config.autosave_interval_secs, 60);
        assert!(config.quick_continue_mode);
        assert_eq!(config.max_background_workers, 2);
        assert!(config.show_processing_notifications);
        assert!(config.auto_retry_failed);
        assert_eq!(config.max_retry_attempts, 3);
        assert_eq!(config.window_width, 1200);
        assert_eq!(config.window_height, 800);
        assert_eq!(config.lmstudio_host, "localhost");
        assert_eq!(config.lmstudio_port, 1234);
        assert!(config.vocabulary_enabled);
        assert_eq!(config.rsvp_wpm, 300);
        assert_eq!(config.rsvp_font_size, 48);
        assert_eq!(config.rsvp_chunk_size, 1);
        assert!(config.rsvp_dark_theme);
        assert!(!config.rsvp_show_context);
        assert!(!config.rsvp_audio_cue);
        assert!(config.rsvp_auto_start);
        assert!(!config.rsvp_remember_sections);
        assert!(config.rsvp_remembered_sections.is_empty());
    }

    #[test]
    fn round_trip_json() {
        let config = AppConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ai_provider, config.ai_provider);
        assert_eq!(back.theme, config.theme);
        assert_eq!(back.window_width, config.window_width);
    }

    #[test]
    fn partial_json_deserialize() {
        // Only override a few fields; all others should use defaults.
        let json = r#"{"ai_provider": "anthropic", "window_width": 1920}"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.ai_provider, "anthropic");
        assert_eq!(config.window_width, 1920);
        // Defaults still in place
        assert_eq!(config.language, "en-US");
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.tts_provider, "local");
    }

    #[test]
    fn theme_serialization() {
        let dark = Theme::Dark;
        let json = serde_json::to_value(&dark).unwrap();
        assert_eq!(json, "dark");

        let light: Theme = serde_json::from_str("\"light\"").unwrap();
        assert_eq!(light, Theme::Light);
    }

    #[test]
    fn icd_version_serialization() {
        let icd10 = IcdVersion::Icd10;
        let json = serde_json::to_value(&icd10).unwrap();
        assert_eq!(json, "icd10");

        let both: IcdVersion = serde_json::from_str("\"both\"").unwrap();
        assert_eq!(both, IcdVersion::Both);
    }

    #[test]
    fn soap_template_serialization() {
        let template = SoapTemplate::NewPatient;
        let json = serde_json::to_value(&template).unwrap();
        assert_eq!(json, "new_patient");

        let telehealth: SoapTemplate = serde_json::from_str("\"telehealth\"").unwrap();
        assert_eq!(telehealth, SoapTemplate::Telehealth);
    }

    #[test]
    fn context_templates_default_empty() {
        let config = AppConfig::default();
        assert!(config.custom_context_templates.is_empty());
    }

    #[test]
    fn context_template_round_trip() {
        let mut config = AppConfig::default();
        config.custom_context_templates = vec![
            ContextTemplate {
                name: "Follow-up".into(),
                body: "Follow-up visit.".into(),
            },
            ContextTemplate {
                name: "Telehealth".into(),
                body: "Video consult.".into(),
            },
        ];
        let json = serde_json::to_string(&config).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.custom_context_templates.len(), 2);
        assert_eq!(back.custom_context_templates[0].name, "Follow-up");
        assert_eq!(back.custom_context_templates[1].body, "Video consult.");
    }

    #[test]
    fn context_templates_missing_from_json_defaults_empty() {
        let json = r#"{"ai_provider": "openai"}"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert!(config.custom_context_templates.is_empty());
    }

    #[test]
    fn stale_ai_provider_migrates_to_lmstudio() {
        let json = r#"{"ai_provider": "anthropic"}"#;
        let mut config: AppConfig = serde_json::from_str(json).unwrap();
        config.migrate();
        assert_eq!(config.ai_provider, "lmstudio");
    }

    #[test]
    fn valid_ai_provider_not_changed_by_migrate() {
        let json = r#"{"ai_provider": "ollama"}"#;
        let mut config: AppConfig = serde_json::from_str(json).unwrap();
        config.migrate();
        assert_eq!(config.ai_provider, "ollama");
    }

    #[test]
    fn all_legacy_cloud_providers_migrate_to_lmstudio() {
        for legacy in ["openai", "anthropic", "gemini", "groq", "cerebras"] {
            let json = format!(r#"{{"ai_provider": "{legacy}"}}"#);
            let mut config: AppConfig = serde_json::from_str(&json).unwrap();
            config.migrate();
            assert_eq!(
                config.ai_provider, "lmstudio",
                "Expected '{legacy}' to migrate"
            );
        }
    }

    #[test]
    fn new_config_defaults_stt_mode_to_local() {
        let config: AppConfig = serde_json::from_str("{}").expect("parse empty");
        assert_eq!(config.stt_mode, SttMode::Local);
        assert_eq!(config.stt_remote_host, "");
        assert_eq!(config.stt_remote_port, 8080);
        assert_eq!(config.stt_remote_model, "whisper-1");
    }

    #[test]
    fn new_config_defaults_ollama_host_and_port() {
        let config: AppConfig = serde_json::from_str("{}").expect("parse empty");
        assert_eq!(config.ollama_host, "localhost");
        assert_eq!(config.ollama_port, 11434);
    }

    #[test]
    fn stt_mode_roundtrips_through_json() {
        let json = r#"{"stt_mode":"remote"}"#;
        let config: AppConfig = serde_json::from_str(json).expect("parse");
        assert_eq!(config.stt_mode, SttMode::Remote);

        let out = serde_json::to_string(&config).expect("serialize");
        assert!(
            out.contains(r#""stt_mode":"remote""#),
            "expected remote, got: {out}"
        );
    }

    #[test]
    fn capture_for_training_defaults_to_false_in_older_configs() {
        // Simulate an older config JSON missing the new field.
        let old_json = r#"{"ai_provider":"ollama","stt_mode":"local"}"#;
        let cfg: AppConfig =
            serde_json::from_str(old_json).expect("should parse with serde defaults");
        assert!(!cfg.capture_for_training, "default must be false");
    }

    #[test]
    fn sync_condition_chips_defaults_to_false_in_older_configs() {
        let old_json = r#"{"ai_provider":"ollama","stt_mode":"local"}"#;
        let cfg: AppConfig =
            serde_json::from_str(old_json).expect("should parse with serde defaults");
        assert!(!cfg.sync_condition_chips, "default must be false");
    }

    #[test]
    fn sync_content_defaults_to_false_in_older_configs() {
        let old_json = r#"{"ai_provider":"ollama","stt_mode":"local"}"#;
        let cfg: AppConfig =
            serde_json::from_str(old_json).expect("should parse with serde defaults");
        assert!(!cfg.sync_content, "default must be false");
    }

    #[test]
    fn capture_for_training_round_trips() {
        let mut cfg = AppConfig::default();
        cfg.capture_for_training = true;
        let json = serde_json::to_string(&cfg).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(back.capture_for_training);
    }

    #[test]
    fn allow_public_endpoint_defaults_to_false() {
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(!cfg.allow_public_endpoint);
    }

    #[test]
    fn ocr_model_defaults_to_none() {
        let json = r#"{"theme":"dark","language":"en"}"#;
        let config: AppConfig = serde_json::from_str(json).unwrap_or_default();
        assert!(config.ocr_model.is_none(), "ocr_model should default to None");
    }
}
