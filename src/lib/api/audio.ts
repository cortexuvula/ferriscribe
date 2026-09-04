import { invoke } from '@tauri-apps/api/core';
import type { AudioDevice } from '../types';

export async function listAudioDevices(): Promise<AudioDevice[]> {
  return invoke('list_audio_devices');
}

export async function startRecording(): Promise<string> {
  return invoke('start_recording');
}

/** Watchdog snapshot emitted as the `audio-health` event (~1 Hz) while a
 *  recording is in progress. Numbers only — no audio content. */
export interface AudioHealthEvent {
  paused: boolean;
  /** Wall-clock seconds since capture start (includes pauses). */
  elapsed_secs: number;
  /** Sample-derived audio seconds (excludes pauses). */
  duration_secs: number;
  /** Span between first and last detected signal, null if never any. */
  signal_secs: number | null;
  peak: number;
  rms: number;
  total_samples: number;
  has_signal: boolean;
  /** Seconds since the device last delivered samples; null = never. */
  secs_since_last_data: number | null;
  /** Seconds since the last signal-qualifying chunk; null = never. */
  secs_since_last_sound: number | null;
  stream_error: string | null;
  /** First WAV write failure (disk full, unwritable folder) — the file on
   *  disk is empty/truncated even though the mic may be delivering. */
  write_error: string | null;
}

/** Structured stop result: recording id + the capture watchdog's verdict. */
export interface StopRecordingResult {
  recording_id: string;
  peak: number;
  rms: number;
  duration_secs: number;
  signal_secs: number | null;
  is_silent: boolean;
  stream_error: string | null;
  write_error: string | null;
}

export async function stopRecording(): Promise<StopRecordingResult> {
  return invoke('stop_recording');
}

export async function cancelRecording(): Promise<void> {
  return invoke('cancel_recording');
}

export async function pauseRecording(): Promise<void> {
  return invoke('pause_recording');
}

export async function resumeRecording(): Promise<void> {
  return invoke('resume_recording');
}

export interface RecordingAudioLevels {
  peak: number;
  rms: number;
  is_silent: boolean;
}

export async function checkRecordingAudioLevels(
  recordingId: string,
): Promise<RecordingAudioLevels> {
  return invoke('check_recording_audio_levels', { recordingId });
}

export interface RecordingStateSnapshot {
  active: boolean;
  recording_id: string | null;
  /** Elapsed recording seconds EXCLUDING paused intervals (matches what
   *  stop_recording persists as the duration). */
  elapsed_secs: number | null;
  /** True while the orphan capture is paused. */
  paused: boolean;
}

export async function getRecordingState(): Promise<RecordingStateSnapshot> {
  return invoke('get_recording_state');
}

/** Result of the Settings → Audio "Test microphone" probe. */
export interface MicrophoneProbeResult {
  peak: number;
  rms: number;
  is_silent: boolean;
  sample_rate: number;
  channels: number;
  samples: number;
}

/** Capture ~1.2 s from the selected (or default) input device and report
 *  its level, so a muted/dead mic can be caught before recording. */
export async function runMicrophoneProbe(
  device: string | null,
  durationMs?: number,
): Promise<MicrophoneProbeResult> {
  return invoke('run_microphone_probe', { device, durationMs });
}
