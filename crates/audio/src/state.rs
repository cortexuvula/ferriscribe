//! Recording session state machine.
//!
//! Tracks the lifecycle of a recording session through `Idle → Recording →
//! Paused → Stopped` transitions, accumulating elapsed time across pause/resume
//! cycles. Invalid transitions return [`AudioError::InvalidTransition`].

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{AudioError, AudioResult};

// ──────────────────────────────────────────────────────────────────────────────
// RecordingState
// ──────────────────────────────────────────────────────────────────────────────

/// The current state of a recording session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state")]
pub enum RecordingState {
    /// Nothing is happening.
    Idle,

    /// A recording is actively in progress.
    Recording {
        /// When recording started (this segment). Not serialized.
        #[serde(skip)]
        started_at: Option<Instant>,
        /// Path to the output WAV file.
        file_path: PathBuf,
        /// Name of the capture device.
        device_name: String,
        /// Total elapsed time accumulated before the current segment started.
        elapsed_before_pause: Duration,
    },

    /// Recording has been paused.
    Paused {
        /// When the pause began. Not serialized.
        #[serde(skip)]
        paused_at: Option<Instant>,
        /// Path to the output WAV file.
        file_path: PathBuf,
        /// Name of the capture device.
        device_name: String,
        /// Elapsed time accumulated up to (and including) the moment we paused.
        elapsed_before_pause: Duration,
    },

    /// Recording has been fully stopped.
    Stopped {
        /// Path to the finished WAV file.
        file_path: PathBuf,
        /// Total duration of the recording.
        duration: Duration,
    },
}

impl RecordingState {
    pub fn is_idle(&self) -> bool {
        matches!(self, RecordingState::Idle)
    }

    pub fn is_recording(&self) -> bool {
        matches!(self, RecordingState::Recording { .. })
    }

    pub fn is_paused(&self) -> bool {
        matches!(self, RecordingState::Paused { .. })
    }

    pub fn is_stopped(&self) -> bool {
        matches!(self, RecordingState::Stopped { .. })
    }

    /// Total elapsed recording time (excluding paused intervals).
    pub fn elapsed(&self) -> Duration {
        match self {
            RecordingState::Idle => Duration::ZERO,
            RecordingState::Recording {
                started_at,
                elapsed_before_pause,
                ..
            } => {
                let segment = started_at
                    .map(|t| t.elapsed())
                    .unwrap_or(Duration::ZERO);
                *elapsed_before_pause + segment
            }
            RecordingState::Paused {
                elapsed_before_pause,
                ..
            } => *elapsed_before_pause,
            RecordingState::Stopped { duration, .. } => *duration,
        }
    }

    /// The output file path, if one is set.
    pub fn file_path(&self) -> Option<&PathBuf> {
        match self {
            RecordingState::Recording { file_path, .. } => Some(file_path),
            RecordingState::Paused { file_path, .. } => Some(file_path),
            RecordingState::Stopped { file_path, .. } => Some(file_path),
            RecordingState::Idle => None,
        }
    }

