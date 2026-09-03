/**
 * Frontend policy for the capture-health watchdog: turns a backend
 * `AudioHealthEvent` snapshot (numbers only, ~1 Hz) into the alert the
 * Record banner should show — or null when capture looks healthy.
 *
 * The thresholds are deliberately generous to avoid false alarms during
 * real consults:
 * - "no data" uses a DEAD-stream signal (device stopped delivering
 *   samples entirely), not merely quiet audio — rooms have noise floor,
 *   dead devices emit nothing;
 * - "no signal yet" only fires after a grace window, so a clinician who
 *   hits Record before the patient starts talking isn't nagged;
 * - everything is suppressed while paused (no samples flow by design).
 */
import type { AudioHealthEvent } from '../api/audio';

/** Grace period before warning that no speech has been detected. */
export const FIRST_SIGNAL_GRACE_SECS = 15;
/** Quiet period after signal was established before warning it went away. */
export const SIGNAL_LOST_SECS = 10;
/** Time without ANY samples from the device before treating it as dead.
 *  Generous: Bluetooth headsets renegotiating to HFP can take several
 *  seconds to deliver their first buffer — a shorter window flashes a
 *  false "not capturing" danger banner on a healthy setup. */
export const NO_DATA_SECS = 8;

export interface AudioHealthAlert {
  level: 'warning' | 'danger';
  message: string;
}

export function audioHealthAlert(h: AudioHealthEvent | null): AudioHealthAlert | null {
  if (!h) return null;

  // OS-level stream failure trumps everything — the capture may be
  // truncated regardless of what arrived before.
  if (h.stream_error) {
    return {
      level: 'danger',
      message: `Microphone stream error — ${h.stream_error}. The recording may be incomplete; check your audio device.`,
    };
  }

  // Paused capture delivers no samples by design — don't alert.
  if (h.paused) return null;

  // Device delivering nothing at all (never, or stopped): dead stream,
  // wrong device, or revoked permission. Digital-silence streams do NOT
  // land here — they keep delivering (zero) samples.
  const secs_since_data = h.secs_since_last_data ?? h.elapsed_secs;
  const data_flowing = h.secs_since_last_data !== null && h.secs_since_last_data < NO_DATA_SECS;
  if (secs_since_data >= NO_DATA_SECS) {
    return {
      level: 'danger',
      message:
        'No audio is arriving from the microphone — the recording is not capturing. Check that the mic is connected, unmuted, and selected.',
    };
  }

  // Data flowing but nothing ever qualified as speech, past the grace
  // window: mic live but silent (muted at the OS/hardware level, or the
  // wrong input picked up only ambient zeros).
  if (!h.has_signal && h.elapsed_secs >= FIRST_SIGNAL_GRACE_SECS) {
    return {
      level: 'warning',
      message:
        'No speech detected yet — the microphone is delivering silence. Check that it is unmuted and positioned correctly.',
    };
  }

  // Signal was established, then went quiet while data still flows.
  // (If data ALSO stopped, the dead-stream alert above already fired.)
  if (h.has_signal && h.secs_since_last_sound !== null && h.secs_since_last_sound >= SIGNAL_LOST_SECS && data_flowing) {
    const secs = Math.round(h.secs_since_last_sound);
    return {
      level: 'warning',
      message: `Microphone went silent ${secs}s ago — check that it is still working.`,
    };
  }

  return null;
}
