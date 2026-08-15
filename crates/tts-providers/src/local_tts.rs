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
//! The engine is owned by a dedicated OS thread and driven through a channel,
//! so no `unsafe impl Send`/`Sync` is needed and OS speech calls never run on
//! (or block) a tokio worker thread.
//!
//! When the feature is **disabled** (the default), a zero-sized stub is
//! provided so downstream code can still reference the type.

// ── Feature-gated implementation ────────────────────────────────────────────

#[cfg(feature = "local-tts")]
mod inner {
    use std::sync::mpsc;

    use async_trait::async_trait;
    use tokio::sync::oneshot;
    use tracing::{info, warn};

    use medical_core::error::{AppError, AppResult};
    use medical_core::traits::TtsProvider;
    use medical_core::types::tts::{TtsConfig, VoiceInfo};

    /// A request for the dedicated TTS thread.
    enum TtsCommand {
        Speak {
            text: String,
            rate: f32,
            volume: f32,
            voice: Option<String>,
            done: oneshot::Sender<AppResult<()>>,
        },
        ListVoices {
            done: oneshot::Sender<AppResult<Vec<VoiceInfo>>>,
        },
    }

    /// Cross-platform local text-to-speech provider.
    ///
    /// Uses the system's native TTS engine. Note that the `tts` crate
    /// speaks audio directly through the system audio output -- it does
    /// **not** return PCM bytes.  The [`synthesize`] method will speak the
    /// text and return an empty byte vector.
    ///
    /// The `tts` crate's `Tts` is `!Send` on some platforms because the
    /// underlying OS speech APIs (SAPI on Windows, speech-dispatcher on
    /// Linux, NSSpeechSynthesizer on macOS) have thread-affinity
    /// requirements.  Rather than asserting `Send` across that boundary, the
    /// engine is owned by one thread spawned in [`LocalTtsProvider::new`]
    /// and driven through a channel; the async trait methods only hold
    /// channel endpoints, so no `unsafe impl Send`/`Sync` is required and
    /// OS speech calls never run on a tokio worker thread.
    pub struct LocalTtsProvider {
        cmd_tx: Option<mpsc::Sender<TtsCommand>>,
    }

    fn unavailable() -> AppError {
        AppError::tts_provider("Local TTS engine is not available on this platform")
    }

    fn thread_died() -> AppError {
        AppError::tts_provider("Local TTS engine thread has exited")
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
                    let (tx, rx) = mpsc::channel::<TtsCommand>();
                    match std::thread::Builder::new()
                        .name("local-tts".to_string())
                        .spawn(move || tts_thread(engine, rx))
                    {
                        Ok(_) => {
                            info!("Local TTS engine initialised successfully");
                            Self { cmd_tx: Some(tx) }
                        }
                        Err(e) => {
                            warn!(
                                "Failed to spawn local TTS thread: {e}. Provider will be unavailable."
                            );
                            Self { cmd_tx: None }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to initialise local TTS engine: {e}. Provider will be unavailable."
                    );
                    Self { cmd_tx: None }
                }
            }
        }

        fn tx(&self) -> AppResult<&mpsc::Sender<TtsCommand>> {
            self.cmd_tx.as_ref().ok_or_else(unavailable)
        }
    }

    impl Default for LocalTtsProvider {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Own the `tts::Tts` engine for its whole life.  Runs until the
    /// provider (and every `Sender` clone) is dropped; a panic in an OS
    /// speech call ends the thread and later commands surface
    /// `thread_died()` errors.
    fn tts_thread(mut engine: tts::Tts, rx: mpsc::Receiver<TtsCommand>) {
        for cmd in rx {
            match cmd {
                TtsCommand::Speak {
                    text,
                    rate,
                    volume,
                    voice,
                    done,
                } => {
                    let _ = done.send(speak_on_thread(&mut engine, &text, rate, volume, voice));
                }
                TtsCommand::ListVoices { done } => {
                    let _ = done.send(list_voices_on_thread(&engine));
                }
            }
        }
    }

    fn speak_on_thread(
        engine: &mut tts::Tts,
        text: &str,
        rate: f32,
        volume: f32,
        voice_id: Option<String>,
    ) -> AppResult<()> {
        // Apply speech rate if supported.
        if let Err(e) = engine.set_rate(rate) {
            warn!("Could not set TTS rate to {rate}: {e}");
        }

        // Apply volume if supported.
        if let Err(e) = engine.set_volume(volume) {
            warn!("Could not set TTS volume to {volume}: {e}");
        }

        // Set voice if requested.
        if let Some(voice_id) = voice_id.as_deref()
            && let Ok(voices) = engine.voices()
            && let Some(voice) = voices.into_iter().find(|v| v.id() == voice_id)
            && let Err(e) = engine.set_voice(&voice)
        {
            warn!("Could not set voice to {voice_id}: {e}");
        }

        // The `tts` crate speaks directly to the audio output device.
        // It does not provide raw PCM bytes. (The returned `UtteranceId`
        // is discarded — we don't track per-utterance callbacks.)
        engine
            .speak(text, false)
            .map(|_| ())
            .map_err(|e| AppError::tts_provider(format!("TTS speak failed: {e}")))
    }

    fn list_voices_on_thread(engine: &tts::Tts) -> AppResult<Vec<VoiceInfo>> {
        let os_voices = engine
            .voices()
            .map_err(|e| AppError::tts_provider(format!("Failed to list voices: {e}")))?;

        Ok(os_voices
            .into_iter()
            .map(|v| VoiceInfo {
                id: v.id().to_string(),
                name: v.name().to_string(),
                language: Some(v.language().to_string()),
                gender: v.gender().map(|g| format!("{g:?}")),
                preview_url: None,
            })
            .collect())
    }

    #[async_trait]
    impl TtsProvider for LocalTtsProvider {
        fn name(&self) -> &str {
            "local"
        }

        async fn available_voices(&self) -> AppResult<Vec<VoiceInfo>> {
            let tx = self.tx()?;
            let (done, done_rx) = oneshot::channel();
            tx.send(TtsCommand::ListVoices { done })
                .map_err(|_| thread_died())?;
            done_rx.await.map_err(|_| thread_died())?
        }

        async fn synthesize(&self, text: &str, config: TtsConfig) -> AppResult<Vec<u8>> {
            let tx = self.tx()?;
            let text_len = text.len();
            let (done, done_rx) = oneshot::channel();
            tx.send(TtsCommand::Speak {
                text: text.to_owned(),
                rate: config.speed,
                volume: config.volume,
                voice: config.voice,
                done,
            })
            .map_err(|_| thread_died())?;
            done_rx.await.map_err(|_| thread_died())??;

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
