import { describe, it, expect, beforeEach, vi } from 'vitest';

// Mock the API + event modules so the store doesn't try to call Tauri.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(vi.fn()),
}));
vi.mock('../api/audio', () => ({
  startRecording: vi.fn().mockResolvedValue('rec-123'),
  stopRecording: vi.fn().mockResolvedValue({
    recording_id: 'rec-123',
    peak: 0.5,
    rms: 0.05,
    duration_secs: 10,
    signal_secs: 9,
    is_silent: false,
    stream_error: null,
    write_error: null,
  }),
  pauseRecording: vi.fn().mockResolvedValue(undefined),
  resumeRecording: vi.fn().mockResolvedValue(undefined),
  cancelRecording: vi.fn().mockResolvedValue(undefined),
  getRecordingState: vi
    .fn()
    .mockResolvedValue({ active: false, recording_id: null, paused: false }),
  listAudioDevices: vi.fn().mockResolvedValue([]),
  checkRecordingAudioLevels: vi.fn().mockResolvedValue(null),
}));
vi.mock('../api/logging', () => ({
  log: { info: vi.fn(), warn: vi.fn(), error: vi.fn() },
}));
vi.mock('../types/errors', () => ({
  formatError: vi.fn((e: unknown) => String(e)),
}));

const { audio } = await import('./audio.svelte');

import { getRecordingState } from '../api/audio';

describe('AudioStore', () => {
  beforeEach(() => {
    audio.reset();
  });

  it('starts in idle state', () => {
    expect(audio.state.state).toBe('idle');
    expect(audio.state.elapsed).toBe(0);
    expect(audio.state.waveformData).toEqual([]);
    expect(audio.state.error).toBeNull();
  });

  it('reset returns to initial state', () => {
    audio.pushWaveform([0.1, 0.2]);
    audio.state = { ...audio.state, elapsed: 42, state: 'stopped' };
    audio.reset();
    expect(audio.state.state).toBe('idle');
    expect(audio.state.elapsed).toBe(0);
    expect(audio.state.waveformData).toEqual([]);
  });

  it('pushWaveform appends and caps at 256 samples', () => {
    audio.pushWaveform([0.1, 0.2, 0.3]);
    expect(audio.state.waveformData).toEqual([0.1, 0.2, 0.3]);
    // Push more than 256 to test the cap
    const big = Array.from({ length: 300 }, (_, i) => i / 300);
    audio.pushWaveform(big);
    expect(audio.state.waveformData.length).toBe(256);
  });

  it('startRecording transitions to recording state', async () => {
    await audio.startRecording('Test Device');
    expect(audio.state.state).toBe('recording');
    expect(audio.state.elapsed).toBe(0);
    expect(audio.state.deviceName).toBe('Test Device');
    expect(audio.state.lastRecordingId).toBe('rec-123');
    expect(audio.state.error).toBeNull();
  });

  it('stop transitions to stopped state', async () => {
    await audio.startRecording();
    await audio.stop();
    expect(audio.state.state).toBe('stopped');
  });

  it('stop keeps the watchdog verdict keyed by its own recording id', async () => {
    await audio.startRecording();
    const id = await audio.stop();
    expect(id).toBe('rec-123');
    expect(audio.state.lastRecordingHealth?.recording_id).toBe('rec-123');
    expect(audio.state.lastRecordingHealth?.is_silent).toBe(false);
  });

  it('startRecording clears a stale verdict from the previous recording', async () => {
    await audio.startRecording();
    await audio.stop();
    expect(audio.state.lastRecordingHealth).not.toBeNull();

    // Next recording must begin without the previous verdict — the
    // silence dialog reads it, and a stale one mis-diagnoses the new one.
    await audio.startRecording();
    expect(audio.state.lastRecordingHealth).toBeNull();
  });

  it('busy is false at rest and exposes the in-flight guard', async () => {
    expect(audio.busy).toBe(false);
    await audio.startRecording();
    expect(audio.busy).toBe(false);
    await audio.stop();
    expect(audio.busy).toBe(false);
  });

  it('pause transitions to paused state', async () => {
    await audio.startRecording();
    await audio.pause();
    expect(audio.state.state).toBe('paused');
  });

  it('resume transitions back to recording state', async () => {
    await audio.startRecording();
    await audio.pause();
    await audio.resume();
    expect(audio.state.state).toBe('recording');
  });

  it('cancel resets to initial state', async () => {
    await audio.startRecording();
    await audio.cancel();
    expect(audio.state.state).toBe('idle');
    expect(audio.state.elapsed).toBe(0);
  });

  it('destroy cleans up without errors', () => {
    audio.pushWaveform([0.5]);
    audio.destroy();
    // Should not throw; state unchanged by destroy (only internal cleanup)
    expect(audio.state.waveformData).toEqual([0.5]);
  });

  it('rehydrate recovers a running orphan recording', async () => {
    vi.mocked(getRecordingState).mockResolvedValueOnce({
      active: true,
      recording_id: 'rec-999',
      elapsed_secs: 65,
      paused: false,
    });
    await audio.rehydrate();
    expect(audio.state.state).toBe('recording');
    expect(audio.state.elapsed).toBe(65);
    expect(audio.state.lastRecordingId).toBe('rec-999');
    audio.destroy();
  });

  it('rehydrate keeps a paused orphan paused without ticking the timer', async () => {
    vi.mocked(getRecordingState).mockResolvedValueOnce({
      active: true,
      recording_id: 'rec-999',
      elapsed_secs: 65,
      paused: true,
    });
    await audio.rehydrate();
    expect(audio.state.state).toBe('paused');
    expect(audio.state.elapsed).toBe(65);
    // The timer must stay stopped while paused — elapsed frozen just over a
    // second later. (Before the paused flag existed, rehydrate always
    // reported 'recording' and started the timer, drifting the display from
    // the pause-aware duration the backend persists.)
    await new Promise((resolve) => setTimeout(resolve, 1100));
    expect(audio.state.elapsed).toBe(65);
    audio.destroy();
  });
});
