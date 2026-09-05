//! Tauri commands for the Translate tab — live bidirectional conversation
//! translation between a clinician and a patient.
//!
//! The flow per utterance: tap-to-talk capture (`translation_capture_start` /
//! `translation_capture_stop`) writes a throwaway WAV under the app data dir
//! (never the recordings DB — translation utterances are ephemeral), which is
//! then transcribed by the active STT provider with the speaker's language as
//! the hint and translated to the other party's language by the active local
//! AI provider via `medical_translation::AiTranslationProvider`. A typed-text
//! fallback (`translation_text_utterance`) skips the audio leg.
//!
//! Translation capture shares `AppState::recording_active` with medical
//! recordings so the two can never run concurrently — cpal capture is
//! single-slot app-wide.
//!
//! PHI discipline: transcripts and translations never appear in logs or
//! events — only lengths, language codes, and speaker labels.

use std::sync::Arc;
use std::time::Instant;

use tauri::Emitter;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use medical_audio::capture::CaptureConfig;
use medical_audio::device::get_input_device;
use medical_core::error::{AppError, AppResult};
use medical_core::traits::translation::Language;
use medical_core::traits::{AiProvider, TranslationProvider, TtsProvider};
use medical_core::types::stt::SttConfig;
use medical_core::types::tts::TtsConfig;
use medical_translation::ai_translator::AiTranslationProvider;
use medical_translation::session::{
    Speaker, TranslationEntry, TranslationMode, TranslationSession,
};

use crate::commands::load_app_config;
use crate::commands::transcription::helpers::load_wav_to_audio_data;
use crate::state::{AppState, SendCaptureHandle, TranslationCapture};

// ─────────────────────────────────────────────────────────────────────────────
// Languages / session lifecycle
// ─────────────────────────────────────────────────────────────────────────────

/// Languages supported for translation. Static list from the
/// `medical-translation` crate — no provider needed.
#[tauri::command]
pub fn translation_supported_languages() -> AppResult<Vec<Language>> {
    Ok(medical_translation::supported_languages())
}

/// Start (or restart) a translation conversation session.
///
/// `provider_lang` is what the clinician speaks (the session's source
/// language); `patient_lang` is what the patient speaks (target).
#[tauri::command]
#[instrument(skip(state), name = "translation::start_session")]
pub async fn translation_start_session(
    state: tauri::State<'_, AppState>,
    patient_lang: String,
    provider_lang: String,
) -> AppResult<TranslationSession> {
    translation_start_session_inner(&state, &patient_lang, &provider_lang).await
}

/// Testable core of [`translation_start_session`].
pub async fn translation_start_session_inner(
    state: &AppState,
    patient_lang: &str,
    provider_lang: &str,
) -> AppResult<TranslationSession> {
    let (patient_lang, provider_lang) = (patient_lang.trim(), provider_lang.trim());
    if patient_lang.is_empty() || provider_lang.is_empty() {
        return Err(AppError::InvalidInput(
            "Both patient and provider languages must be set".to_string(),
        ));
    }
    if patient_lang == provider_lang {
        return Err(AppError::InvalidInput(
            "Patient and provider languages must differ".to_string(),
        ));
    }

    let session = TranslationSession::new(
        provider_lang.to_string(),
        patient_lang.to_string(),
        TranslationMode::Bidirectional,
    );
    let mut translation = state.translation.lock().await;
    if translation.capture.is_some() {
        return Err(AppError::translation(
            "An utterance capture is in progress — stop it before restarting the session"
                .to_string(),
        ));
    }
    if translation.in_flight > 0 {
        return Err(AppError::translation(
            "An utterance is still being translated — wait for it before changing languages"
                .to_string(),
        ));
    }
    info!(
        provider_lang = %session.source_lang,
        patient_lang = %session.target_lang,
        "Translation session started"
    );
    translation.session = Some(session.clone());
    Ok(session)
}

/// Fetch the active session (history included) for UI rehydration.
/// `None` when no session has been started.
#[tauri::command]
pub async fn translation_get_session(
    state: tauri::State<'_, AppState>,
) -> AppResult<Option<TranslationSession>> {
    Ok(state.translation.lock().await.session.clone())
}

/// Clear the session history (rejects while a capture or a translation is
/// mid-flight).
#[tauri::command]
#[instrument(skip(state), name = "translation::clear_session")]
pub async fn translation_clear_session(state: tauri::State<'_, AppState>) -> AppResult<()> {
    translation_clear_session_inner(&state).await
}

/// Testable core of [`translation_clear_session`].
pub async fn translation_clear_session_inner(state: &AppState) -> AppResult<()> {
    let mut translation = state.translation.lock().await;
    if translation.capture.is_some() {
        return Err(AppError::translation(
            "An utterance capture is in progress — stop it before clearing the session".to_string(),
        ));
    }
    if translation.in_flight > 0 {
        return Err(AppError::translation(
            "An utterance is still being translated — wait for it before clearing the session"
                .to_string(),
        ));
    }
    translation.session = None;
    info!("Translation session cleared");
    Ok(())
}

