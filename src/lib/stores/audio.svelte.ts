import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import * as audioApi from '../api/audio';
import type { AudioHealthEvent, StopRecordingResult } from '../api/audio';
import { log } from '../api/logging';
import { formatError } from '../types/errors';

export type RecordingState = 'idle' | 'recording' | 'paused' | 'stopped';

export interface AudioStoreState {
  state: RecordingState;
  elapsed: number;
  waveformData: number[];
  /** Latest watchdog snapshot (null when not recording). */
  health: AudioHealthEvent | null;
  deviceName: string | null;
  lastRecordingId: string | null;
  /** Watchdog verdict returned by the last stop (id + health summary). */
  lastRecordingHealth: StopRecordingResult | null;
  error: string | null;
}

const initialState: AudioStoreState = {
  state: 'idle',
  elapsed: 0,
  waveformData: [],
  health: null,
  deviceName: null,
  lastRecordingId: null,
  lastRecordingHealth: null,
  error: null,
};

class AudioStore {
  state = $state<AudioStoreState>({
    state: 'idle',
    elapsed: 0,
    waveformData: [],
    health: null,
    deviceName: null,
    lastRecordingId: null,
    lastRecordingHealth: null,
    error: null,
  });

  // Private non-reactive internal state
  private timer: ReturnType<typeof setInterval> | null = null;
  private waveformUnlisten: UnlistenFn | null = null;
  private healthUnlisten: UnlistenFn | null = null;
  private busy = false;

