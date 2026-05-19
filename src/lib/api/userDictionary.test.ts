import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { listUserDict, addUserDict, removeUserDict } from './userDictionary';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('userDictionary api', () => {
  it('listUserDict invokes with no args', async () => {
    await listUserDict();
    expect(invokeMock).toHaveBeenCalledWith('user_dict_list');
  });

  it('addUserDict passes word', async () => {
    await addUserDict('metformin');
    expect(invokeMock).toHaveBeenCalledWith('user_dict_add', { word: 'metformin' });
  });

  it('removeUserDict passes word', async () => {
    await removeUserDict('metformin');
    expect(invokeMock).toHaveBeenCalledWith('user_dict_remove', { word: 'metformin' });
  });
});
