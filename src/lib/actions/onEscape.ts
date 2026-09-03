/**
 * Svelte action that closes an overlay on Escape, capture phase.
 *
 * Used on <svelte:window> by the three settings-manager dialogs
 * (ContextTemplateDialog, DictionaryDialog, VocabularyDialog). The
 * callback returns whether it HANDLED the event; propagation is stopped
 * (preventDefault + stopImmediatePropagation) only when it did. The stop
 * used to be unconditional, so a mounted-but-closed dialog swallowed
 * Escape for everything behind it — the Settings modal couldn't be
 * Escape-closed while these components were mounted.
 *
 * Capture phase (plus the stop when handled) means nested editors don't
 * also receive the keydown.
 *
 * Usage: `<svelte:window use:onEscape={handleEscape} />` where
 * `handleEscape(): boolean` returns true when it closed something.
 */
export function onEscape(node: HTMLElement | Window, onclose: () => boolean) {
  function handler(e: Event) {
    if (e instanceof KeyboardEvent && e.key === 'Escape' && onclose()) {
      e.preventDefault();
      e.stopImmediatePropagation();
    }
  }
  node.addEventListener('keydown', handler, { capture: true });
  return {
    destroy() {
      node.removeEventListener('keydown', handler, { capture: true });
    },
  };
}
