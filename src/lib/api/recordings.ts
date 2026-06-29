import { invoke } from '@tauri-apps/api/core';
import type { Recording, RecordingSummary } from '../types';

export async function listRecordings(limit = 50, offset = 0): Promise<RecordingSummary[]> {
  return invoke('list_recordings', { limit, offset });
}

export async function getRecording(id: string): Promise<Recording> {
  return invoke('get_recording', { id });
}

export async function searchRecordings(query: string, limit = 20): Promise<Recording[]> {
  return invoke('search_recordings', { query, limit });
}

export async function deleteRecording(id: string): Promise<void> {
  return invoke('delete_recording', { id });
}

export async function restoreRecording(id: string): Promise<void> {
  return invoke('restore_recording', { id });
}

export async function deleteAllRecordings(): Promise<number> {
  return invoke('delete_all_recordings');
}

export async function importAudioFile(filePath: string): Promise<string> {
  return invoke('import_audio_file', { filePath });
}
