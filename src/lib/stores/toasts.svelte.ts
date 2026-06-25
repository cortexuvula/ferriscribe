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
}

class ToastStore {
  list = $state<Toast[]>([]);
  private counter = 0;
  /** Pending auto-dismiss timers keyed by toast id, so dismiss() can cancel
   *  the timer when the user manually closes early, and a destroy() can clear
   *  all timers on app teardown. */
  private timers = new Map<string, ReturnType<typeof setTimeout>>();

  add(toast: Omit<Toast, 'id'>) {
    const id = `toast-${++this.counter}`;
    this.list = [...this.list, { ...toast, id }];
    if (toast.autoDismiss) {
      const handle = setTimeout(() => this.dismiss(id), 8000);
      this.timers.set(id, handle);
    }
    return id;
  }

  dismiss(id: string) {
    this.list = this.list.filter((t) => t.id !== id);
    const handle = this.timers.get(id);
    if (handle) {
      clearTimeout(handle);
      this.timers.delete(id);
    }
  }

  /** Clear all toasts and cancel pending timers. Called on app teardown. */
  destroy() {
    for (const handle of this.timers.values()) clearTimeout(handle);
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
