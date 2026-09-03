/**
 * Promise-based replacement for the native window.confirm()/alert() in
 * settings flows — the styled ConfirmDialog host (rendered once in
 * App.svelte) draws from this store, mirroring the toasts-store pattern:
 *
 *   const ok = await confirmDialog({ title, message, danger: true });
 *
 * A dialog superseded by a newer request resolves `false` so no caller
 * hangs. `confirmOnly` renders a single Close button for informational
 * use (replacing window.alert).
 */

export interface ConfirmOptions {
  title: string;
  /** Body text. Newlines are preserved (white-space: pre-wrap). */
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Destructive styling on the confirm button (deletes, stops, resets). */
  danger?: boolean;
  /** Informational mode: single Close button, no cancel. */
  confirmOnly?: boolean;
}

interface ConfirmState {
  open: boolean;
  options: ConfirmOptions | null;
  resolve: ((ok: boolean) => void) | null;
}

class ConfirmStore {
  state = $state<ConfirmState>({ open: false, options: null, resolve: null });

  confirm(options: ConfirmOptions): Promise<boolean> {
    // Dismiss (as cancelled) whatever dialog is already open.
    this.state.resolve?.(false);
    return new Promise<boolean>((resolve) => {
      this.state = { open: true, options, resolve };
    });
  }

  /** Resolve the pending dialog and close. */
  settle(ok: boolean): void {
    this.state.resolve?.(ok);
    this.state = { open: false, options: null, resolve: null };
  }
}

export const confirmStore = new ConfirmStore();

export function confirmDialog(options: ConfirmOptions): Promise<boolean> {
  return confirmStore.confirm(options);
}
