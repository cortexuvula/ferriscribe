import { invoke } from '@tauri-apps/api/core';

/**
 * A downloadable STT model (whisper / pyannote). Named `DownloadableModel`
 * (was `ModelInfo`) so it can't be confused with the AI-provider model
 * entry of the same name in `api/chat.ts`.
 */
export interface DownloadableModel {
  id: string;
  filename: string;
  size_bytes: number;
  download_url: string;
  description: string;
  downloaded: boolean;
}

export async function listWhisperModels(): Promise<DownloadableModel[]> {
  return invoke('list_whisper_models');
}

export async function listPyannoteModels(): Promise<DownloadableModel[]> {
  return invoke('list_pyannote_models');
}

export async function downloadModel(modelId: string): Promise<void> {
  return invoke('download_model', { modelId });
}

export async function deleteModel(modelId: string): Promise<void> {
  return invoke('delete_model', { modelId });
}
