import { invokeWithOfflineHandling } from './invokeWithOfflineHandling';
import type { PatientContext } from '../types';

export async function generateSoap(
  recordingId: string,
  template?: string,
  context?: string,
  patientContext?: PatientContext,
): Promise<string> {
  // Tauri omits undefined fields from the payload, so explicitly pass null
  // for optional parameters to ensure they map to Rust Option::None
  return invokeWithOfflineHandling('generate_soap', {
    recordingId,
    template: template ?? null,
    context: context ?? null,
    patientContext: patientContext ?? null,
  });
}

export async function generateReferral(
  recordingId: string,
  recipientType?: string,
  urgency?: string
): Promise<string> {
  return invokeWithOfflineHandling('generate_referral', {
    recordingId,
    recipientType: recipientType ?? null,
    urgency: urgency ?? null,
  });
}

export async function generateLetter(
  recordingId: string,
  letterType?: string,
  audienceId?: string
): Promise<string> {
  return invokeWithOfflineHandling('generate_letter', {
    recordingId,
    letterType: letterType ?? null,
    audienceId: audienceId ?? null,
  });
}

export async function generateSynopsis(
  recordingId: string
): Promise<string> {
  return invokeWithOfflineHandling('generate_synopsis', { recordingId });
}

export async function generatePeerDiscussion(
  recordingId: string,
  physicianName: string,
  specialty: string,
  reason: string,
): Promise<string> {
  return invokeWithOfflineHandling('generate_peer_discussion', {
    recordingId,
    physicianName,
    specialty,
    reason,
  });
}
