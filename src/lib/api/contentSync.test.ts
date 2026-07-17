import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';

import { syncContentNow, subscribeContentSync, fetchAudioFromServer } from './contentSync';

const invokeMock = vi.mocked(invoke);

describe('contentSync API', () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  describe('syncContentNow', () => {
    it('calls sync_content_now command with no args', async () => {
      invokeMock.mockResolvedValueOnce(undefined);
      await syncContentNow();
      expect(invokeMock).toHaveBeenCalledWith('sync_content_now');
    });
  });

  describe('subscribeContentSync', () => {
    it('calls subscribe_content_sync command with no args', async () => {
      invokeMock.mockResolvedValueOnce(undefined);
      await subscribeContentSync();
      expect(invokeMock).toHaveBeenCalledWith('subscribe_content_sync');
    });
  });

  describe('fetchAudioFromServer', () => {
    it('calls fetch_audio_from_server with recordingId', async () => {
      invokeMock.mockResolvedValueOnce('/path/to/audio.enc');
      await fetchAudioFromServer('abc-123');
      expect(invokeMock).toHaveBeenCalledWith('fetch_audio_from_server', {
        recordingId: 'abc-123',
      });
    });
  });
});
