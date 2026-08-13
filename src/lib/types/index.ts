// ── Processing Status ──────────────────────────────────────────────────────────

export type ProcessingStatus =
  | { status: 'pending' }
  | { status: 'processing'; started_at: string }
  | { status: 'completed'; completed_at: string }
  | { status: 'failed'; error: string; retry_count: number };

// ── Patient Context ────────────────────────────────────────────────────────────

export interface PatientContext {
  patient_name?: string | null;
  prior_soap_notes?: string[];
  medications: string[];
  conditions: string[];
  allergies: string[];
}

// ── Recording ─────────────────────────────────────────────────────────────────

export interface Recording {
  id: string;
  filename: string;
  transcript: string | null;
  soap_note: string | null;
  referral: string | null;
  letter: string | null;
  peer_discussion: string | null;
  chat: string | null;
  patient_name: string | null;
  audio_path: string;
  duration_seconds: number | null;
  file_size_bytes: number | null;
  stt_provider: string | null;
  ai_provider: string | null;
  tags: string[];
  status: ProcessingStatus;
  created_at: string;
  metadata: {
    context?: string;
    patient_context?: PatientContext;
    [key: string]: unknown;
  } | null;
}

// ── Recording Summary ─────────────────────────────────────────────────────────

export interface RecordingSummary {
  id: string;
  filename: string;
  patient_name: string | null;
  status: ProcessingStatus;
  duration_seconds: number | null;
  created_at: string;
  tags: string[];
  has_transcript: boolean;
  has_soap_note: boolean;
  has_referral: boolean;
  has_letter: boolean;
  has_peer_discussion: boolean;
  is_remote: boolean;
}

// ── Context Template ──────────────────────────────────────────────────────────

export type { ContextTemplate } from '../api/contextTemplates';
import type { ContextTemplate } from '../api/contextTemplates';

// ── App Config ────────────────────────────────────────────────────────────────

// The Rust AppConfig (crates/core/src/types/settings.rs) defines additional
// fields the frontend doesn't touch (channels, window_width/height, soap_note_settings,
// agent_settings, soap_template, embedding_model, quick_continue_mode,
// max_background_workers, show_processing_notifications, auto_retry_failed,
// max_retry_attempts, auto_generate_referral/letter, auto_index_rag). They round-
// trip through this type without being modeled here; if the frontend ever reads
// or writes one, add it here so the typo guard kicks in.
export interface AppConfig {
  theme: 'dark' | 'light';
  language: string;
  storage_path: string | null;
  ai_provider: string;
  ai_model: string;
  /** Optional vision model for OCR (extracting text from dropped documents).
   *  Mirrors `ocr_model: Option<String>` in crates/core/src/types/settings.rs.
   *  When null, the generation model (`ai_model`) is used. */
  ocr_model: string | null;
  whisper_model: string;
  tts_provider: string;
  tts_voice: string;
  lmstudio_host: string;
  lmstudio_port: number;
  stt_mode: 'local' | 'remote';
  stt_remote_host: string;
  stt_remote_port: number;
  stt_remote_model: string;
  ollama_host: string;
  ollama_port: number;
  temperature: number;
  input_device: string | null;
  sample_rate: number;
  autosave_enabled: boolean;
  autosave_interval_secs: number;
  auto_generate_soap: boolean;
  /** Play a short local chime when a SOAP note finishes generating.
   *  Mirrors `soap_notification_sound: bool` in crates/core/src/types/settings.rs. */
  soap_notification_sound: boolean;
  search_top_k: number;
  mmr_lambda: number;
  vocabulary_enabled: boolean;
  medical_dict_enabled: boolean;
  max_speakers: number | null;
  custom_context_templates: ContextTemplate[];
  custom_soap_prompt: string | null;
  custom_referral_prompt: string | null;
  custom_letter_prompt: string | null;
  custom_synopsis_prompt: string | null;
  custom_peer_discussion_prompt: string | null;
  // ICD coding version (drives prompt + chip validation behavior).
  // Mirrors IcdVersion enum in crates/core/src/types/settings.rs (snake_case).
  icd_version: 'icd9' | 'icd10' | 'both';
  // RSVP speed-reader
  rsvp_wpm: number;
  rsvp_font_size: number;
  rsvp_chunk_size: number;
  rsvp_dark_theme: boolean;
  rsvp_show_context: boolean;
  rsvp_audio_cue: boolean;
  rsvp_auto_start: boolean;
  rsvp_remember_sections: boolean;
  rsvp_remembered_sections: string[];
  // Training corpus
  capture_for_training: boolean;
  // Security
  allow_public_endpoint: boolean;
  // Onboarding
  onboarding_completed: boolean;
  // Updates
  auto_update_check: boolean;
  // Quick-add condition chips
  custom_conditions: string[];
  /** When true, condition chip presets sync two-way with the paired server. Defaults to false. */
  sync_condition_chips: boolean;
  /** When true, patient content (transcripts, SOAP notes, letters, peer discussions)
   *  syncs two-way with the paired server over Tailscale. Audio is archived on the
   *  server and fetched on demand. Defaults to false. */
  sync_content: boolean;
}

// ── Chat ──────────────────────────────────────────────────────────────────────

/** Output of a single tool call. Mirrors `crates/core/src/types/agent.rs::ToolOutput`. */
export interface ToolOutput {
  content: string;
  is_error: boolean;
}

/** Token-usage stats for a completion. Mirrors `crates/core/src/types/ai.rs::UsageInfo`. */
export interface UsageInfo {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

/**
 * Record of a single tool invocation made during an agent run.
 * Mirrors `crates/core/src/types/agent.rs::AgentToolCallRecord`.
 * `arguments` is structured JSON from the model — typed as `unknown` so
 * call sites must narrow before use.
 */
export interface ToolCallRecord {
  tool_name: string;
  arguments: unknown;
  result: ToolOutput;
  duration_ms: number;
}

/** Final response from a `chat_with_agent` invocation. Mirrors `AgentResponse`. */
export interface AgentResponse {
  content: string;
  tool_calls_made: ToolCallRecord[];
  usage: UsageInfo;
  iterations: number;
}

export interface ChatMessage {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: string;
  agent?: string;
  tool_calls?: ToolCallRecord[];
}

// ── Processing Events ─────────────────────────────────────────────────────────

export type ProcessingEvent =
  | { type: 'started'; recording_id: string }
  | { type: 'progress'; recording_id: string; step: string; percent: number }
  | { type: 'completed'; recording_id: string }
  | { type: 'failed'; recording_id: string; error: string }
  | { type: 'queue_update'; pending: number; processing: number; completed: number };

// ── Audio Device ──────────────────────────────────────────────────────────────

export interface AudioDevice {
  name: string;
  is_input: boolean;
  is_default: boolean;
  sample_rates: number[];
  channels: number;
}
