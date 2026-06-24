//! # medical-audio
//!
//! Microphone capture, audio device management, format conversion, playback,
//! and waveform analysis for the FerriScribe medical transcription app.
//!
//! ## Modules
//!
//! - [`capture`] — Real-time microphone capture via cpal with WAV file output.
//! - [`device`] — Enumerate and select system audio input/output devices.
//! - [`convert`] — Decode any supported audio format (MP3, FLAC, OGG, AAC) to WAV.
//! - [`playback`] — Play audio files through the system output device via rodio.
//! - [`state`] — Recording session state machine (Idle → Recording → Paused → Stopped).
//! - [`waveform`] — Signal analysis utilities: RMS, peak, dB conversion, normalization.
//!
//! ## Quick Start
//!
//! ```no_run
//! use medical_audio::device::get_input_device;
//! use medical_audio::capture::{start_capture, CaptureConfig};
//!
//! let device = get_input_device(None).unwrap(); // system default mic
//! let (handle, waveform_rx) = start_capture(
//!     &device,
//!     CaptureConfig::default(),
//!     std::path::Path::new("/tmp/recording.wav"),
//! ).unwrap();
//!
//! // Receive waveform snapshots for UI visualization
//! for snapshot in waveform_rx.iter() {
//!     // snapshot: Vec<f32> of ~128 peak values
//! }
//!
//! handle.stop(); // finalize WAV and join drain thread
//! ```

pub mod capture;
pub mod convert;
pub mod device;
pub mod playback;
pub mod state;
pub mod waveform;

use thiserror::Error;

/// Errors that can occur in the audio subsystem.
#[derive(Error, Debug)]
pub enum AudioError {
    /// An error from the audio device layer (enumeration, configuration).
    #[error("Device error: {0}")]
    Device(String),
    /// An error during microphone capture (stream creation, sample delivery).
    #[error("Capture error: {0}")]
    Capture(String),
    /// An error during audio playback (decoding, output stream).
    #[error("Playback error: {0}")]
    Playback(String),
    /// An error during audio format encoding/decoding (conversion, WAV writing).
    #[error("Encoding error: {0}")]
    Encoding(String),
    /// A filesystem I/O error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// No audio input device is available or the requested device was not found.
    #[error("No input device available")]
    NoInputDevice,
    /// No audio output device is available or the requested device was not found.
    #[error("No output device available")]
    NoOutputDevice,
    /// The state machine rejected a transition (e.g., pausing from Idle).
    #[error("Invalid state transition: {from} → {to}")]
    InvalidTransition {
        /// The state we were in.
        from: String,
        /// The state we attempted to transition to.
        to: String,
    },
}

/// Convenience result type for audio operations.
pub type AudioResult<T> = Result<T, AudioError>;