/// Export the session as a plain-text transcript (`session.export_text`).
#[tauri::command]
pub async fn translation_export_session(state: tauri::State<'_, AppState>) -> AppResult<String> {
    let translation = state.translation.lock().await;
    translation
        .session
        .as_ref()
        .map(|s| s.export_text())
        .ok_or_else(|| AppError::translation("No translation session is active".to_string()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Translate + record (shared by the audio and typed-input paths)
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve the active AI provider as an `Arc`, releasing the registry lock
/// before any `.await` (the `chat.rs` pattern).
async fn active_ai_provider(state: &AppState) -> AppResult<Arc<dyn AiProvider>> {
    state
        .ai_providers
        .lock()
        .await
        .get_active_arc()
        .ok_or_else(|| {
            AppError::ai_provider(
                "No active AI provider configured. Configure one in Settings → Models.".to_string(),
            )
        })
}

/// The model translation requests are sent to: the per-feature
/// `translation_model` override when set (non-empty), else the global
/// `ai_model` — the OCR fallback pattern.
fn translation_model_from_config(config: &medical_core::types::settings::AppConfig) -> String {
    config
        .translation_model
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| config.ai_model.clone())
}

/// Content of the keep-alive ping fired while the user is still speaking.
/// A FIXED LITERAL — never patient or clinician content (PHI discipline).
const KEEPALIVE_PROMPT: &str = "ping";

/// One-token completion sent during `translation_capture_start` so the
/// local server (Ollama et al.) pages the translation model into memory
/// while the user is still talking. The result is discarded; failures are
/// the caller's to log and ignore.
///
/// Thinking is opted out (`reasoning_effort: "none"`) so reasoning models
/// don't burn a CoT preamble on the ping itself.
async fn llm_keepalive_ping(provider: Arc<dyn AiProvider>, model: String) -> AppResult<()> {
    let request = medical_core::types::CompletionRequest {
        model,
        messages: vec![medical_core::types::Message {
            role: medical_core::types::Role::User,
            content: medical_core::types::MessageContent::Text(KEEPALIVE_PROMPT.to_string()),
            tool_calls: vec![],
        }],
        temperature: Some(0.0),
        max_tokens: Some(1),
        system_prompt: None,
        reasoning_effort: Some("none".to_string()),
    };
    provider.complete(request).await.map(|_| ())
}

/// Fire-and-forget prewarm while the user speaks: load the whisper model
/// into the provider's context cache and page the translation model into
/// the AI server. Both run concurrently with the capture and neither may
/// affect it — errors are logged and dropped.
///
/// Takes the already-resolved `Arc`s (not `&AppState`) so it can be spawned
/// onto a detached task without borrowing command state.
fn spawn_translation_prewarm(
    stt: Arc<dyn medical_core::traits::SttProvider + Send + Sync>,
    ai: Arc<dyn AiProvider>,
    translation_model: String,
) {
    tokio::spawn(async move {
        // Concurrent, not sequential: the two warm different resources, and
        // a multi-second whisper load must not delay the LLM page-in past
        // the user's stop-tap.
        let (stt_res, ping_res) =
            tokio::join!(stt.prewarm(), llm_keepalive_ping(ai, translation_model));
        if let Err(e) = stt_res {
            tracing::debug!(error = %e, "Translation STT prewarm failed (advisory)");
        }
        if let Err(e) = ping_res {
            tracing::debug!(error = %e, "Translation LLM keep-alive failed (advisory)");
        }
    });
}

/// Translate `original` in the direction implied by `speaker` and append the
/// entry to the session. Testable core shared by both utterance paths.
///
/// Marks the utterance as in-flight for the duration of the LLM call so
/// clear/restart reject instead of racing the push (see
/// `TranslationState::in_flight`).
async fn translate_and_record(
    state: &AppState,
    speaker: Speaker,
    original: &str,
) -> AppResult<TranslationEntry> {
    let original = original.trim();
    if original.is_empty() {
        return Err(AppError::InvalidInput(
            "Nothing to translate — the text is empty".to_string(),
        ));
    }

    let (source, target) = {
        let mut translation = state.translation.lock().await;
        let session = translation
            .session
            .as_ref()
            .ok_or_else(|| AppError::translation("No translation session is active".to_string()))?;
        let pair = match speaker {
            Speaker::Provider => (session.source_lang.clone(), session.target_lang.clone()),
            Speaker::Patient => (session.target_lang.clone(), session.source_lang.clone()),
        };
        translation.in_flight += 1;
        pair
    };

    // Everything below must decrement in_flight on EVERY exit path before
    // returning — hence the single fallible stretch feeding one epilogue
    // instead of early `?` returns past the increment.
    let outcome = translate_utterance(state, original, &source, &target).await;

    let entry = {
        let mut translation = state.translation.lock().await;
        translation.in_flight = translation.in_flight.saturating_sub(1);
        match outcome {
            Ok(translated) => {
                let entry = TranslationEntry {
                    original: original.to_string(),
                    translated,
                    source_lang: source,
                    target_lang: target,
                    timestamp: chrono::Utc::now(),
                    speaker: speaker.clone(),
                };
                // clear/restart were rejected while in_flight > 0, so the
                // session can't have vanished or been replaced here.
                let session = translation.session.as_mut().ok_or_else(|| {
                    AppError::translation("No translation session is active".to_string())
                })?;
                session.history.push(entry.clone());
                Ok(entry)
            }
            Err(e) => Err(e),
        }
    }?;

    info!(
        speaker = ?entry.speaker,
        source_lang = %entry.source_lang,
        target_lang = %entry.target_lang,
        original_len = entry.original.len(),
        translated_len = entry.translated.len(),
        "Utterance translated"
    );
    Ok(entry)
}

/// The lock-free middle of [`translate_and_record`]: resolve the provider,
/// honor the configured model, and run the translation.
async fn translate_utterance(
    state: &AppState,
    original: &str,
    source: &str,
    target: &str,
) -> AppResult<String> {
    let provider = active_ai_provider(state).await?;
    // The provider stack resolves no default of its own — an empty model
    // goes on the wire verbatim and Ollama rejects it. Send the configured
    // model like every other AI command (the `chat.rs` convention).
    let model = {
        let config = load_app_config(&state.db, "translation").await?;
        translation_model_from_config(&config)
    };
    let translator = AiTranslationProvider::with_model(provider, model);
    let translated = translator
        .translate(original, Some(source), target)
        .await?
        .trim()
        .to_string();
    if translated.is_empty() {
        return Err(AppError::translation(
            "The AI provider returned an empty translation".to_string(),
        ));
    }
    Ok(translated)
}

/// Typed-input fallback: translate text the user typed (or pasted) as the
/// given speaker, bypassing audio capture entirely.
#[tauri::command]
#[instrument(skip(state), name = "translation::text_utterance", fields(speaker = ?speaker))]
pub async fn translation_text_utterance(
    state: tauri::State<'_, AppState>,
    speaker: Speaker,
    text: String,
) -> AppResult<TranslationEntry> {
    translate_and_record(&state, speaker, &text).await
}

// ─────────────────────────────────────────────────────────────────────────────
// Tap-to-talk capture
// ─────────────────────────────────────────────────────────────────────────────

/// Begin capturing an utterance from the configured input device for the
/// given speaker. Fails if a medical recording (or another translation
/// capture) is already in progress — audio capture is single-slot app-wide.
#[tauri::command]
#[instrument(skip(app, state), name = "translation::capture_start", fields(speaker = ?speaker))]
pub async fn translation_capture_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    speaker: Speaker,
) -> AppResult<()> {
    // Atomically claim the shared capture slot (same flag as medical
    // recordings — the two are mutually exclusive).
    {
        let mut active = state.recording_active.lock().await;
        if *active {
            return Err(AppError::audio(
                "Audio capture is already in progress".to_string(),
            ));
        }
        *active = true;
    }

    // Reset the flag on any error so the user isn't locked out of capture.
    macro_rules! try_or_release {
        ($state:expr, $expr:expr) => {
            match $expr {
                Ok(v) => v,
                Err(e) => {
                    *$state.recording_active.lock().await = false;
                    return Err(e);
                }
            }
        };
    }

    // A session must exist — it defines the speaker's language (STT hint)
    // and the translation direction.
    {
        let translation = state.translation.lock().await;
        if translation.session.is_none() {
            try_or_release!(
                state,
                Err(AppError::translation(
                    "No translation session is active — pick both languages first".to_string()
                ))
            );
        }
    }

    // Read the configured input device and sample rate from settings —
    // the Translate tab deliberately uses the same device as recordings.
    let (input_device_name, sample_rate) = try_or_release!(state, {
        let config = load_app_config(&state.db, "translation").await?;
        AppResult::Ok((
            config.input_device.filter(|s| !s.is_empty()),
            config.sample_rate,
        ))
    });

    // Throwaway WAV under the app data dir (not /tmp — world-readable on
    // Linux; not the recordings dir — would trip the sync/orphan sweeps).
    // Deleted the moment the samples have been read out in capture_stop.
    let wav_dir = try_or_release!(state, {
        let dir = state.data_dir.join("translation");
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::audio(format!("create translation dir: {e}")))?;
        AppResult::Ok(dir)
    });
    let wav_path = wav_dir.join(format!("utterance-{}.wav", Uuid::new_v4()));

    // Start capture on a dedicated std::thread so the !Send CaptureHandle
    // never crosses a thread boundary (same pattern as `start_recording`).
    let wav_path_for_capture = wav_path.clone();
    let (tx, rx) = std::sync::mpsc::channel::<
        Result<(SendCaptureHandle, std::sync::mpsc::Receiver<Vec<f32>>), AppError>,
    >();
    std::thread::spawn(move || {
        let result = (|| {
            let device = get_input_device(input_device_name.as_deref())
                .map_err(|e| AppError::audio_with_source(e.to_string(), e))?;
            let config = CaptureConfig {
                sample_rate,
                ..CaptureConfig::default()
            };
            let (handle, waveform_rx) =
                medical_audio::capture::start_capture(&device, config, &wav_path_for_capture)
                    .map_err(|e| AppError::audio_with_source(e.to_string(), e))?;
            Ok((SendCaptureHandle(Some(handle), None), waveform_rx))
        })();
        let _ = tx.send(result);
    });

    let (send_handle, waveform_rx) = try_or_release!(
        state,
        tokio::task::spawn_blocking(move || {
            rx.recv()
                .map_err(|_| AppError::audio("Audio capture thread panicked".to_string()))
                .and_then(|r| r)
        })
        .await
        .map_err(|e| AppError::audio(format!("capture join: {e}")))?
    );

    info!(
        speaker = ?speaker,
        wav_path = %wav_path.display(),
        sample_rate,
        "Translation utterance capture started"
    );

    // Publish the capture. The session is re-checked UNDER THE SAME LOCK that
    // stores the capture: `clear_session`/`start_session` take this lock too,
    // so they either ran before us (session gone → we clean up and fail) or
    // will find `capture.is_some()` and reject. This closes the window where
    // an early session check let a capture be stored for a vanished session.
    let mut prepared = Some(TranslationCapture {
        handle: send_handle,
        wav_path,
        speaker: speaker.clone(),
        started_at: Instant::now(),
    });
    let stashed = {
        let mut translation = state.translation.lock().await;
        if translation.session.is_some() {
            translation.capture = prepared.take();
            true
        } else {
            false
        }
    };
    if !stashed {
        // We own a live capture nobody references — stop it on a blocking
        // worker, delete its WAV, and release the capture slot.
        let leftover = prepared.take().expect("capture not stashed");
        let handle = leftover.handle;
        tokio::task::spawn_blocking(move || drop(handle))
            .await
            .map_err(|e| AppError::Other(format!("Stop task panicked: {e}")))?;
        let _ = std::fs::remove_file(leftover.wav_path);
        *state.recording_active.lock().await = false;
        return Err(AppError::translation(
            "No translation session is active — pick both languages first".to_string(),
        ));
    }

    // Waveform loop for the live level meter — same `waveform-data` event
    // name as recordings so the frontend meter works unchanged. Exits when
    // the capture is dropped (channel closes) in capture_stop.
    tokio::task::spawn_blocking(move || {
        while let Ok(data) = waveform_rx.recv() {
            let _ = app.emit("waveform-data", &data);
        }
    });

    // Prewarm while the user speaks: load the whisper context and page the
    // translation model into the AI server. Best-effort on every axis —
    // missing providers/models and failures are logged and dropped inside
    // the detached task; a failed prewarm never blocks the capture. The
    // provider Arc is cloned out of the lock BEFORE any await (the
    // `chat.rs` discipline) so reinits/downloads aren't blocked on the
    // config read.
    let stt = state.stt_providers.lock().await.clone();
    if let Some(stt) = stt
        && let Ok(config) = load_app_config(&state.db, "translation").await
        && let Ok(ai) = active_ai_provider(&state).await
    {
        spawn_translation_prewarm(stt, ai, translation_model_from_config(&config));
    }

    Ok(())
}