  private clearTimer() {
    if (this.timer !== null) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  /** Unsubscribe both capture-event listeners (waveform + health). */
  private teardownListeners() {
    if (this.waveformUnlisten) {
      this.waveformUnlisten();
      this.waveformUnlisten = null;
    }
    if (this.healthUnlisten) {
      this.healthUnlisten();
      this.healthUnlisten = null;
    }
  }

  /** Subscribe to the backend capture events (waveform visualizer + the
   *  ~1 Hz audio-health watchdog). Must be called BEFORE startRecording so
   *  no early frames are missed. Cleans up stale listeners first. */
  private async subscribeCaptureEvents() {
    this.teardownListeners();
    this.waveformUnlisten = await listen<number[]>('waveform-data', (event) => {
      this.state = {
        ...this.state,
        waveformData: [...this.state.waveformData, ...event.payload].slice(-256),
      };
    });
    this.healthUnlisten = await listen<AudioHealthEvent>('audio-health', (event) => {
      this.state = { ...this.state, health: event.payload };
    });
  }

  /**
   * Tear down all background resources (elapsed-seconds timer + event
   * listeners). Called from App.svelte onDestroy so a webview reload or app
   * exit doesn't orphan the interval or the Tauri event listeners. Safe to
   * call when idle — everything is null-checked.
   */
  destroy() {
    this.clearTimer();
    this.teardownListeners();
  }

  private startTimer() {
    this.clearTimer();
    this.timer = setInterval(() => {
      this.state = { ...this.state, elapsed: this.state.elapsed + 1 };
    }, 1000);
  }

  async startRecording(device: string | null = null) {
    if (this.busy) return;
    this.busy = true;
    try {
      // Subscribe to waveform + health events BEFORE starting recording.
      await this.subscribeCaptureEvents();

      const recordingId = await audioApi.startRecording();
      log.info('Recording started', { recordingId, device: device ?? 'default' });
      this.state = {
        ...this.state,
        state: 'recording',
        elapsed: 0,
        waveformData: [],
        health: null,
        deviceName: device,
        lastRecordingId: recordingId,
        error: null,
      };
      this.startTimer();
    } catch (e) {
      const message = formatError(e);
      log.error('Failed to start recording', { error: message, device: device ?? 'default' });
      this.teardownListeners();
      this.state = {
        ...this.state,
        error: message || 'Failed to start recording',
      };
    } finally {
      this.busy = false;
    }
  }

  async pause() {
    try {
      await audioApi.pauseRecording();
      this.clearTimer();
      this.state = { ...this.state, state: 'paused' };
    } catch (e) {
      this.state = {
        ...this.state,
        error: formatError(e) || 'Failed to pause',
      };
    }
  }

  async resume() {
    try {
      await audioApi.resumeRecording();
      this.state = { ...this.state, state: 'recording' };
      this.startTimer();
    } catch (e) {
      this.state = {
        ...this.state,
        error: formatError(e) || 'Failed to resume',
      };
    }
  }

  /** Stop the recording. Resolves to the backend-confirmed recording id,
   *  or null when the stop failed / was already busy — callers must NOT
   *  launch the pipeline on null (the row may not exist). The backend's
   *  watchdog verdict is kept on `state.lastRecordingHealth` for the
   *  silence-dialog flow. */
  async stop(): Promise<string | null> {
    if (this.busy) return null;
    this.busy = true;
    this.clearTimer();
    // Stop listening for capture events immediately so the visualizer
    // freezes and health alerts stop.
    this.teardownListeners();
    // Optimistically flip the UI to 'stopped' BEFORE awaiting the backend.
    // This makes the Stop button feel instant — the button set swaps from
    // "Pause/Stop/Cancel" to "New Recording" immediately. The backend stop
    // (drain-thread join + encryption + DB insert) runs concurrently and
    // reconciles lastRecordingId when done.
    this.state = {
      ...this.state,
      state: 'stopped',
      waveformData: [], // clear the visualizer so it returns to a flat line
      health: null,
      lastRecordingId: this.state.lastRecordingId, // keep existing until backend confirms
    };
    try {
      const result = await audioApi.stopRecording();
      log.info('Recording stopped', {
        recordingId: result.recording_id,
        isSilent: result.is_silent,
        signalSecs: result.signal_secs,
      });
      // Reconcile with the actual recording ID from the backend.
      this.state = {
        ...this.state,
        lastRecordingId: result.recording_id,
        lastRecordingHealth: result,
      };
      return result.recording_id;
    } catch (e) {
      const message = formatError(e);
      log.error('Failed to stop recording', { error: message });
      // The UI already shows 'stopped'. If the backend failed, surface the
      // error but don't revert to 'recording' — the stream is likely torn
      // down on the OS side even if the command errored.
      this.state = {
        ...this.state,
        error: message || 'Failed to stop recording',
      };
      // Signal failure: the old lastRecordingId may reference a row that
      // was never inserted (the backend inserts at stop time), so the
      // caller must not launch the pipeline against it.
      return null;
    } finally {
      this.busy = false;
    }
  }

  async cancel() {
    if (this.busy) return;
    this.busy = true;
    this.clearTimer();
    try {
      await audioApi.cancelRecording();
    } catch (_e) {
      // Best-effort — even if backend fails, reset the frontend state
    }
    this.teardownListeners();
    this.state = { ...initialState };
    this.busy = false;
  }

  reset() {
    this.clearTimer();
    this.teardownListeners();
    this.state = { ...initialState };
  }

  pushWaveform(data: number[]) {
    this.state = {
      ...this.state,
      waveformData: [...this.state.waveformData, ...data].slice(-256),
    };
  }

  /** Recover state from the backend on startup — if a recording is still
   *  running (e.g. after a webview reload), rehydrate the store so the Stop
   *  button is visible and the timer keeps ticking. */
  async rehydrate() {
    try {
      const snap = await audioApi.getRecordingState();
      if (!snap.active || !snap.recording_id) return;

      // Re-subscribe to capture events (waveform + health) — the backend
      // emitter task keeps running across webview reloads.
      await this.subscribeCaptureEvents();

      const initialElapsed = Math.floor(snap.elapsed_secs ?? 0);
      this.state = {
        ...this.state,
        state: 'recording',
        elapsed: initialElapsed,
        waveformData: [],
        health: null,
        lastRecordingId: snap.recording_id,
        error: null,
      };
      this.startTimer();
      log.info('Recovered orphan recording after reload', {
        recordingId: snap.recording_id,
        elapsedSecs: initialElapsed,
      });
    } catch (e) {
      log.warn('Could not query recording state on startup', { error: formatError(e) });
    }
  }
}

export const audio = new AudioStore();
