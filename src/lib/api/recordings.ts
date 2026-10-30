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

/** All active recording creation instants, one per recording (duplicates
 *  retained — group them locally to get per-date counts). Timestamps ONLY:
 *  no names, filenames, paths, documents, or metadata ever travel in this
 *  payload. Throws rather than returning a silently empty calendar when a
 *  stored timestamp is unreadable. */
export async function listRecordingCreationTimes(): Promise<string[]> {
  return invoke('list_recording_creation_times');
}

/** Options for queryRecordings — the composite Active-list query. All
 *  fields optional. `startIso`/`endIso` form a half-open [start, end)
 *  creation window and must be passed TOGETHER (an incomplete pair is
 *  rejected by the backend — never silently unfiltered). `search` is
 *  literal-term FTS (hyphens/quotes safe); empty string means no filter.
 *  `limit` defaults to 50 and caps at 200. */
export interface QueryRecordingsOptions {
  search?: string;
  startIso?: string;
  endIso?: string;
  limit?: number;
  offset?: number;
}

/** Composite Active-list query: optional search + optional creation-date
 *  window + paging, all intersected in SQL before LIMIT. Deterministic
 *  newest-first ordering (id tie-break) — the same order with or without
 *  search. Returns summary flags only, never transcript/SOAP documents. */
export async function queryRecordings(
  options: QueryRecordingsOptions = {},
): Promise<RecordingSummary[]> {
  return invoke('query_recordings', {
    search: options.search ?? null,
    startIso: options.startIso ?? null,
    endIso: options.endIso ?? null,
    limit: options.limit ?? 50,
    offset: options.offset ?? 0,
  });
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

/** Preview the same local-clock/encryption-guarded set the purge sweeper uses. */
export async function countSoftDeletedOlderThan(days: number): Promise<number> {
  return invoke('count_soft_deleted_older_than', { days });
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
