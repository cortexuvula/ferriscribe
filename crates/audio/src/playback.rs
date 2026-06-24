//! Audio playback via rodio.
//!
//! Provides a [`Player`] struct that wraps a rodio `Sink` and `OutputStream`,
//! exposing play/pause/stop/volume controls for decoded audio files.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

use crate::{AudioError, AudioResult};

// ──────────────────────────────────────────────────────────────────────────────
// Player
// ──────────────────────────────────────────────────────────────────────────────

/// A simple audio player backed by rodio.
///
/// The `OutputStream` must be kept alive as long as the `Player` is in use,
/// which is why it is stored inside the struct.
pub struct Player {
    // The stream must not be dropped while the sink is alive.
    _stream: OutputStream,
    _stream_handle: OutputStreamHandle,
    sink: Sink,
}

impl Player {
    /// Create a new `Player` using the system default output device.
    ///
    /// The sink starts in a paused state.
    pub fn new() -> AudioResult<Self> {
        let (_stream, stream_handle) =
            OutputStream::try_default().map_err(|e| AudioError::Playback(e.to_string()))?;

        let sink =
            Sink::try_new(&stream_handle).map_err(|e| AudioError::Playback(e.to_string()))?;

        // Start paused — callers call play_file() which un-pauses as needed.
        sink.pause();

        Ok(Self {
            _stream,
            _stream_handle: stream_handle,
            sink,
        })
    }

    // ── Playback control ──────────────────────────────────────────────────────

    /// Open `path`, decode it, append it to the sink, and start playing.
    pub fn play_file(&self, path: &Path) -> AudioResult<()> {
        let file = File::open(path).map_err(AudioError::Io)?;
        let source =
            Decoder::new(BufReader::new(file)).map_err(|e| AudioError::Playback(e.to_string()))?;
        self.sink.append(source);
        self.sink.play();
        Ok(())
    }

    /// Pause playback (can be resumed later).
    pub fn pause(&self) {
        self.sink.pause();
    }

    /// Resume a paused playback.
    pub fn resume(&self) {
        self.sink.play();
    }

    /// Stop playback immediately and clear the queue.
    pub fn stop(&self) {
        self.sink.stop();
    }

    /// Set the playback volume.  The value is clamped to `[0.0, 2.0]`.
    pub fn set_volume(&self, volume: f32) {
        let clamped = volume.clamp(0.0, 2.0);
        self.sink.set_volume(clamped);
    }

    /// Current volume level.
    pub fn volume(&self) -> f32 {
        self.sink.volume()
    }

    /// `true` if the sink is paused.
    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    /// `true` if the queue is empty (nothing left to play).
    pub fn is_empty(&self) -> bool {
        self.sink.empty()
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_creation_fails_gracefully() {
        // In CI with no audio hardware this may fail with a Playback error —
        // that is the expected graceful degradation.
        match Player::new() {
            Ok(_) => {}
            Err(AudioError::Playback(_)) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn play_nonexistent_errors() {
        // If we can't create a player at all, skip.
        let player = match Player::new() {
            Ok(p) => p,
            Err(_) => return,
        };
        let result = player.play_file(Path::new("/nonexistent/file.wav"));
        assert!(
            result.is_err(),
            "expected error when playing a nonexistent file"
        );
    }
}
