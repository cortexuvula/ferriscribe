import { invoke } from '@tauri-apps/api/core';

/** Manual full bidirectional sync. */
export async function syncContentNow(): Promise<void> {
  await invoke('sync_content_now');
}

/** Subscribe to SSE content change notifications from the server. */
export async function subscribeContentSync(): Promise<void> {
  await invoke('subscribe_content_sync');
}

/** Fetch audio for a recording from the server (on-demand). */
export async function fetchAudioFromServer(recordingId: string): Promise<void> {
  await invoke('fetch_audio_from_server', { recordingId });
}

/** Upload audio for a recording to the server. */
export async function uploadAudioToServer(recordingId: string): Promise<void> {
  await invoke('upload_audio_to_server', { recordingId });
}
