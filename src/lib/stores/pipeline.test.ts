import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import type { UnlistenFn } from '@tauri-apps/api/event';

let capturedHandler:
  | ((event: { payload: { recording_id: string; stage: string; error?: string } }) => void)
  | null = null;

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (eventName: string, handler: any) => {
    if (eventName === 'pipeline-progress') {
      capturedHandler = handler;
    }
    const unlisten: UnlistenFn = () => {};
    return unlisten;
  }),
}));

vi.mock('../api/pipeline', () => ({
  processRecording: vi.fn(async () => {}),
  cancelPipeline: vi.fn(async () => true),
}));

const logInfo = vi.fn();
const logError = vi.fn();
vi.mock('../api/logging', () => ({
  log: {
    info: (...args: unknown[]) => logInfo(...args),
    error: (...args: unknown[]) => logError(...args),
    debug: vi.fn(),
    warn: vi.fn(),
  },
}));

const selectRecordingMock = vi.fn(async (_id: string) => {});
const recordingsLoadMock = vi.fn(async () => {});
vi.mock('./recordings', () => ({
  recordings: {
    subscribe: vi.fn(),
    load: (...args: unknown[]) => recordingsLoadMock(...(args as [])),
    search: vi.fn(),
    remove: vi.fn(),
    removeAll: vi.fn(),
  },
  selectRecording: (id: string) => selectRecordingMock(id),
  selectedRecording: {
    subscribe: vi.fn(),
    set: vi.fn(),
    update: vi.fn(),
  },
  loading: { subscribe: vi.fn(), set: vi.fn(), update: vi.fn() },
  searchQuery: { subscribe: vi.fn(), set: vi.fn(), update: vi.fn() },
}));

import { pipeline } from './pipeline';

describe('pipeline auto-select on completion', () => {
  let handler: (event: { payload: { recording_id: string; stage: string; error?: string } }) => void;

  beforeAll(async () => {
    await pipeline.init();
    if (!capturedHandler) throw new Error('pipeline-progress handler was not captured');
    handler = capturedHandler;
  });

  beforeEach(() => {
    selectRecordingMock.mockReset();
    selectRecordingMock.mockResolvedValue(undefined);
    recordingsLoadMock.mockReset();
    recordingsLoadMock.mockResolvedValue(undefined);
    logInfo.mockReset();
    logError.mockReset();
    pipeline.reset();
  });

  it('auto-selects the recording when the most-recently-launched pipeline completes', () => {
    pipeline.launch('rec-1');
    handler({ payload: { recording_id: 'rec-1', stage: 'completed' } });
    expect(selectRecordingMock).toHaveBeenCalledTimes(1);
    expect(selectRecordingMock).toHaveBeenCalledWith('rec-1');
  });

  it('does not auto-select when a non-current pipeline completes', () => {
    pipeline.launch('rec-1');
    pipeline.launch('rec-2');
    handler({ payload: { recording_id: 'rec-1', stage: 'completed' } });
    expect(selectRecordingMock).not.toHaveBeenCalled();
  });

  it('does not auto-select on failure of the current pipeline', () => {
    pipeline.launch('rec-1');
    handler({ payload: { recording_id: 'rec-1', stage: 'failed', error: 'boom' } });
    expect(selectRecordingMock).not.toHaveBeenCalled();
  });

  it('logs and swallows selectRecording rejection', async () => {
    selectRecordingMock.mockRejectedValueOnce(new Error('db down'));
    pipeline.launch('rec-1');
    handler({ payload: { recording_id: 'rec-1', stage: 'completed' } });
    await vi.waitFor(() =>
      expect(logError).toHaveBeenCalledWith(
        'Auto-select after pipeline completion failed',
        expect.objectContaining({
          recording_id: 'rec-1',
          error: expect.stringContaining('db down'),
        }),
      ),
    );
  });
});
