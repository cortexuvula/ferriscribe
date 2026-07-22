import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
  listConditionChips,
  addConditionChip,
  removeConditionChip,
  reorderConditionChips,
  syncConditionChips,
} from './conditions';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue([]);
});

describe('conditions api', () => {
  it('listConditionChips invokes with no args', async () => {
    await listConditionChips();
    expect(invokeMock).toHaveBeenCalledWith('list_condition_chips');
  });

  it('addConditionChip passes text', async () => {
    await addConditionChip('Hypertension');
    expect(invokeMock).toHaveBeenCalledWith('add_condition_chip', { text: 'Hypertension' });
  });

  it('removeConditionChip passes text', async () => {
    await removeConditionChip('Diabetes');
    expect(invokeMock).toHaveBeenCalledWith('remove_condition_chip', { text: 'Diabetes' });
  });

  it('reorderConditionChips passes orderedIds', async () => {
    const ids = ['chip-1', 'chip-2', 'chip-3'];
    await reorderConditionChips(ids);
    expect(invokeMock).toHaveBeenCalledWith('reorder_condition_chips', { orderedIds: ids });
  });

  it('syncConditionChips invokes with no args', async () => {
    await syncConditionChips();
    expect(invokeMock).toHaveBeenCalledWith('sync_condition_chips_cmd');
  });
});
