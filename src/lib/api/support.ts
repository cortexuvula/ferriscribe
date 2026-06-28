import { invoke } from '@tauri-apps/api/core';

/** Export PHI-redacted application logs to a user-chosen file path. */
export async function exportSupportBundle(filePath: string): Promise<void> {
  await invoke('export_support_bundle', { filePath });
}
