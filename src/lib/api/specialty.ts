import { invoke } from '@tauri-apps/api/core';

import type { DocType } from './prompts';

/** One discovered specialty pack (or a broken pack directory) for the
 * Settings picker. Mirrors
 * `medical_processing::specialty::SpecialtyPackInfo`. Carries manifest
 * metadata only — never prompt content. */
export interface SpecialtyPackInfo {
  id: string;
  name: string;
  version: string;
  description: string;
  icon: string | null;
  source: 'bundled' | 'user';
  /** Document types the pack provides prompt artifacts for. */
  provided_prompts: DocType[];
  /** Present when the pack directory failed to load (bad manifest, missing
   *  artifact, duplicate id, …). The pack is not usable. */
  error?: string;
}

export async function listSpecialtyPacks(): Promise<SpecialtyPackInfo[]> {
  return await invoke<SpecialtyPackInfo[]>('list_specialty_packs');
}
