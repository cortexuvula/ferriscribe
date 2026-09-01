import type { Recording } from '../types';

/**
 * The four patient-context text fields shown in the Record/Generate tabs,
 * derived from a recording's saved metadata. Each is a newline-joined string
 * so it can bind directly to a `<textarea>`.
 */
export interface ContextFields {
  contextText: string;
  medicationsText: string;
  allergiesText: string;
  conditionsText: string;
}

const EMPTY: ContextFields = {
  contextText: '',
  medicationsText: '',
  allergiesText: '',
  conditionsText: '',
};

/**
 * Map a recording's metadata back into the four context text fields the UI
 * binds to. This is the inverse of {@link buildPatientContext}: it turns the
 * stored `patient_context` lists (and the freeform `context` string) back into
 * the newline-joined textarea values.
 *
 * Used by both the Record tab and the Generate tab when the selected recording
 * changes, so switching to a history entry repopulates its saved context. If
 * metadata is absent/non-object, returns all-empty fields — a fresh recording
 * has no saved context, so the fields start blank.
 *
 * Extracted as a pure function so the mapping is unit-testable and identical
 * across both tabs (previously the Generate tab inlined this logic and the
 * Record tab had no such loading at all — the root cause of the
 * upload-clears-context bug).
 */
export function contextFromMetadata(
  meta: Recording['metadata'] | null | undefined,
): ContextFields {
  if (!meta || typeof meta !== 'object' || Array.isArray(meta)) {
    return { ...EMPTY };
  }

  const pc = meta.patient_context;
  return {
    contextText: typeof meta.context === 'string' ? meta.context : '',
    medicationsText: listToText(pc?.medications),
    allergiesText: listToText(pc?.allergies),
    conditionsText: listToText(pc?.conditions),
  };
}

/** Join a string array into newline-separated textarea text. Empty/missing → ''. */
function listToText(list: string[] | undefined | null): string {
  if (!Array.isArray(list)) return '';
  return list.join('\n');
}
