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

/** Result of any bulk restore path: the ACTUAL count restored (may be lower
 *  than a preview when a purge or concurrent restore intervened). */
export interface RestoreResult {
  count: number;
  ids: string[];
}

/** Restore recordings by exact id set — the batch Undo for Move-all-to-Trash. */
export async function restoreRecordings(ids: string[]): Promise<RestoreResult> {
  return invoke('restore_recordings', { ids });
}

/** Result of "Move all to Trash": the count plus the exact id set trashed.
 *  The Undo toast restores exactly these ids — never "everything deleted
 *  since T", which could sweep in a later, unrelated deletion. */
export interface DeleteAllResult {
  count: number;
  ids: string[];
}

export async function deleteAllRecordings(): Promise<DeleteAllResult> {
  return invoke('delete_all_recordings');
}

/** Authoritative count of active (non-trashed) recordings. The recordings
 *  list is a paginated subset — dialogs promising "all N recordings" must
 *  use this number, never `list.length`. */
export async function countRecordings(): Promise<number> {
  return invoke('count_recordings');
}

export async function importAudioFile(filePath: string): Promise<string> {
  return invoke('import_audio_file', { filePath });
}
