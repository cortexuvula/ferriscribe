import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
  listRecordings,
  getRecording,
  searchRecordings,
  deleteRecording,
  deleteAllRecordings,
  countRecordings,
  restoreRecordings,
  listTrashedRecordings,
  restoreAllTrashed,
  restoreRecordingsDeletedBetween,
  countRecordingsDeletedBetween,
  countRecordingsCreatedBetween,
  deleteRecordingsCreatedBetween,
  importAudioFile,
  listRecordingCreationTimes,
  queryRecordings,
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

  it('countRecordings invokes the authoritative active count command', async () => {
    invokeMock.mockResolvedValueOnce(3);
    await expect(countRecordings()).resolves.toBe(3);
    expect(invokeMock).toHaveBeenCalledWith('count_recordings');
  });

  it('restoreRecordings passes the exact id set', async () => {
    invokeMock.mockResolvedValueOnce({ count: 2, ids: ['a', 'b'] });
    await expect(restoreRecordings(['a', 'b'])).resolves.toEqual({
      count: 2,
      ids: ['a', 'b'],
    });
    expect(invokeMock).toHaveBeenCalledWith('restore_recordings', {
      ids: ['a', 'b'],
    });
  });

  it('listTrashedRecordings defaults limit=50, offset=0 and returns items+total', async () => {
    invokeMock.mockResolvedValueOnce({ items: [], total: 3 });
    await expect(listTrashedRecordings()).resolves.toEqual({ items: [], total: 3 });
    expect(invokeMock).toHaveBeenCalledWith('list_trashed_recordings', {
      limit: 50,
      offset: 0,
    });
  });

  it('restoreAllTrashed invokes with no args', async () => {
    invokeMock.mockResolvedValueOnce({ count: 0, ids: [] });
    await restoreAllTrashed();
    expect(invokeMock).toHaveBeenCalledWith('restore_all_trashed');
  });

  it('count/restore by deletion date pass both interval bounds', async () => {
    invokeMock.mockResolvedValueOnce(4);
    await expect(
      countRecordingsDeletedBetween('2026-10-01T00:00:00Z', '2026-10-02T00:00:00Z'),
    ).resolves.toBe(4);
    expect(invokeMock).toHaveBeenCalledWith('count_recordings_deleted_between', {
      startIso: '2026-10-01T00:00:00Z',
      endIso: '2026-10-02T00:00:00Z',
    });

    invokeMock.mockResolvedValueOnce({ count: 4, ids: [] });
    await restoreRecordingsDeletedBetween('2026-10-01T00:00:00Z', '2026-10-02T00:00:00Z');
    expect(invokeMock).toHaveBeenCalledWith('restore_recordings_deleted_between', {
      startIso: '2026-10-01T00:00:00Z',
      endIso: '2026-10-02T00:00:00Z',
    });
  });

  it('count/delete by created date pass both interval bounds and return the result shape', async () => {
    invokeMock.mockResolvedValueOnce(3);
    await expect(
      countRecordingsCreatedBetween('2026-10-01T00:00:00Z', '2026-10-04T00:00:00Z'),
    ).resolves.toBe(3);
    expect(invokeMock).toHaveBeenCalledWith('count_recordings_created_between', {
      startIso: '2026-10-01T00:00:00Z',
      endIso: '2026-10-04T00:00:00Z',
    });

    invokeMock.mockResolvedValueOnce({ count: 3, ids: ['a', 'b', 'c'] });
    await expect(
      deleteRecordingsCreatedBetween('2026-10-01T00:00:00Z', '2026-10-04T00:00:00Z'),
    ).resolves.toEqual({ count: 3, ids: ['a', 'b', 'c'] });
    expect(invokeMock).toHaveBeenCalledWith('delete_recordings_created_between', {
      startIso: '2026-10-01T00:00:00Z',
      endIso: '2026-10-04T00:00:00Z',
    });
  });

  it('importAudioFile passes filePath in camelCase', async () => {
    await importAudioFile('/tmp/audio.mp3');
    expect(invokeMock).toHaveBeenCalledWith('import_audio_file', { filePath: '/tmp/audio.mp3' });
  });

  it('listRecordingCreationTimes invokes with no args and returns timestamp strings', async () => {
    const times = ['2026-10-01T09:00:00+00:00', '2026-10-01T09:00:00+00:00'];
    invokeMock.mockResolvedValueOnce(times);
    await expect(listRecordingCreationTimes()).resolves.toBe(times);
    expect(invokeMock).toHaveBeenCalledWith('list_recording_creation_times');
  });

  it('queryRecordings defaults: no filters, limit 50, offset 0, null bounds', async () => {
    invokeMock.mockResolvedValueOnce([]);
    await expect(queryRecordings()).resolves.toEqual([]);
    expect(invokeMock).toHaveBeenCalledWith('query_recordings', {
      search: null,
      startIso: null,
      endIso: null,
      limit: 50,
      offset: 0,
    });
  });

  it('queryRecordings forwards search + date interval + explicit paging', async () => {
    invokeMock.mockResolvedValueOnce([]);
    await queryRecordings({
      search: 'covid-19',
      startIso: '2026-10-01T00:00:00Z',
      endIso: '2026-10-02T00:00:00Z',
      limit: 200,
      offset: 50,
    });
    expect(invokeMock).toHaveBeenCalledWith('query_recordings', {
      search: 'covid-19',
      startIso: '2026-10-01T00:00:00Z',
      endIso: '2026-10-02T00:00:00Z',
      limit: 200,
      offset: 50,
    });
  });

  it('queryRecordings sends only one interval half when the other is omitted', async () => {
    // The backend rejects an incomplete pair — the wrapper must not invent
    // the missing half, so the rejection stays observable to the caller.
    invokeMock.mockResolvedValueOnce([]);
    await queryRecordings({ startIso: '2026-10-01T00:00:00Z' });
    expect(invokeMock).toHaveBeenCalledWith('query_recordings', {
      search: null,
      startIso: '2026-10-01T00:00:00Z',
      endIso: null,
      limit: 50,
      offset: 0,
    });
  });
});
