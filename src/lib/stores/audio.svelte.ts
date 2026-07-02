import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import * as audioApi from '../api/audio';
import { log } from '../api/logging';
import { formatError } from '../types/errors';

export type RecordingState = 'idle' | 'recording' | 'paused' | 'stopped';

export interface AudioStoreState {
  state: RecordingState;
  elapsed: number;
  waveformData: number[];
  deviceName: string | null;
  lastRecordingId: string | null;
  error: string | null;
}

const initialState: AudioStoreState = {
  state: 'idle',
  elapsed: 0,
  waveformData: [],
  deviceName: null,
  lastRecordingId: null,
  error: null,
};

class AudioStore {
  state = $state<AudioStoreState>({
    state: 'idle',
    elapsed: 0,
    waveformData: [],
    deviceName: null,
    lastRecordingId: null,
    error: null,
  });

  // Private non-reactive internal state
  private timer: ReturnType<typeof setInterval> | null = null;
  private waveformUnlisten: UnlistenFn | null = null;
  private busy = false;

  private clearTimer() {
    if (this.timer !== null) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  /**
   * Tear down all background resources (elapsed-seconds timer + waveform
   * listener). Called from App.svelte onDestroy so a webview reload or app
   * exit doesn't orphan the interval or the Tauri event listener. Safe to
   * call when idle — both fields are null-checked.
   */
  destroy() {
    this.clearTimer();
    if (this.waveformUnlisten) {
      this.waveformUnlisten();
      this.waveformUnlisten = null;
    }
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
      // Clean up any stale listener before attaching a new one
      if (this.waveformUnlisten) { this.waveformUnlisten(); this.waveformUnlisten = null; }
      // Listen for waveform events BEFORE starting recording
      this.waveformUnlisten = await listen<number[]>('waveform-data', (event) => {
        this.state = {
          ...this.state,
          waveformData: [...this.state.waveformData, ...event.payload].slice(-256),
        };
      });

      const recordingId = await audioApi.startRecording();
      log.info('Recording started', { recordingId, device: device ?? 'default' });
      this.state = {
        ...this.state,
        state: 'recording',
        elapsed: 0,
        waveformData: [],
        deviceName: device,
        lastRecordingId: recordingId,
        error: null,
      };
      this.startTimer();
    } catch (e) {
      const message = formatError(e);
      log.error('Failed to start recording', { error: message, device: device ?? 'default' });
      if (this.waveformUnlisten) {
        this.waveformUnlisten();
        this.waveformUnlisten = null;
      }
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

  async stop() {
    if (this.busy) return;
    this.busy = true;
    this.clearTimer();
    // Stop listening for waveform events immediately so the visualizer freezes.
    if (this.waveformUnlisten) {
      this.waveformUnlisten();
      this.waveformUnlisten = null;
    }
    // Optimistically flip the UI to 'stopped' BEFORE awaiting the backend.
    // This makes the Stop button feel instant — the button set swaps from
    // "Pause/Stop/Cancel" to "New Recording" immediately. The backend stop
    // (drain-thread join + encryption + DB insert) runs concurrently and
    // reconciles lastRecordingId when done.
    this.state = {
      ...this.state,
      state: 'stopped',
      lastRecordingId: this.state.lastRecordingId, // keep existing until backend confirms
    };
    try {
      const recordingId = await audioApi.stopRecording();
      log.info('Recording stopped', { recordingId });
      // Reconcile with the actual recording ID from the backend.
      this.state = {
        ...this.state,
        lastRecordingId: recordingId,
      };
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
    if (this.waveformUnlisten) {
      this.waveformUnlisten();
      this.waveformUnlisten = null;
    }
    this.state = { ...initialState };
    this.busy = false;
  }

  reset() {
    this.clearTimer();
    if (this.waveformUnlisten) {
      this.waveformUnlisten();
      this.waveformUnlisten = null;
    }
    this.state = { ...initialState };
  }

  pushWaveform(data: number[]) {
    this.state = {
      ...this.state,
      waveformData: [...this.state.waveformData, ...data].slice(-256),
    };
  }

  /** Recover state from the backend on startup — if a recording is still
   * running (e.g. after a webview reload), rehydrate the store so the Stop
   * button is visible and the timer keeps ticking. */
  async rehydrate() {
    try {
      const snap = await audioApi.getRecordingState();
      if (!snap.active || !snap.recording_id) return;

      // Clean up any prior listener before attaching a new one. Without this,
      // repeated rehydrate calls (HMR, future reconnect flows) would stack
      // listeners and produce duplicate waveform updates.
      if (this.waveformUnlisten) {
        this.waveformUnlisten();
        this.waveformUnlisten = null;
      }
      this.waveformUnlisten = await listen<number[]>('waveform-data', (event) => {
        this.state = {
          ...this.state,
          waveformData: [...this.state.waveformData, ...event.payload].slice(-256),
        };
      });

      const initialElapsed = Math.floor(snap.elapsed_secs ?? 0);
      this.state = {
        ...this.state,
        state: 'recording',
        elapsed: initialElapsed,
        waveformData: [],
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
