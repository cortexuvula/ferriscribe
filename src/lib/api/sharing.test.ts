import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { renameClient, suggestedClientLabel } from './sharing';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
});

describe('sharing api', () => {
  it('renameClient passes id and label as camelCase keys', async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await renameClient(7, 'Dr. Patel — Room 4');
    expect(invokeMock).toHaveBeenCalledWith('rename_client', {
      id: 7,
      label: 'Dr. Patel — Room 4',
    });
  });

  it('renameClient propagates backend errors', async () => {
    invokeMock.mockRejectedValueOnce(new Error('label cannot be empty'));
    await expect(renameClient(7, '   ')).rejects.toThrow('label cannot be empty');
  });

  it('suggestedClientLabel resolves to the backend string', async () => {
    invokeMock.mockResolvedValueOnce('cortex-mbp');
    await expect(suggestedClientLabel()).resolves.toBe('cortex-mbp');
    expect(invokeMock).toHaveBeenCalledWith('suggested_client_label');
  });
});
