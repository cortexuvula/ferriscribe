import { invokeWithOfflineHandling } from './invokeWithOfflineHandling';
import { toasts } from '../stores/toasts.svelte';
import { formatError } from '../types/errors';

/** The built-in hotkey fallback, mirroring `DEFAULT_HOTKEY` in
 *  src-tauri/src/commands/screenshot_ocr.rs. Single frontend source for the
 *  Settings label/placeholder and any consumer needing the default. */
export const DEFAULT_OCR_HOTKEY = 'CmdOrCtrl+Alt+O';

/** Outcome of a screenshot-region OCR capture run.
 *  Mirrors `CaptureOcrOutcome` in src-tauri/src/commands/screenshot_ocr.rs. */
export interface CaptureOcrOutcome {
  /** "copied" — text is on the clipboard; "cancelled" — user dismissed the
   *  selection; "empty" — the model found no text; "in_progress" — the
   *  single-flight guard rejected a concurrent trigger (typed no-op). */
  status: 'copied' | 'cancelled' | 'empty' | 'in_progress';
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
    case 'in_progress':
      return '';
  }
}

/** Toast a capture outcome the ONE way every trigger site does: `copied`
 *  is a success toast, expected cancellations/empty extractions are
 *  auto-dismissing notices, and a concurrent-trigger `in_progress` is a
 *  silent no-op. */
export function toastOcrOutcome(outcome: CaptureOcrOutcome): void {
  if (outcome.status === 'in_progress') return;
  const message = captureOutcomeMessage(outcome);
  if (outcome.status === 'copied') {
    toasts.success(message);
  } else {
    toasts.add({ message, type: 'success', autoDismiss: true });
  }
}

/** Toast a capture FAILURE the one way every trigger site does. */
export function toastOcrFailure(err: unknown): void {
  toasts.error(`Screenshot OCR failed: ${formatError(err)}`);
}

/** Run the interactive region capture → OCR → clipboard flow. */
export async function captureRegionOcr(): Promise<CaptureOcrOutcome> {
  return invokeWithOfflineHandling<CaptureOcrOutcome>('capture_region_ocr', {});
}
