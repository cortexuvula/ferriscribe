//! # medical-stt-providers
//!
//! Speech-to-text via Whisper (local whisper.cpp or remote OpenAI-compatible server)
//! and speaker diarization via pyannote + WeSpeaker ONNX models.
//!
//! ## Provider Implementations
//!
//! - [`LocalSttProvider`] — runs whisper-rs locally (Metal GPU on macOS) with optional
//!   local pyannote diarization. The only implementation re-exported at crate root.
//! - [`remote_provider::RemoteSttProvider`] — sends audio to a remote Whisper server
//!   via HTTP POST, with the same local diarization pipeline.
//!
//! Both implement the [`medical_core::traits::SttProvider`] trait. Neither supports
//! streaming transcription.
//!
//! ## Module Overview
//!
//! | Module | Purpose |
//! |---|---|
//! | [`local_provider`] | Local whisper-rs inference + diarization |
//! | [`remote_provider`] | HTTP client for remote Whisper server |
//! | [`endpoint`] | LAN/Tailscale URL resolution with 30s cache |
//! | [`client`] | Multipart HTTP POST + cancellation + error mapping |
//! | [`whisper`] | whisper-rs wrapper: greedy decoding, centisecond timestamps |
//! | [`diarization`] | pyannote VAD + WeSpeaker embeddings + cosine clustering |
//! | [`audio_prep`] | Resampling (rubato), f32↔i16, WAV encoding |
//! | [`merge`] | Merge whisper segments with speaker turns by overlap |
//! | [`models`] | Model catalog, download/delete, path helpers |

pub mod audio_prep;
pub mod client;
pub mod diarization;
pub mod endpoint;
pub mod local_provider;
pub mod merge;
pub mod models;
pub mod remote_provider;
pub mod whisper;

pub use local_provider::LocalSttProvider;

use thiserror::Error;

/// Errors specific to the STT provider layer.
///
/// Most STT errors in practice flow through [`medical_core::error::AppError`]
/// (e.g. `AppError::SttProvider`, `AppError::EndpointOffline`). This enum
/// covers legacy paths in model download/delete that predate the unified
/// error model.
#[derive(Error, Debug)]
pub enum SttError {
    /// Transcription failed (generic — prefer `AppError::SttProvider` for new code).
    #[error("Transcription failed: {0}")]
    Transcription(String),
    /// The STT provider binary or server is not available.
    #[error("Provider unavailable: {0}")]
    Unavailable(String),
    /// The audio data is malformed or in an unsupported format.
    #[error("Audio format error: {0}")]
    AudioFormat(String),
    /// Model download failed or the destination is not writable.
    #[error("Model download error: {0}")]
    ModelDownload(String),
    /// The requested model file does not exist on disk.
    #[error("Model not found: {0}")]
    ModelNotFound(String),
}

/// Convenience result type for [`SttError`].
pub type SttResult<T> = Result<T, SttError>;

#[cfg(all(test, target_os = "macos"))]
mod link_regression_tests {
    //! Regression tripwire for the v0.82.0 macOS release-link failure
    //! (run 41077937853): whisper.cpp's ggml-metal-device.m and ORT's
    //! CoreML objects reference `___isPlatformVersionAtLeast`, which only
    //! the toolchain's compiler-rt archive (now wired in by this crate's
    //! build.rs) or rustc's std shim can define. This test binary links
    //! both native objects, so a toolchain that drops the std shim AND a
    //! regressed build.rs fail here (or at link time) instead of only in
    //! the release job. See crates/stt-providers/build.rs for the full
    //! mechanism.

    use std::process::Command;

    #[test]
    fn apple_availability_symbol_is_resolved_in_this_binary() {
        let current = std::env::current_exe().expect("current exe path");
        let output = Command::new("nm")
            .arg(&current)
            .output()
            .expect("run nm (Xcode tools are a build prerequisite)");
        assert!(
            output.status.success(),
            "nm failed on {}: {}",
            current.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let syms = String::from_utf8_lossy(&output.stdout);
        let bad: Vec<&str> = syms
            .lines()
            .filter(|l| l.contains("isPlatformVersionAtLeast") && l.contains(" U "))
            .collect();
        // An *undefined* symbol in the symbol table of an already-linked
        // executable means the availability shim is being imported from
        // nowhere (flat-namespace hunt at launch). Defined (T/t/W) or
        // absent (dead-stripped, nothing referenced it) are both healthy.
        assert!(
            bad.is_empty(),
            "___isPlatformVersionAtLeast is UNDEFINED in {} — the \
             compiler-rt link fix (crates/stt-providers/build.rs) has \
             regressed: {bad:?}",
            current.display()
        );
    }
}
