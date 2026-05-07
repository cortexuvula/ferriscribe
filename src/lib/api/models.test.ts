import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { listWhisperModels, listPyannoteModels, downloadModel, deleteModel } from './models';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('models api', () => {
  it('listWhisperModels invokes list_whisper_models', async () => {
    await listWhisperModels();
    expect(invokeMock).toHaveBeenCalledWith('list_whisper_models');
  });

  it('listPyannoteModels invokes list_pyannote_models', async () => {
    await listPyannoteModels();
    expect(invokeMock).toHaveBeenCalledWith('list_pyannote_models');
  });

  it('downloadModel passes modelId in camelCase', async () => {
    await downloadModel('ggml-large-v3-turbo');
    expect(invokeMock).toHaveBeenCalledWith('download_model', { modelId: 'ggml-large-v3-turbo' });
  });

  it('deleteModel passes modelId in camelCase', async () => {
    await deleteModel('ggml-base');
    expect(invokeMock).toHaveBeenCalledWith('delete_model', { modelId: 'ggml-base' });
  });
});