/// Result of [`translation_capture_stop`].
///
/// `entry: None` + a human-readable `note` marks an EXPECTED unusable
/// capture (mistimed tap, silence, nothing transcribed) — the frontend
/// renders those as a soft, auto-dismissing notice. Real failures stay
/// `Err` and surface as errors.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CaptureStopResult {
    pub entry: Option<TranslationEntry>,
    pub note: Option<String>,
}

impl CaptureStopResult {
    fn with_entry(entry: TranslationEntry) -> Self {
        Self {
            entry: Some(entry),
            note: None,
        }
    }

    fn note(note: &str) -> Self {
        Self {
            entry: None,
            note: Some(note.to_string()),
        }
    }
}

/// Stop the in-flight capture, transcribe it, translate it, and return the
/// recorded entry. The temp WAV is deleted as soon as its samples are read.
#[tauri::command]
#[instrument(skip(app, state), name = "translation::capture_stop")]
pub async fn translation_capture_stop(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> AppResult<CaptureStopResult> {
    // Take the capture out of state. The shared `recording_active` flag is
    // cleared ONLY when a translation capture was actually taken — a stray
    // stop while a MEDICAL recording runs must not unlock its capture slot.
    let capture = {
        let mut translation = state.translation.lock().await;
        translation.capture.take()
    };
    let capture = match capture {
        Some(c) => {
            *state.recording_active.lock().await = false;
            c
        }
        None => {
            return Err(AppError::audio(
                "No translation capture is in progress".to_string(),
            ));
        }
    };
    info!(
        speaker = ?capture.speaker,
        capture_secs = capture.started_at.elapsed().as_secs_f64(),
        "Translation utterance capture stopped"
    );

    // Short-tap guard: a sub-second capture is a mistimed tap, not speech —
    // skip the (multi-second) STT call entirely. An expected outcome, not a
    // failure: surfaced as a note, not an error.
    let capture_secs = capture.started_at.elapsed().as_secs_f64();
    if capture_secs < 0.5 {
        let leftover_path = capture.wav_path;
        tokio::task::spawn_blocking(move || drop(capture.handle))
            .await
            .map_err(|e| AppError::Other(format!("Stop task panicked: {e}")))?;
        let _ = std::fs::remove_file(leftover_path);
        return Ok(CaptureStopResult::note(
            "That tap was too short — hold to speak and tap again to stop",
        ));
    }

    // Drop the wrapper on a blocking worker so CaptureHandle::drop (which
    // joins the drain thread and finalizes the WAV) doesn't block the
    // async runtime.
    let wrapper = capture.handle;
    tokio::task::spawn_blocking(move || drop(wrapper))
        .await
        .map_err(|e| AppError::Other(format!("Stop task panicked: {e}")))?;

    // Load samples, then delete the throwaway WAV immediately — the file
    // only ever exists for the duration of one utterance.
    let audio = {
        let wav_path = capture.wav_path.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            let result = load_wav_to_audio_data(&wav_path);
            let _ = std::fs::remove_file(&wav_path);
            result
        })
        .await
        .map_err(crate::commands::join_err)?;
        loaded?
    };

    if audio.samples.is_empty() {
        return Ok(CaptureStopResult::note(
            "No audio was captured — check the microphone and try again",
        ));
    }

    // Silence guard before the (multi-second) STT call: whisper hallucinates
    // plausible text on near-silence, which would then be translated as if
    // the patient said it. Same threshold as the transcription pipeline.
    let rms = ((audio
        .samples
        .iter()
        .map(|s| {
            let f = *s as f64;
            f * f
        })
        .sum::<f64>()
        / audio.samples.len() as f64)
        .sqrt()) as f32;
    if rms < 0.001 {
        return Ok(CaptureStopResult::note(
            "No speech was detected — the microphone picked up silence",
        ));
    }

    let speaker_language = state
        .translation
        .lock()
        .await
        .speaker_language(capture.speaker.clone())
        .ok_or_else(|| AppError::translation("No translation session is active".to_string()))?;

    // ── transcribe ──
    let _ = app.emit("translation-progress", "transcribing");
    let stt: Arc<dyn medical_core::traits::SttProvider + Send + Sync> = {
        let guard = state.stt_providers.lock().await;
        guard.as_ref().cloned().ok_or_else(|| {
            AppError::stt_provider(
                "No STT provider configured. Download a Whisper model in Settings → Audio / STT."
                    .to_string(),
            )
        })?
    };
    let config = SttConfig {
        language: Some(speaker_language),
        diarize: false,
        ..SttConfig::default()
    };
    let transcript = stt
        .transcribe(audio, config, CancellationToken::default())
        .await?;
    let original = transcript.text.trim().to_string();
    info!(transcript_len = original.len(), "Utterance transcribed");
    if original.is_empty() {
        return Ok(CaptureStopResult::note(
            "No speech was detected in that utterance — try speaking again",
        ));
    }

    // ── translate ──
    let _ = app.emit("translation-progress", "translating");
    let entry = translate_and_record(&state, capture.speaker.clone(), &original).await?;
    let _ = app.emit("translation-progress", "complete");
    Ok(CaptureStopResult::with_entry(entry))
}

