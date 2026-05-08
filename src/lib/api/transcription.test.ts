import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { transcribeRecording, listSttProviders } from './transcription';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('transcription api', () => {
  it('transcribeRecording null-coalesces language / diarize when omitted', async () => {
    await transcribeRecording('rec-1');
    expect(invokeMock).toHaveBeenCalledWith('transcribe_recording', {
      recordingId: 'rec-1',
      language: null,
      diarize: null,
    });
  });

  it('transcribeRecording forwards language and diarize when provided', async () => {
    await transcribeRecording('rec-1', 'en', true);
    expect(invokeMock).toHaveBeenCalledWith('transcribe_recording', {
      recordingId: 'rec-1',
      language: 'en',
      diarize: true,
    });
  });

  it('listSttProviders invokes list_stt_providers with no args', async () => {
    await listSttProviders();
    expect(invokeMock).toHaveBeenCalledWith('list_stt_providers');
  });
});
