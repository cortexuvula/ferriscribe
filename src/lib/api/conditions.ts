import { invoke } from '@tauri-apps/api/core';

export interface ConditionChip {
  id: string;
  text: string;
  updated_at: string;
  deleted_at: string | null;
  sort_order: number;
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

export async function reorderConditionChips(orderedIds: string[]): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('reorder_condition_chips', { orderedIds });
}

export async function syncConditionChips(): Promise<ConditionChip[]> {
  return invoke<ConditionChip[]>('sync_condition_chips_cmd');
}
