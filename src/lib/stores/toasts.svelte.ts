export interface Toast {
  id: string;
  message: string;
  type: 'success' | 'error';
  /** Recording ID for "View" button navigation. */
  recordingId?: string;
  /** Display name shown in the toast. */
  displayName?: string;
  /** Whether to auto-dismiss (errors persist until manually dismissed). */
  autoDismiss: boolean;
  /** Optional action button label (e.g. "Undo"). */
  actionLabel?: string;
  /** Callback fired when the action button is clicked. */
  onAction?: () => void;
}

class ToastStore {
  list = $state<Toast[]>([]);
  private counter = 0;
  /** Armed auto-dismiss timers keyed by toast id. The entry tracks the
   *  remaining budget so actionable toasts can PAUSE dismissal while
   *  focused/hovered (an Undo the user is reaching for must not vanish)
   *  and resume without resetting the clock. */
  private timers = new Map<string, { handle: ReturnType<typeof setTimeout>; remaining: number; startedAt: number }>();

  add(toast: Omit<Toast, 'id'>) {
    const id = `toast-${++this.counter}`;
    this.list = [...this.list, { ...toast, id }];
    if (toast.autoDismiss) {
      this.arm(id, 8000);
    }
    return id;
  }

  private arm(id: string, ms: number) {
    const handle = setTimeout(() => this.dismiss(id), ms);
    this.timers.set(id, { handle, remaining: ms, startedAt: Date.now() });
  }

  dismiss(id: string) {
    this.list = this.list.filter((t) => t.id !== id);
    const entry = this.timers.get(id);
    if (entry) {
      clearTimeout(entry.handle);
      this.timers.delete(id);
    }
  }

  /** Pause an actionable toast's auto-dismiss (focus/hover). No-op for
   *  toasts without a timer (persistent errors). */
  pause(id: string) {
    const entry = this.timers.get(id);
    if (!entry) return;
    clearTimeout(entry.handle);
    entry.remaining = Math.max(0, entry.remaining - (Date.now() - entry.startedAt));
  }

  /** Resume a paused toast's auto-dismiss with its remaining budget. */
  resume(id: string) {
    const entry = this.timers.get(id);
    if (!entry) return;
    this.arm(id, entry.remaining);
  }

  /** Clear all toasts and cancel pending timers. Called on app teardown. */
  destroy() {
    for (const entry of this.timers.values()) clearTimeout(entry.handle);
    this.timers.clear();
    this.list = [];
  }

  /** Convenience: show an error toast that persists until dismissed. */
  error(message: string) {
    return this.add({ message, type: 'error', autoDismiss: false });
  }

  /** Convenience: show a success toast that auto-dismisses. */
  success(message: string) {
    return this.add({ message, type: 'success', autoDismiss: true });
  }
}

export const toasts = new ToastStore();