    /// A human-readable label for the current state.
    pub fn label(&self) -> &str {
        match self {
            RecordingState::Idle => "Idle",
            RecordingState::Recording { .. } => "Recording",
            RecordingState::Paused { .. } => "Paused",
            RecordingState::Stopped { .. } => "Stopped",
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// StateMachine
// ──────────────────────────────────────────────────────────────────────────────

/// Manages transitions between `RecordingState` variants.
#[derive(Debug)]
pub struct StateMachine {
    state: RecordingState,
}

impl Default for StateMachine {
    fn default() -> Self {
        Self {
            state: RecordingState::Idle,
        }
    }
}

impl StateMachine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current state (shared reference).
    pub fn state(&self) -> &RecordingState {
        &self.state
    }

    // ── Transitions ───────────────────────────────────────────────────────────

    /// `Idle → Recording`
    pub fn start(&mut self, file_path: PathBuf, device_name: String) -> AudioResult<()> {
        match &self.state {
            RecordingState::Idle => {
                self.state = RecordingState::Recording {
                    started_at: Some(Instant::now()),
                    file_path,
                    device_name,
                    elapsed_before_pause: Duration::ZERO,
                };
                Ok(())
            }
            other => Err(AudioError::InvalidTransition {
                from: other.label().to_string(),
                to: "Recording".to_string(),
            }),
        }
    }

    /// `Recording → Paused`
    pub fn pause(&mut self) -> AudioResult<()> {
        match std::mem::replace(&mut self.state, RecordingState::Idle) {
            RecordingState::Recording {
                started_at,
                file_path,
                device_name,
                elapsed_before_pause,
            } => {
                let segment = started_at
                    .map(|t| t.elapsed())
                    .unwrap_or(Duration::ZERO);
                self.state = RecordingState::Paused {
                    paused_at: Some(Instant::now()),
                    file_path,
                    device_name,
                    elapsed_before_pause: elapsed_before_pause + segment,
                };
                Ok(())
            }
            other => {
                let label = other.label().to_string();
                self.state = other;
                Err(AudioError::InvalidTransition {
                    from: label,
                    to: "Paused".to_string(),
                })
            }
        }
    }

    /// `Paused → Recording`
    pub fn resume(&mut self) -> AudioResult<()> {
        match std::mem::replace(&mut self.state, RecordingState::Idle) {
            RecordingState::Paused {
                file_path,
                device_name,
                elapsed_before_pause,
                ..
            } => {
                self.state = RecordingState::Recording {
                    started_at: Some(Instant::now()),
                    file_path,
                    device_name,
                    elapsed_before_pause,
                };
                Ok(())
            }
            other => {
                let label = other.label().to_string();
                self.state = other;
                Err(AudioError::InvalidTransition {
                    from: label,
                    to: "Recording".to_string(),
                })
            }
        }
    }

    /// `Recording | Paused → Stopped`
    pub fn stop(&mut self) -> AudioResult<()> {
        let elapsed = self.state.elapsed();
        match std::mem::replace(&mut self.state, RecordingState::Idle) {
            RecordingState::Recording { file_path, .. }
            | RecordingState::Paused { file_path, .. } => {
                self.state = RecordingState::Stopped {
                    file_path,
                    duration: elapsed,
                };
                Ok(())
            }
            other => {
                let label = other.label().to_string();
                self.state = other;
                Err(AudioError::InvalidTransition {
                    from: label,
                    to: "Stopped".to_string(),
                })
            }
        }
    }

    /// `Stopped | Idle → Idle`
    pub fn reset(&mut self) -> AudioResult<()> {
        match &self.state {
            RecordingState::Stopped { .. } | RecordingState::Idle => {
                self.state = RecordingState::Idle;
                Ok(())
            }
            other => Err(AudioError::InvalidTransition {
                from: other.label().to_string(),
                to: "Idle".to_string(),
            }),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn path() -> PathBuf {
        PathBuf::from("/tmp/test.wav")
    }

    #[test]
    fn starts_idle() {
        let sm = StateMachine::new();
        assert!(sm.state().is_idle());
    }

    #[test]
    fn full_lifecycle() {
        let mut sm = StateMachine::new();
        sm.start(path(), "Mic".into()).unwrap();
        assert!(sm.state().is_recording());
        sm.pause().unwrap();
        assert!(sm.state().is_paused());
        sm.resume().unwrap();
        assert!(sm.state().is_recording());
        sm.stop().unwrap();
        assert!(sm.state().is_stopped());
        sm.reset().unwrap();
        assert!(sm.state().is_idle());
    }

    #[test]
    fn cannot_pause_from_idle() {
        let mut sm = StateMachine::new();
        assert!(matches!(
            sm.pause(),
            Err(AudioError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn cannot_resume_from_idle() {
        let mut sm = StateMachine::new();
        assert!(matches!(
            sm.resume(),
            Err(AudioError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn cannot_start_while_recording() {
        let mut sm = StateMachine::new();
        sm.start(path(), "Mic".into()).unwrap();
        assert!(matches!(
            sm.start(path(), "Mic".into()),
            Err(AudioError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn cannot_stop_from_idle() {
        let mut sm = StateMachine::new();
        assert!(matches!(
            sm.stop(),
            Err(AudioError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn can_stop_from_paused() {
        let mut sm = StateMachine::new();
        sm.start(path(), "Mic".into()).unwrap();
        sm.pause().unwrap();
        sm.stop().unwrap();
        assert!(sm.state().is_stopped());
    }

    #[test]
    fn file_path_in_active_states() {
        let mut sm = StateMachine::new();
        assert!(sm.state().file_path().is_none());
        sm.start(path(), "Mic".into()).unwrap();
        assert_eq!(sm.state().file_path(), Some(&path()));
        sm.pause().unwrap();
        assert_eq!(sm.state().file_path(), Some(&path()));
        sm.stop().unwrap();
        assert_eq!(sm.state().file_path(), Some(&path()));
    }

    #[test]
    fn elapsed_accumulates_across_pause_resume() {
        let mut sm = StateMachine::new();
        sm.start(path(), "Mic".into()).unwrap();
        // Recording for a tiny bit.
        std::thread::sleep(Duration::from_millis(20));
        sm.pause().unwrap();
        let after_first = sm.state().elapsed();
        // Elapsed should be > 0
        assert!(after_first > Duration::ZERO);
        // While paused, elapsed doesn't grow.
        let still_paused = sm.state().elapsed();
        assert_eq!(after_first, still_paused);
        sm.resume().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        sm.stop().unwrap();
        let total = sm.state().elapsed();
        // Total should be at least the first segment.
        assert!(total >= after_first);
    }

    #[test]
    fn state_labels() {
        let mut sm = StateMachine::new();
        assert_eq!(sm.state().label(), "Idle");
        sm.start(path(), "Mic".into()).unwrap();
        assert_eq!(sm.state().label(), "Recording");
        sm.pause().unwrap();
        assert_eq!(sm.state().label(), "Paused");
        sm.stop().unwrap();
        assert_eq!(sm.state().label(), "Stopped");
    }
}
