import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
  listRecordings,
  getRecording,
  searchRecordings,
  deleteRecording,
  deleteAllRecordings,
  importAudioFile,
} from './recordings';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('recordings api', () => {
  it('listRecordings defaults limit=50, offset=0 when called with no args', async () => {
    await listRecordings();
    expect(invokeMock).toHaveBeenCalledWith('list_recordings', { limit: 50, offset: 0 });
  });

  it('listRecordings forwards explicit limit + offset', async () => {
    await listRecordings(10, 100);
    expect(invokeMock).toHaveBeenCalledWith('list_recordings', { limit: 10, offset: 100 });
  });

  it('getRecording / deleteRecording pass id', async () => {
    await getRecording('rec-1');
    expect(invokeMock).toHaveBeenLastCalledWith('get_recording', { id: 'rec-1' });
    await deleteRecording('rec-1');
    expect(invokeMock).toHaveBeenLastCalledWith('delete_recording', { id: 'rec-1' });
  });

  it('searchRecordings defaults limit=20', async () => {
    await searchRecordings('cough');
    expect(invokeMock).toHaveBeenCalledWith('search_recordings', { query: 'cough', limit: 20 });
    invokeMock.mockReset();
    await searchRecordings('cough', 5);
    expect(invokeMock).toHaveBeenCalledWith('search_recordings', { query: 'cough', limit: 5 });
  });

  it('deleteAllRecordings invokes with no args', async () => {
    await deleteAllRecordings();
    expect(invokeMock).toHaveBeenCalledWith('delete_all_recordings');
  });

  it('importAudioFile passes filePath in camelCase', async () => {
    await importAudioFile('/tmp/audio.mp3');
    expect(invokeMock).toHaveBeenCalledWith('import_audio_file', { filePath: '/tmp/audio.mp3' });
  });
});
