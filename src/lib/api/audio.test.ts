import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
  listAudioDevices,
  startRecording,
  stopRecording,
  cancelRecording,
  pauseRecording,
  resumeRecording,
  checkRecordingAudioLevels,
  getRecordingState,
} from './audio';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('audio api', () => {
  it('listAudioDevices invokes list_audio_devices with no args', async () => {
    await listAudioDevices();
    expect(invokeMock).toHaveBeenCalledWith('list_audio_devices');
  });

  it('startRecording / stopRecording / cancelRecording / pauseRecording / resumeRecording invoke their commands with no args', async () => {
    await startRecording();
    await stopRecording();
    await cancelRecording();
    await pauseRecording();
    await resumeRecording();
    expect(invokeMock).toHaveBeenNthCalledWith(1, 'start_recording');
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'stop_recording');
    expect(invokeMock).toHaveBeenNthCalledWith(3, 'cancel_recording');
    expect(invokeMock).toHaveBeenNthCalledWith(4, 'pause_recording');
    expect(invokeMock).toHaveBeenNthCalledWith(5, 'resume_recording');
  });

  it('checkRecordingAudioLevels passes recordingId in camelCase', async () => {
    await checkRecordingAudioLevels('rec-1');
    expect(invokeMock).toHaveBeenCalledWith('check_recording_audio_levels', { recordingId: 'rec-1' });
  });

  it('getRecordingState invokes get_recording_state with no args', async () => {
    await getRecordingState();
    expect(invokeMock).toHaveBeenCalledWith('get_recording_state');
  });
});
