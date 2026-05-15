import { writable, get } from 'svelte/store';
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

function createAudioStore() {
  const store = writable<AudioStoreState>(initialState);
  const { subscribe, set, update } = store;

  let timer: ReturnType<typeof setInterval> | null = null;
  let waveformUnlisten: UnlistenFn | null = null;
  let busy = false;

  function clearTimer() {
    if (timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  }

  function startTimer() {
    clearTimer();
    timer = setInterval(() => {
      update((s) => ({ ...s, elapsed: s.elapsed + 1 }));
    }, 1000);
  }

  return {
    subscribe,

    async startRecording(device: string | null = null) {
      if (busy) return;
      busy = true;
      try {
        // Clean up any stale listener before attaching a new one
        if (waveformUnlisten) { waveformUnlisten(); waveformUnlisten = null; }
        // Listen for waveform events BEFORE starting recording
        waveformUnlisten = await listen<number[]>('waveform-data', (event) => {
          update((s) => ({
            ...s,
            waveformData: [...s.waveformData, ...event.payload].slice(-256),
          }));
        });

        const recordingId = await audioApi.startRecording();
        log.info('Recording started', { recordingId, device: device ?? 'default' });
        update((s) => ({
          ...s,
          state: 'recording',
          elapsed: 0,
          waveformData: [],
          deviceName: device,
          lastRecordingId: recordingId,
          error: null,
        }));
        startTimer();
      } catch (e) {
        const message = formatError(e);
        log.error('Failed to start recording', { error: message, device: device ?? 'default' });
        if (waveformUnlisten) {
          waveformUnlisten();
          waveformUnlisten = null;
        }
        update((s) => ({
          ...s,
          error: message || 'Failed to start recording',
        }));
      } finally {
        busy = false;
      }
    },

    async pause() {
      try {
        await audioApi.pauseRecording();
        clearTimer();
        update((s) => ({ ...s, state: 'paused' }));
      } catch (e) {
        update((s) => ({
          ...s,
          error: formatError(e) || 'Failed to pause',
        }));
      }
    },

    async resume() {
      try {
        await audioApi.resumeRecording();
        update((s) => ({ ...s, state: 'recording' }));
        startTimer();
      } catch (e) {
        update((s) => ({
          ...s,
          error: formatError(e) || 'Failed to resume',
        }));
      }
    },

    async stop() {
      if (busy) return;
      busy = true;
      // Capture pre-stop state so we only restore the timer if we were
      // actively recording (not paused).
      const wasRecording = get(store).state === 'recording';
      clearTimer();
      try {
        const recordingId = await audioApi.stopRecording();
        log.info('Recording stopped', { recordingId });
        if (waveformUnlisten) {
          waveformUnlisten();
          waveformUnlisten = null;
        }
        update((s) => ({
          ...s,
          state: 'stopped',
          lastRecordingId: recordingId,
        }));
      } catch (e) {
        const message = formatError(e);
        log.error('Failed to stop recording', { error: message });
        if (waveformUnlisten) {
          waveformUnlisten();
          waveformUnlisten = null;
        }
        // Don't change state to 'stopped' on error — backend may still be recording
        update((s) => ({
          ...s,
          error: message || 'Failed to stop recording',
        }));
        if (wasRecording) startTimer(); // Only restore timer if we were actively recording
      } finally {
        busy = false;
      }
    },

    async cancel() {
      if (busy) return;
      busy = true;
      clearTimer();
      try {
        await audioApi.cancelRecording();
      } catch (_e) {
        // Best-effort — even if backend fails, reset the frontend state
      }
      if (waveformUnlisten) {
        waveformUnlisten();
        waveformUnlisten = null;
      }
      set(initialState);
      busy = false;
    },

    reset() {
      clearTimer();
      if (waveformUnlisten) {
        waveformUnlisten();
        waveformUnlisten = null;
      }
      set(initialState);
    },

    pushWaveform(data: number[]) {
      update((s) => ({
        ...s,
        waveformData: [...s.waveformData, ...data].slice(-256),
      }));
    },

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
        if (waveformUnlisten) {
          waveformUnlisten();
          waveformUnlisten = null;
        }
        waveformUnlisten = await listen<number[]>('waveform-data', (event) => {
          update((s) => ({
            ...s,
            waveformData: [...s.waveformData, ...event.payload].slice(-256),
          }));
        });

        const initialElapsed = Math.floor(snap.elapsed_secs ?? 0);
        update((s) => ({
          ...s,
          state: 'recording',
          elapsed: initialElapsed,
          waveformData: [],
          lastRecordingId: snap.recording_id,
          error: null,
        }));
        startTimer();
        log.info('Recovered orphan recording after reload', {
          recordingId: snap.recording_id,
          elapsedSecs: initialElapsed,
        });
      } catch (e) {
        log.warn('Could not query recording state on startup', { error: formatError(e) });
      }
    },
  };
}

export const audio = createAudioStore();
