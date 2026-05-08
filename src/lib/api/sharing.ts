import { invoke } from '@tauri-apps/api/core';

export async function renameClient(id: number, label: string): Promise<void> {
  await invoke('rename_client', { id, label });
}

export async function suggestedClientLabel(): Promise<string> {
  return await invoke<string>('suggested_client_label');
}
