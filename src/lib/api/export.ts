import { invoke } from '@tauri-apps/api/core';

export async function exportPdf(
  recordingId: string,
  exportType: 'soap' | 'referral' | 'letter'
): Promise<number[]> {
  return invoke('export_pdf', { recordingId, exportType });
}

export async function exportDocx(
  recordingId: string,
  exportType: 'soap' | 'referral' | 'letter'
): Promise<number[]> {
  return invoke('export_docx', { recordingId, exportType });
}

export async function exportFhir(recordingId: string): Promise<number[]> {
  return invoke('export_fhir', { recordingId });
}

/** Export the recording's audio as a standard 16-bit PCM WAV file. */
export async function exportAudio(recordingId: string, filePath: string): Promise<void> {
  await invoke('export_audio', { recordingId, filePath });
}
