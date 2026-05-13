import { invoke } from '@tauri-apps/api/core';
import { invokeWithOfflineHandling } from './invokeWithOfflineHandling';
import type { PatientContext } from '../types';

export async function processRecording(
  recordingId: string,
  context?: string,
  template?: string,
  patientContext?: PatientContext,
): Promise<string> {
  return invokeWithOfflineHandling('process_recording', {
    recordingId,
    context: context ?? null,
    template: template ?? null,
    patientContext: patientContext ?? null,
  });
}

export async function cancelPipeline(recordingId: string): Promise<boolean> {
  return invoke('cancel_pipeline', { recordingId });
}
