import { invoke } from '@tauri-apps/api/core';
import type { Recording, RecordingSummary, TrashedRecordingSummary } from '../types';

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

/** One page of the Trash view plus the authoritative total. */
export interface TrashedListResult {
  items: TrashedRecordingSummary[];
  total: number;
}

/** List trashed recordings, newest deletion first. `total` is the
 *  authoritative trashed count (badge / "M in Trash" copy) — independent
 *  of pagination. */
export async function listTrashedRecordings(
  limit = 50,
  offset = 0,
): Promise<TrashedListResult> {
  return invoke('list_trashed_recordings', { limit, offset });
}

/** Restore EVERY recording in Trash. Returns the actual restored count. */
export async function restoreAllTrashed(): Promise<RestoreResult> {
  return invoke('restore_all_trashed');
}

/** Preview count for restore-by-date: how many recordings in Trash were
 *  moved there inside the half-open [startIso, endIso) window. Always the
 *  dedicated count command — never derived from paginated trash pages. */
export async function countRecordingsDeletedBetween(
  startIso: string,
  endIso: string,
): Promise<number> {
  return invoke('count_recordings_deleted_between', { startIso, endIso });
}

/** Restore every recording moved to Trash inside the half-open
 *  [startIso, endIso) window. Returns the ACTUAL restored count (a purge
 *  or concurrent restore may have changed the set since the preview). */
export async function restoreRecordingsDeletedBetween(
  startIso: string,
  endIso: string,
): Promise<RestoreResult> {
  return invoke('restore_recordings_deleted_between', { startIso, endIso });
}

export async function importAudioFile(filePath: string): Promise<string> {
  return invoke('import_audio_file', { filePath });
}
