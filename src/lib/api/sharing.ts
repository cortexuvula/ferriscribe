import { invoke } from '@tauri-apps/api/core';

export async function renameClient(id: number, label: string): Promise<void> {
  await invoke('rename_client', { id, label });
}

export async function suggestedClientLabel(): Promise<string> {
  return await invoke<string>('suggested_client_label');
}

/** Connection metadata for a paired office server (mirrors the Rust
 * `PairedConnection`); null when this machine isn't paired. */
export interface PairedConnectionInfo {
  lan: string | null;
  tailscale: string | null;
  ports: {
    ollama: number;
    whisper: number;
    pairing: number;
    lmstudio: number | null;
    omlx: number | null;
    vocab: number | null;
  };
  label: string;
}

/**
 * True when this machine is paired with an office server — providers route
 * through the office's proxies rather than local servers, which changes
 * what "provider offline" means for the user.
 */
export async function isPairedWithServer(): Promise<boolean> {
  const conn = await invoke<PairedConnectionInfo | null>('paired_endpoint');
  return conn !== null;
}
