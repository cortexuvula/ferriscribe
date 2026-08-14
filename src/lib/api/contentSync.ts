import { invoke } from '@tauri-apps/api/core';

/**
 * Counts-only summary of a content-sync round (mirrors `SyncSummaryPayload`
 * in src-tauri/src/commands/content_sync.rs — no PHI).
 *
 * `disabled` is true when the sync was skipped entirely because a gate
 * failed (sync off, no Tailscale address, unpaired, no token) — previously
 * indistinguishable in the UI from "synced, nothing changed".
 */
export interface SyncSummary {
  pulled: number;
  pushed: number;
  merge_conflicts: number;
  push_conflicts: number;
  disabled: boolean;
}

/** Manual full bidirectional sync. */
export async function syncContentNow(): Promise<SyncSummary> {
  return invoke('sync_content_now');
}

/** Subscribe to SSE content change notifications from the server. */
export async function subscribeContentSync(): Promise<void> {
  await invoke('subscribe_content_sync');
}

/** Fetch audio for a recording from the server (on-demand). */
export async function fetchAudioFromServer(recordingId: string): Promise<void> {
  await invoke('fetch_audio_from_server', { recordingId });
}
