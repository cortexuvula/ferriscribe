/**
 * Shared overlay plumbing for Modal and ConfirmDialog:
 *
 * - An open-instance stack so only the TOPMOST overlay reacts to Escape.
 *   Without it, one keypress closes a whole nested stack (e.g. Terms of
 *   Service rendered inside the Settings dialog, or a confirm dialog over
 *   any modal).
 * - A Tab trap helper so keyboard focus cycles inside an open overlay
 *   instead of escaping to the page behind it.
 */

const openStack: HTMLElement[] = [];

/** Register an open overlay's root element; returns its unregister fn. */
export function pushOverlay(el: HTMLElement): () => void {
  openStack.push(el);
  return () => {
    const i = openStack.indexOf(el);
    if (i !== -1) openStack.splice(i, 1);
  };
}

/** True when `el` is the topmost open overlay (entitled to Escape). */
export function isTopmostOverlay(el: HTMLElement): boolean {
  return openStack[openStack.length - 1] === el;
}

/** True when ANY overlay (modal, confirm, manager dialog, RSVP reader) is
 *  open — global page shortcuts (e.g. the Record tab's Space-to-toggle)
 *  must stand down while one is. */
export function anyOverlayOpen(): boolean {
  return openStack.length > 0;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

function focusablesWithin(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => el.offsetParent !== null || el === document.activeElement,
  );
}

/**
 * Keep Tab/Shift+Tab cycling inside `root`. Only acts when the CURRENT
 * focus already lives in `root` — a nested overlay's own trap owns the
 * keystroke otherwise (the check makes stacked traps cooperate).
 */
export function trapTabWithin(root: HTMLElement, e: KeyboardEvent): void {
  if (e.key !== 'Tab') return;
  if (!root.contains(document.activeElement)) return;
  const focusables = focusablesWithin(root);
  if (focusables.length === 0) return;
  const first = focusables[0];
  const last = focusables[focusables.length - 1];
  const active = document.activeElement;
  if (e.shiftKey && (active === first || active === root)) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && active === last) {
    e.preventDefault();
    first.focus();
  }
}
