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

/** Manual full bidirectional dictionary sync. Returns the active word list. */
export async function syncUserDictionary(): Promise<string[]> {
  return invoke('sync_user_dictionary_cmd');
}

/** Subscribe to SSE user-dictionary change notifications from the server. */
export async function subscribeUserDictionary(): Promise<void> {
  await invoke('subscribe_user_dictionary');
}
