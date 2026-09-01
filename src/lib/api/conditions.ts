import { invoke } from '@tauri-apps/api/core';

export interface ConditionChip {
  id: string;
  text: string;
  updated_at: string;
  deleted_at: string | null;
  sort_order: number;
  /** Times this chip has been added to a note. Drives frequency ordering. */
  use_count: number;
}

export async function listConditionChips(): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('list_condition_chips');
}

export async function addConditionChip(text: string): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('add_condition_chip', { text });
}

export async function removeConditionChip(text: string): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('remove_condition_chip', { text });
}

/**
 * Increment a chip's use count (called when the condition is added to a note).
 * Returns the active list, reordered by use-count descending. Sync reconciles
 * counts across machines via MAX merge, so this never clobbers a larger count
 * elsewhere.
 */
export async function incrementConditionChipUse(text: string): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('increment_condition_chip_use', { text });
}

export async function syncConditionChips(): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('sync_condition_chips_cmd');
}
