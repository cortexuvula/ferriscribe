import { ocrDocuments } from '../api/ocr';
import { OfflineCancelled } from '../api/invokeWithOfflineHandling';
import type { OcrFileStatus } from '../components/OcrDropZone.svelte';

/**
 * Shared OCR state and handlers for the Record and Generate tabs.
 *
 * Both tabs previously carried an identical copy of this state machine
 * (per-file chip status, a monotonic batch token to guard against
 * cross-patient PHI leak on context switches, a derived text preview, and
 * a manual override the user edits). Extracting it here removes ~120
 * duplicated lines and guarantees the two tabs stay in sync.
 *
 * Svelte 5 runes ($state / $derived) are legal in a `.svelte.ts` module,
 * so the reactivity survives extraction. Callers reach the reactive values
 * via the returned getters (e.g. `ocr.ocrTextDisplay`); the underlying
 * signals are not exposed.
 */
export function useOcr() {
  let ocrFiles = $state<OcrFileStatus[]>([]);
  let ocrLoading = $state(false);
  let ocrTextOverride = $state<string | null>(null);
  /// Monotonic batch token — incremented when context is cleared. In-flight
  /// OCR callbacks check this token before writing state, preventing cross-
  /// patient PHI leak when the user starts a new recording / switches
  /// recordings during OCR.
  let ocrBatchToken = 0;

  /// Concatenation of all done-file text blocks. The user edits this in the
  /// preview textarea, but we rebuild from chips on removal. Using a derived
  /// value ensures consistency.
  const ocrText = $derived(
    ocrFiles
      .filter((f) => f.status === 'done' && f.text)
      .map((f) => `--- ${f.filename} ---\n${f.text}`)
      .join('\n\n'),
  );

  /// Mutable override of the derived text — the user can edit the preview,
  /// which overrides the derived value until a file is added/removed.
  const ocrTextDisplay = $derived(ocrTextOverride ?? ocrText);

  /**
   * Kick off OCR for the given file paths. Deduplicates paths, adds loading
   * chips immediately, matches results back to chips by filename within the
   * batch, and guards every state write with the batch token so a stale
   * callback (after `clearOcr()` or a context switch) cannot leak PHI into
   * a different patient's context.
   */
  async function handleOcrFilesSelected(paths: string[]) {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- transient dedup Set, not reactive state
    const uniquePaths = [...new Set(paths)];
    if (uniquePaths.length === 0) return;
    ocrLoading = true;
    ocrTextOverride = null; // reset manual edits when new files arrive
    const myToken = ++ocrBatchToken; // Capture token for this batch.
    // Add loading chips immediately so the user sees feedback. Track the chip
    // IDs created by THIS invocation so concurrent drops don't interfere.
    const chipIds: string[] = [];
    const pendingChips = uniquePaths.map((p) => {
      const id = crypto.randomUUID();
      chipIds.push(id);
      const filename = p.split(/[/\\]/).pop() || p;
      return {
        id,
        filename,
        path: p,
        status: 'loading' as const,
        pageCount: 0,
        text: '',
      };
    });
    ocrFiles = [...ocrFiles, ...pendingChips];
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- transient lookup Set, not reactive state
    const idSet = new Set(chipIds);

    try {
      const results = await ocrDocuments(uniquePaths);
      if (myToken !== ocrBatchToken) return; // Stale — context was cleared.
      // Match results to THIS invocation's chips by filename within our batch.
      // The backend returns { filename, text, page_count } per processed file.
      // Filename collisions (same basename from different folders) are a known
      // edge case — the first match wins.
      ocrFiles = ocrFiles.map((f) => {
        if (!idSet.has(f.id)) return f; // not our chip
        const result = results.find((r) => r.filename === f.filename);
        if (result) {
          return {
            ...f,
            status: 'done' as const,
            pageCount: result.page_count,
            text: result.text,
          };
        }
        return { ...f, status: 'error' as const };
      });
    } catch (e) {
      if (myToken !== ocrBatchToken) return; // Stale — context was cleared.
      ocrFiles = ocrFiles.map((f) =>
        idSet.has(f.id) ? { ...f, status: 'error' as const } : f,
      );
      if (!(e instanceof OfflineCancelled)) {
        console.error('OCR failed:', e);
      }
    } finally {
      // Only clear the loading flag if no other batch is in flight.
      ocrLoading = ocrFiles.some((f) => f.status === 'loading');
    }
  }

  function handleOcrTextChange(text: string) {
    ocrTextOverride = text;
  }

  function handleRemoveOcrFile(id: string) {
    ocrFiles = ocrFiles.filter((f) => f.id !== id);
    ocrTextOverride = null; // rebuild from remaining chips
  }

  /**
   * Reset all OCR state and invalidate any in-flight callbacks by bumping
   * the batch token. Called on context switches (new recording / recording
   * change) so OCR text from one patient never leaks into another's context.
   */
  function clearOcr() {
    ocrFiles = [];
    ocrTextOverride = null;
    ocrBatchToken++; // Invalidate any in-flight OCR callbacks.
  }

  return {
    get ocrFiles() {
      return ocrFiles;
    },
    get ocrLoading() {
      return ocrLoading;
    },
    get ocrText() {
      return ocrText;
    },
    get ocrTextDisplay() {
      return ocrTextDisplay;
    },
    handleOcrFilesSelected,
    handleOcrTextChange,
    handleRemoveOcrFile,
    clearOcr,
  };
}