// ─────────────────────────────────────────────────────────────────────────────
// Speak (local OS TTS)
// ─────────────────────────────────────────────────────────────────────────────

/// macOS gimmick voices (Bad News, Bells, Bahh, Whisper, …) live under this
/// identifier prefix. They report ordinary language tags and sort BEFORE the
/// real voices (`com.apple.speech.*` < `com.apple.voice.*`), so an
/// unfiltered first-match picks one — and renders speech through an effect
/// pipeline that is unintelligible.
const NOVELTY_VOICE_ID_PREFIX: &str = "com.apple.speech.synthesis.voice";

/// Lower rank is better. Enhanced/premium are the downloadable
/// high-quality voices; compact/siri are the built-ins; anything else
/// (Windows SAPI tokens, etc.) ranks last and falls back to first-match
/// order within that tier.
fn voice_quality_rank(id: &str) -> u8 {
    if id.contains(".enhanced.") || id.contains(".premium.") {
        0
    } else if id.contains(".compact.") || id.contains(".siri.") {
        1
    } else {
        2
    }
}

/// Pick the OS voice that speaks `language` (BCP-47 base code prefix match
/// — `"zh"` matches `"zh-CN"`). Prefers the highest-quality non-novelty
/// voice; ties keep the engine's list order (deterministic). Returns `None`
/// when the system has no usable voice for that language.
fn pick_voice_for_language<'a>(
    voices: &'a [medical_core::types::tts::VoiceInfo],
    language: &str,
) -> Option<&'a medical_core::types::tts::VoiceInfo> {
    voices
        .iter()
        .filter(|v| {
            v.language
                .as_deref()
                .is_some_and(|l| l.starts_with(language))
        })
        .filter(|v| !v.id.starts_with(NOVELTY_VOICE_ID_PREFIX))
        .min_by_key(|v| voice_quality_rank(&v.id))
}

