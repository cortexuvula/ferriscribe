import { useOcr } from '../composables/useOcr.svelte';
import { generateLetterFromDocument } from '../api/generation';
import { copyWithStatus } from '../utils/clipboard';
import { formatError } from '../types/errors';
import { OfflineCancelled } from '../api/invokeWithOfflineHandling';
import { toasts } from './toasts.svelte';

/**
 * Singleton store backing the Letter Writer tab.
 *
 * State lives here (not in the component) because the tab is conditionally
 * rendered with `{#if activeTab === 'letter_writer'}`, which unmounts the
 * component on every tab switch. Keeping the OCR'd source text, pasted text,
 * the structured fields, the writer's instructions, and the generated letter
 * here means a user can OCR a document, pop into another tab, and come back
 * without losing their work. The single `useOcr()` instance is owned here
 * too, so its batch token and any in-flight OCR (or generation) survive the
 * round trip.
 */
class LetterWriterStore {
  // Source document inputs + structured fields (persisted across tab switches).
  pastedText = $state('');
  recipient = $state('');
  letterType = $state('');
  tone = $state('Formal');
  reLine = $state('');
  userInstructions = $state('');

  // Generated letter (persisted so a tab switch mid-generation keeps the result
  // — the in-flight promise resolves into this field whenever it completes).
  output = $state('');

  // Transient generation/copy UI state.
  generating = $state(false);
  error = $state<string | null>(null);
  copyStatus = $state<'idle' | 'copying' | 'copied'>('idle');

  // One OCR composable instance for the life of the app. Owned here so OCR'd
  // text and the batch token survive tab switches.
  readonly ocr = useOcr();

  // The source document sent to the backend: OCR'd text and pasted text are
  // both optional; when both are present they are joined into one document.
  documentText = $derived(
    [this.ocr.ocrTextDisplay.trim(), this.pastedText.trim()].filter(Boolean).join('\n\n'),
  );

  canGenerate = $derived(this.documentText.length > 0 && !this.generating);

  async handleGenerate(): Promise<void> {
    if (!this.canGenerate) return;
    this.generating = true;
    this.error = null;
    // NOTE: the previous letter is deliberately kept visible during
    // generation and only replaced on success — clearing it up-front
    // destroyed the user's letter (including manual edits) when the
    // request was cancelled (offline dialog) without producing anything.
    try {
      const letter = await generateLetterFromDocument(this.documentText, {
        recipient: this.recipient.trim() || undefined,
        letterType: this.letterType || undefined,
        tone: this.tone || undefined,
        reLine: this.reLine.trim() || undefined,
        userInstructions: this.userInstructions.trim() || undefined,
      });
      this.output = letter;
      toasts.success('Letter generated');
    } catch (e) {
      if (e instanceof OfflineCancelled) {
        // The offline dialog already informed the user; stay quiet (no banner).
        return;
      }
      this.error = formatError(e) || 'Failed to generate letter';
    } finally {
      this.generating = false;
    }
  }

  async handleCopy(): Promise<void> {
    if (this.copyStatus !== 'idle' || !this.output) return;
    await copyWithStatus({
      setStatus: (s) => (this.copyStatus = s),
      getText: () => this.output,
    });
  }

  handleClearAll(): void {
    this.ocr.clearOcr();
    this.pastedText = '';
    this.recipient = '';
    this.letterType = '';
    this.tone = 'Formal';
    this.reLine = '';
    this.userInstructions = '';
    this.output = '';
    this.error = null;
  }
}

export const letterWriter = new LetterWriterStore();
