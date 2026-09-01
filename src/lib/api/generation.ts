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
  urgency?: string,
  context?: string,
): Promise<string> {
  return invokeWithOfflineHandling('generate_referral', {
    recordingId,
    recipientType: recipientType ?? null,
    urgency: urgency ?? null,
    context: context ?? null,
  });
}

export async function generateLetter(
  recordingId: string,
  letterType?: string,
  audienceId?: string,
  context?: string,
): Promise<string> {
  return invokeWithOfflineHandling('generate_letter', {
    recordingId,
    letterType: letterType ?? null,
    audienceId: audienceId ?? null,
    context: context ?? null,
  });
}

/**
 * Standalone Letter Writer: draft a letter from already-extracted document text
 * (typically OCR'd by `ocrDocuments`) plus optional structured fields and
 * freeform writer's instructions. Not tied to a recording; result is ephemeral.
 */
export async function generateLetterFromDocument(
  documentText: string,
  opts: {
    recipient?: string;
    letterType?: string;
    tone?: string;
    reLine?: string;
    userInstructions?: string;
  } = {},
): Promise<string> {
  return invokeWithOfflineHandling('generate_letter_from_document', {
    documentText,
    recipient: opts.recipient ?? null,
    letterType: opts.letterType ?? null,
    tone: opts.tone ?? null,
    reLine: opts.reLine ?? null,
    userInstructions: opts.userInstructions ?? null,
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
  context?: string,
): Promise<string> {
  return invokeWithOfflineHandling('generate_peer_discussion', {
    recordingId,
    physicianName,
    specialty,
    reason,
    context: context ?? null,
  });
}
