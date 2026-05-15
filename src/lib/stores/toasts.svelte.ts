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

  add(toast: Omit<Toast, 'id'>) {
    const id = `toast-${++this.counter}`;
    this.list = [...this.list, { ...toast, id }];
    if (toast.autoDismiss) {
      setTimeout(() => this.dismiss(id), 8000);
    }
    return id;
  }

  dismiss(id: string) {
    this.list = this.list.filter((t) => t.id !== id);
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
