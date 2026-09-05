import { invokeWithOfflineHandling } from './invokeWithOfflineHandling';

/** Outcome of a screenshot-region OCR capture run.
 *  Mirrors `CaptureOcrOutcome` in src-tauri/src/commands/screenshot_ocr.rs. */
export interface CaptureOcrOutcome {
  /** "copied" — text is on the clipboard; "cancelled" — user dismissed the
   *  selection; "empty" — the model found no text. */
  status: 'copied' | 'cancelled' | 'empty';
  /** Extracted character count (0 unless copied). */
  chars: number;
}

/** Human summary for toasts — outcome only, never content. */
export function captureOutcomeMessage(outcome: CaptureOcrOutcome): string {
  switch (outcome.status) {
    case 'copied':
      return `OCR text copied to clipboard (${outcome.chars} characters)`;
    case 'cancelled':
      return 'Region selection cancelled';
    case 'empty':
      return 'No text found in the selected region';
  }
}

/** Run the interactive region capture → OCR → clipboard flow. */
export async function captureRegionOcr(): Promise<CaptureOcrOutcome> {
  return invokeWithOfflineHandling<CaptureOcrOutcome>('capture_region_ocr', {});
}
