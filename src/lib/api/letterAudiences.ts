import { invoke } from '@tauri-apps/api/core';
import type { LetterAudience } from '../types/letterAudience';

export async function listLetterAudiences(): Promise<LetterAudience[]> {
  return invoke('list_letter_audiences');
}

export async function upsertLetterAudience(
  audience: LetterAudience
): Promise<LetterAudience> {
  return invoke('upsert_letter_audience', { audience });
}

export async function deleteLetterAudience(id: string): Promise<void> {
  return invoke('delete_letter_audience', { id });
}
