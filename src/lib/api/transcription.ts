import { invoke } from '@tauri-apps/api/core';

export async function transcribeRecording(
  recordingId: string,
  language?: string,
  diarize?: boolean
): Promise<string> {
  return invoke('transcribe_recording', {
    recordingId,
    language: language ?? null,
    diarize: diarize ?? null,
  });
}

export async function listSttProviders(): Promise<[string, boolean][]> {
  return invoke('list_stt_providers');
}
