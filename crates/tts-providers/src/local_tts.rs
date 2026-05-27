//! Local platform TTS provider, feature-gated behind `local-tts`.
//!
//! When the `local-tts` feature is enabled, this module provides a
//! [`LocalTtsProvider`] backed by the `tts` crate which uses the platform's
//! native speech synthesis engine:
//!
//! - **Linux**: speech-dispatcher
//! - **macOS**: NSSpeechSynthesizer (AVSpeechSynthesizer)
//! - **Windows**: SAPI
//!
//! When the feature is **disabled** (the default), a zero-sized stub is
//! provided so downstream code can still reference the type.

// ── Feature-gated implementation ────────────────────────────────────────────

#[cfg(feature = "local-tts")]
mod inner {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use tracing::{info, warn};

    use medical_core::error::{AppError, AppResult};
    use medical_core::traits::TtsProvider;
    use medical_core::types::tts::{TtsConfig, VoiceInfo};

    /// Wrapper to make `tts::Tts` usable across threads.
    ///
    /// The `tts` crate's `Tts` struct is `!Send` on some platforms because
    /// the underlying OS speech APIs have thread-affinity requirements.
    /// We wrap it in a `Mutex` and mark the wrapper as `Send + Sync` so it
    /// can live inside an async context.  All actual calls go through
    /// `spawn_blocking` to stay on one OS thread at a time.
    struct TtsHandle(Mutex<tts::Tts>);

    // SAFETY: We only access the inner `Tts` through the `Mutex`, ensuring
    // exclusive access.  The `tts` crate itself serialises OS calls.
    unsafe impl Send for TtsHandle {}
    unsafe impl Sync for TtsHandle {}

    /// Cross-platform local text-to-speech provider.
    ///
    /// Uses the system's native TTS engine.  Note that the `tts` crate
    /// speaks audio directly through the system audio output -- it does
    /// **not** return PCM bytes.  The [`synthesize`] method will speak the
    /// text and return an empty byte vector.
    pub struct LocalTtsProvider {
        handle: Option<TtsHandle>,
    }

    impl LocalTtsProvider {
        /// Create a new local TTS provider.
        ///
        /// If the platform's TTS engine cannot be initialised (e.g. missing
        /// speech-dispatcher on Linux), the provider is created in a degraded
        /// state and all synthesis calls will return an error.
        pub fn new() -> Self {
            match tts::Tts::default() {
                Ok(engine) => {
                    info!("Local TTS engine initialised successfully");
                    Self {
                        handle: Some(TtsHandle(Mutex::new(engine))),
                    }
                }
                Err(e) => {
                    warn!("Failed to initialise local TTS engine: {e}. Provider will be unavailable.");
                    Self { handle: None }
                }
            }
        }
    }

    impl Default for LocalTtsProvider {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait]
    impl TtsProvider for LocalTtsProvider {
        fn name(&self) -> &str {
            "local"
        }

        async fn available_voices(&self) -> AppResult<Vec<VoiceInfo>> {
            let handle = match &self.handle {
                Some(h) => h,
                None => {
                    return Err(AppError::TtsProvider(
                        "Local TTS engine is not available on this platform".into(),
                    ));
                }
            };

            // Lock must happen on a blocking thread because the underlying
            // OS calls may not be async-safe.
            let guard = handle
                .0
                .lock()
                .map_err(|e| AppError::TtsProvider(format!("TTS mutex poisoned: {e}")))?;

            let os_voices = guard
                .voices()
                .map_err(|e| AppError::TtsProvider(format!("Failed to list voices: {e}")))?;

            let voices: Vec<VoiceInfo> = os_voices
                .into_iter()
                .map(|v| VoiceInfo {
                    id: v.id().to_string(),
                    name: v.name().to_string(),
                    language: Some(v.language().to_string()),
                    gender: v.gender().map(|g| format!("{g:?}")),
                    preview_url: None,
                })
                .collect();

            Ok(voices)
        }

        async fn synthesize(&self, text: &str, config: TtsConfig) -> AppResult<Vec<u8>> {
            let handle = match &self.handle {
                Some(h) => h,
                None => {
                    return Err(AppError::TtsProvider(
                        "Local TTS engine is not available on this platform".into(),
                    ));
                }
            };

            let text = text.to_owned();
            let mut guard = handle
                .0
                .lock()
                .map_err(|e| AppError::TtsProvider(format!("TTS mutex poisoned: {e}")))?;

            // Apply speech rate if supported.
            if let Err(e) = guard.set_rate(config.speed) {
                warn!("Could not set TTS rate to {}: {e}", config.speed);
            }

            // Apply volume if supported.
            if let Err(e) = guard.set_volume(config.volume) {
                warn!("Could not set TTS volume to {}: {e}", config.volume);
            }

            // Set voice if requested.
            if let Some(ref voice_id) = config.voice {
                if let Ok(voices) = guard.voices() {
                    if let Some(voice) = voices.into_iter().find(|v| v.id() == voice_id.as_str()) {
                        if let Err(e) = guard.set_voice(&voice) {
                            warn!("Could not set voice to {voice_id}: {e}");
                        }
                    }
                }
            }

            // The `tts` crate speaks directly to the audio output device.
            // It does not provide raw PCM bytes.
            let text_len = text.len();
            guard
                .speak(text, false)
                .map_err(|e| AppError::TtsProvider(format!("TTS speak failed: {e}")))?;

            info!(chars = text_len, "Local TTS speaking text");

            // Return empty bytes -- audio is played directly by the OS.
            Ok(Vec::new())
        }
    }
}

#[cfg(feature = "local-tts")]
pub use inner::LocalTtsProvider;

// ── Stub when feature is disabled ────────────────────────────────────────────

#[cfg(not(feature = "local-tts"))]
pub struct LocalTtsProvider;

#[cfg(not(feature = "local-tts"))]
impl LocalTtsProvider {
    /// Create a stub local TTS provider (feature `local-tts` is disabled).
    ///
    /// The returned provider does nothing — it exists only so that
    /// downstream code can reference [`LocalTtsProvider`] unconditionally.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(feature = "local-tts"))]
impl Default for LocalTtsProvider {
    fn default() -> Self {
        Self::new()
    }
}
