import { invoke } from '@tauri-apps/api/core';

export async function listUserDict(): Promise<string[]> {
  return invoke('user_dict_list');
}

export async function addUserDict(word: string): Promise<boolean> {
  return invoke('user_dict_add', { word });
}

export async function removeUserDict(word: string): Promise<boolean> {
  return invoke('user_dict_remove', { word });
}
