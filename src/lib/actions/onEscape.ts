/**
 * Svelte action that calls `onclose` when Escape is pressed with capture.
 *
 * Extracted from the three dialogs (ContextTemplateDialog, DictionaryDialog,
 * VocabularyDialog) that each copy-pasted the same addEventListener /
 * removeEventListener pair. Using an action keeps the cleanup tied to the
 * element's lifecycle so the removeEventListener can't be missed.
 *
 * Semantics match the original handlers: capture phase, stopImmediatePropagation
 * so nested editors don't also receive the keydown. Use on <svelte:window>.
 *
 * Usage: `<svelte:window use:onEscape={() => handleClose()} />`
 */
export function onEscape(node: HTMLElement | Window, onclose: () => void) {
  function handler(e: Event) {
    if (e instanceof KeyboardEvent && e.key === 'Escape') {
      e.preventDefault();
      e.stopImmediatePropagation();
      onclose();
    }
  }
  node.addEventListener('keydown', handler, { capture: true });
  return {
    destroy() {
      node.removeEventListener('keydown', handler, { capture: true });
    },
  };
}