/// Speak `text` aloud through the local OS speech engine in the given
/// language. Fire-and-forget: synthesis blocks until the utterance finishes,
/// so it runs on a spawned task (the provider serializes utterances on its
/// engine thread; it cannot interrupt one mid-speech).
#[tauri::command]
#[instrument(skip(state), name = "translation::speak", fields(text_len = text.len()))]
pub async fn translation_speak(
    state: tauri::State<'_, AppState>,
    text: String,
    language: String,
) -> AppResult<()> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(AppError::InvalidInput(
            "Nothing to speak — text is empty".to_string(),
        ));
    }
    let language = language.trim().to_string();
    if language.is_empty() {
        return Err(AppError::InvalidInput(
            "A language code is required to pick a voice".to_string(),
        ));
    }

    // Lazily initialise the OS speech engine (spawns its dedicated thread).
    let tts = {
        let mut guard = state
            .tts
            .lock()
            .map_err(|e| AppError::MutexPoisoned(format!("tts: {e}")))?;
        if guard.is_none() {
            *guard = Some(Arc::new(
                medical_tts_providers::local_tts::LocalTtsProvider::new(),
            ));
        }
        Arc::clone(guard.as_ref().expect("just initialised"))
    };

    // Pick an OS voice that speaks the requested language (the local
    // provider ignores TtsConfig::language, so the voice id is what steers
    // it). NO fallback to the default voice: a non-matching voice would
    // render the text in the wrong language's phonology (gibberish) —
    // erroring tells the clinician to install a voice instead.
    let voices = tts.available_voices().await?;
    let Some(voice) = pick_voice_for_language(&voices, &language) else {
        return Err(AppError::tts_provider(format!(
            "No {language} voice is available on this system — add one in the OS speech \
             settings to hear translations read aloud"
        )));
    };
    // Voice name + language only — never speech content (PHI discipline).
    info!(language = %language, voice = %voice.name, "TTS voice selected");
    let voice_id = voice.id.clone();

    tokio::spawn(async move {
        let config = TtsConfig {
            voice: Some(voice_id),
            language: None,
            speed: 1.0,
            volume: 1.0,
            model: None,
        };
        if let Err(e) = tts.synthesize(&text, config).await {
            warn!(error = %e, text_len = text.len(), "Local TTS speak failed");
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::generation::test_helpers::{
        MockCompletionProvider, build_test_state_with_provider,
    };
    use medical_core::error::AppResult as CoreResult;
    use medical_core::types::settings::AppConfig;

    /// Build a test AppState whose active AI provider is a
    /// `MockCompletionProvider` returning `translation`, and start an
    /// en(provider)↔es(patient) session.
    async fn state_with_session(translation: &str) -> AppState {
        let config = AppConfig {
            ai_provider: "mock".into(),
            ..Default::default()
        };
        let provider = Arc::new(MockCompletionProvider::new("mock", translation, 10));
        let (state, _) = build_test_state_with_provider(config, "", provider).await;
        translation_start_session_inner(&state, "es", "en")
            .await
            .expect("start session");
        state
    }

    /// An AI provider whose `complete()` blocks until the test releases it,
    /// recording every request. Lets tests observe the in-flight window of
    /// `translate_and_record` and assert the configured model (and other
    /// request fields) reached the wire.
    struct GatedProvider {
        release: Arc<tokio::sync::Notify>,
        requested: std::sync::Mutex<Vec<medical_core::types::CompletionRequest>>,
    }

    impl GatedProvider {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                release: Arc::new(tokio::sync::Notify::new()),
                requested: std::sync::Mutex::new(Vec::new()),
            })
        }

        /// Let the pending `complete()` call resolve.
        fn notify_complete(&self) {
            self.release.notify_one();
        }

        /// Model names recorded from every `complete()` request so far.
        fn requested_models(&self) -> Vec<String> {
            self.requests().iter().map(|r| r.model.clone()).collect()
        }

        /// Every `complete()` request seen so far, in order.
        fn requests(&self) -> Vec<medical_core::types::CompletionRequest> {
            self.requested.lock().expect("requests lock").clone()
        }
    }

    #[async_trait::async_trait]
    impl AiProvider for GatedProvider {
        fn name(&self) -> &str {
            "gated"
        }

        async fn available_models(&self) -> CoreResult<Vec<medical_core::types::ModelInfo>> {
            Ok(Vec::new())
        }

        async fn complete(
            &self,
            request: medical_core::types::CompletionRequest,
        ) -> CoreResult<medical_core::types::CompletionResponse> {
            self.requested.lock().expect("requests lock").push(request);
            self.release.notified().await;
            Ok(medical_core::types::CompletionResponse {
                content: "Translated".into(),
                model: String::new(),
                usage: medical_core::types::UsageInfo {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                    decode_tokens_per_second: None,
                },
                tool_calls: Vec::new(),
            })
        }

        async fn complete_stream(
            &self,
            _request: medical_core::types::CompletionRequest,
        ) -> CoreResult<
            Box<
                dyn futures_util::Stream<Item = CoreResult<medical_core::types::StreamChunk>>
                    + Send
                    + Unpin,
            >,
        > {
            Err(AppError::ai_provider(
                "not implemented in test mock".to_string(),
            ))
        }

        async fn complete_with_tools(
            &self,
            _request: medical_core::types::CompletionRequest,
            _tools: Vec<medical_core::types::ToolDef>,
        ) -> CoreResult<medical_core::types::ToolCompletionResponse> {
            Err(AppError::ai_provider(
                "not implemented in test mock".to_string(),
            ))
        }
    }

    #[test]
    fn supported_languages_command_returns_crate_list() {
        let langs = translation_supported_languages().expect("languages");
        assert!(langs.iter().any(|l| l.code == "en"));
        assert!(langs.iter().any(|l| l.code == "zh"));
    }

    /// A macOS-like voice list: novelty voices sort first, then compact,
    /// then enhanced — the ordering trap the picker has to survive.
    fn voice_fixture() -> Vec<medical_core::types::tts::VoiceInfo> {
        use medical_core::types::tts::VoiceInfo;

        let voice = |id: &str, lang: Option<&str>| VoiceInfo {
            id: id.to_string(),
            name: id.to_string(),
            language: lang.map(|l| l.to_string()),
            gender: None,
            preview_url: None,
        };
        vec![
            voice("com.apple.speech.synthesis.voice.Bells", Some("en-US")),
            voice("com.apple.speech.synthesis.voice.Bahh", Some("en-US")),
            voice("com.apple.voice.compact.en-US.Samantha", Some("en-US")),
            voice("com.apple.voice.enhanced.en-US.Zoe", Some("en-US")),
            voice("com.apple.voice.compact.zh-CN.Tingting", Some("zh-CN")),
            voice("com.apple.voice.enhanced.zh-CN.Tingting", Some("zh-CN")),
            voice("mystery", None),
        ]
    }

    #[test]
    fn pick_voice_prefix_matches_and_returns_none_without_a_match() {
        let voices = voice_fixture();

        // Base-code prefix match finds the regional variant (best quality)…
        assert_eq!(
            pick_voice_for_language(&voices, "zh").map(|v| v.id.as_str()),
            Some("com.apple.voice.enhanced.zh-CN.Tingting")
        );
        // …and an unmatched language yields None (caller errors — a wrong
        // voice would speak the text as gibberish).
        assert!(pick_voice_for_language(&voices, "ko").is_none());
        // Voices with no language tag never match by accident.
        assert!(pick_voice_for_language(&voices, "mystery").is_none());
        assert!(pick_voice_for_language(&[], "en").is_none());
    }

    /// Regression: on macOS the gimmick/novelty voices (Bad News, Bells,
    /// Bahh…) report ordinary language tags and sort BEFORE the real ones,
    /// so a first-match picker selected one and spoke through its effect
    /// pipeline — unintelligible audio.
    #[test]
    fn pick_voice_never_selects_a_macos_novelty_voice() {
        let voices = voice_fixture();
        // "Bells" is the first en-US voice in the fixture (real macOS list
        // order); the picker must skip it for the best real voice.
        let picked = pick_voice_for_language(&voices, "en").expect("an English voice");
        assert!(!picked.id.starts_with("com.apple.speech.synthesis.voice"));
        assert_eq!(picked.id, "com.apple.voice.enhanced.en-US.Zoe");
    }

    #[test]
    fn pick_voice_prefers_enhanced_over_compact() {
        let voices = voice_fixture();
        let picked = pick_voice_for_language(&voices, "zh").expect("a Chinese voice");
        // Tingting exists in both compact and enhanced forms; enhanced wins
        // even though compact appears first.
        assert!(picked.id.contains(".enhanced."));
    }

    #[tokio::test]
    async fn start_session_rejects_missing_or_equal_languages() {
        let config = AppConfig::default();
        let provider = Arc::new(MockCompletionProvider::new("mock", "x", 1));
        let (state, _) = build_test_state_with_provider(config, "", provider).await;

        let err = translation_start_session_inner(&state, "", "en")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));

        let err = translation_start_session_inner(&state, "en", "en")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));

        let session = translation_start_session_inner(&state, "es", "en")
            .await
            .expect("valid pair");
        assert_eq!(session.source_lang, "en");
        assert_eq!(session.target_lang, "es");
    }

    #[tokio::test]
    async fn text_utterance_translates_provider_direction() {
        let state = state_with_session("Hola").await;
        let entry = translate_and_record(&state, Speaker::Provider, "Hello")
            .await
            .expect("translate");

        // Provider speaks the session source language (en) → target (es).
        assert_eq!(entry.source_lang, "en");
        assert_eq!(entry.target_lang, "es");
        assert_eq!(entry.original, "Hello");
        assert_eq!(entry.translated, "Hola");
        assert_eq!(entry.speaker, Speaker::Provider);

        let session = state.translation.lock().await.session.clone().unwrap();
        assert_eq!(session.entry_count(), 1);
    }

    #[tokio::test]
    async fn text_utterance_translates_patient_direction() {
        let state = state_with_session("My head hurts").await;
        let entry = translate_and_record(&state, Speaker::Patient, "Me duele la cabeza")
            .await
            .expect("translate");

        // Patient speaks the session target language (es) → source (en).
        assert_eq!(entry.source_lang, "es");
        assert_eq!(entry.target_lang, "en");
        assert_eq!(entry.translated, "My head hurts");
    }

    #[tokio::test]
    async fn text_utterance_rejects_whitespace_only_text() {
        let state = state_with_session("Hola").await;
        let err = translate_and_record(&state, Speaker::Provider, "   ")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));
        // No entry recorded for the rejected utterance.
        let session = state.translation.lock().await.session.clone().unwrap();
        assert_eq!(session.entry_count(), 0);
    }

    #[tokio::test]
    async fn text_utterance_requires_a_session() {
        let config = AppConfig::default();
        let provider = Arc::new(MockCompletionProvider::new("mock", "x", 1));
        let (state, _) = build_test_state_with_provider(config, "", provider).await;

        let err = translate_and_record(&state, Speaker::Provider, "Hello")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Translation { .. }));
    }

    #[tokio::test]
    async fn export_and_clear_round_trip() {
        let state = state_with_session("Hola").await;
        translate_and_record(&state, Speaker::Provider, "Hello")
            .await
            .expect("first");
        translate_and_record(&state, Speaker::Patient, "Me duele la cabeza")
            .await
            .expect("second");

        let exported = state
            .translation
            .lock()
            .await
            .session
            .as_ref()
            .unwrap()
            .export_text();
        assert!(exported.contains("Provider"));
        assert!(exported.contains("en→es"));
        assert!(exported.contains("es→en"));

        // Clear resets to no session (and would reject mid-capture, tested
        // via the same lock — capture is None here).
        {
            let mut translation = state.translation.lock().await;
            translation.session = None;
        }
        assert!(state.translation.lock().await.session.is_none());
    }

    /// Regression (in-flight guard): while an utterance translation is
    /// between "language pair read" and "entry pushed", clear and restart
    /// must reject — otherwise the completed entry is dropped or lands in a
    /// replacement session with the wrong language pair.
    #[tokio::test]
    async fn clear_and_restart_reject_while_translation_in_flight() {
        let config = AppConfig {
            ai_provider: "gated".into(),
            ..Default::default()
        };
        let provider = GatedProvider::new();
        let (state, _) = build_test_state_with_provider(config, "", provider.clone()).await;
        let state = Arc::new(state);
        translation_start_session_inner(&state, "es", "en")
            .await
            .expect("start session");

        let task_state = Arc::clone(&state);
        let utterance = tokio::spawn(async move {
            translate_and_record(&task_state, Speaker::Provider, "Hello").await
        });

        // Wait for the utterance to enter its in-flight window (bounded so a
        // broken guard fails fast instead of hanging).
        for _ in 0..500 {
            if state.translation.lock().await.in_flight == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(state.translation.lock().await.in_flight, 1);

        let err = translation_clear_session_inner(&state).await.unwrap_err();
        assert!(matches!(err, AppError::Translation { .. }));
        let err = translation_start_session_inner(&state, "fr", "en")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Translation { .. }));

        // Release the provider and let the utterance finish — it must land
        // in the (unchanged) session, not be dropped.
        provider.notify_complete();
        let entry = utterance
            .await
            .expect("task join")
            .expect("translation completes");
        assert_eq!(entry.translated, "Translated");
        assert_eq!(state.translation.lock().await.in_flight, 0);
        assert_eq!(
            state
                .translation
                .lock()
                .await
                .session
                .as_ref()
                .unwrap()
                .entry_count(),
            1
        );

        // With nothing in flight, clear succeeds.
        translation_clear_session_inner(&state)
            .await
            .expect("clear after completion");
        assert!(state.translation.lock().await.session.is_none());
    }

    /// Regression (model threading): the command layer must send the
    /// configured `ai_model` on translation requests — an empty model goes
    /// on the wire verbatim and Ollama rejects it.
    #[tokio::test]
    async fn text_utterance_sends_the_configured_model() {
        let config = AppConfig {
            ai_provider: "gated".into(),
            ai_model: "qwen3:8b".into(),
            ..Default::default()
        };
        let provider = GatedProvider::new();
        let (state, _) = build_test_state_with_provider(config, "", provider.clone()).await;
        let state = Arc::new(state);
        translation_start_session_inner(&state, "es", "en")
            .await
            .expect("start session");

        let task_state = Arc::clone(&state);
        let utterance = tokio::spawn(async move {
            translate_and_record(&task_state, Speaker::Patient, "Me duele la cabeza").await
        });
        for _ in 0..500 {
            if state.translation.lock().await.in_flight == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        provider.notify_complete();
        utterance.await.expect("task join").expect("translate");
        let models = provider.requested_models();
        assert_eq!(models, vec!["qwen3:8b".to_string()]);

        // The translation request itself opts out of thinking and is
        // token-capped (the ai_translator contract, observed end-to-end
        // through the command layer).
        let requests = provider.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].reasoning_effort.as_deref(), Some("none"));
        assert!(requests[0].max_tokens.is_some_and(|t| t <= 1024));
    }

    /// The per-feature `translation_model` override wins when set (non-empty)
    /// and the global `ai_model` is used otherwise — the OCR fallback pattern.
    #[test]
    fn translation_model_resolution_prefers_override_and_falls_back() {
        let mut config = AppConfig {
            ai_model: "qwen3:8b".into(),
            ..Default::default()
        };
        // Unset → global model.
        assert_eq!(translation_model_from_config(&config), "qwen3:8b");
        // Empty string is treated as unset (the sentinel the frontend saves).
        config.translation_model = Some(String::new());
        assert_eq!(translation_model_from_config(&config), "qwen3:8b");
        // Set → override wins.
        config.translation_model = Some("qwen3:1.7b".into());
        assert_eq!(translation_model_from_config(&config), "qwen3:1.7b");
    }

    #[tokio::test]
    async fn text_utterance_sends_the_translation_model_override() {
        let config = AppConfig {
            ai_provider: "gated".into(),
            ai_model: "qwen3:8b".into(),
            translation_model: Some("qwen3:1.7b".into()),
            ..Default::default()
        };
        let provider = GatedProvider::new();
        let (state, _) = build_test_state_with_provider(config, "", provider.clone()).await;
        let state = Arc::new(state);
        translation_start_session_inner(&state, "es", "en")
            .await
            .expect("start session");

        let task_state = Arc::clone(&state);
        let utterance = tokio::spawn(async move {
            translate_and_record(&task_state, Speaker::Provider, "Hello").await
        });
        for _ in 0..500 {
            if state.translation.lock().await.in_flight == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        provider.notify_complete();
        utterance.await.expect("task join").expect("translate");
        assert_eq!(provider.requested_models(), vec!["qwen3:1.7b".to_string()]);
    }

    /// The capture-time keep-alive ping must be content-free (fixed literal),
    /// one token, and thinking-free — it pages the model in, nothing else.
    /// PHI: this is the pinned contract that no utterance content ever
    /// reaches the ping.
    #[tokio::test]
    async fn keepalive_ping_is_a_content_free_one_token_no_thinking_request() {
        let provider = GatedProvider::new();
        let ping = tokio::spawn(llm_keepalive_ping(
            Arc::clone(&provider) as Arc<dyn AiProvider>,
            "qwen3:1.7b".to_string(),
        ));
        for _ in 0..500 {
            if !provider.requests().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        provider.notify_complete();
        ping.await.expect("task join").expect("ping succeeds");

        let requests = provider.requests();
        assert_eq!(requests.len(), 1);
        let req = &requests[0];
        assert_eq!(req.model, "qwen3:1.7b");
        assert_eq!(req.max_tokens, Some(1));
        assert_eq!(req.reasoning_effort.as_deref(), Some("none"));
        assert!(req.system_prompt.is_none());
        match &req.messages[..] {
            [msg] => match &msg.content {
                medical_core::types::MessageContent::Text(text) => {
                    assert_eq!(text, KEEPALIVE_PROMPT);
                }
                _ => panic!("expected text content"),
            },
            _ => panic!("expected exactly one message"),
        }
    }
}
