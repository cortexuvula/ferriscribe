<script lang="ts">
  interface Props {
    open: boolean;
    title?: string;
    message: string;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  }

  const {
    open,
    title = 'Confirm',
    message,
    confirmLabel = 'Delete',
    cancelLabel = 'Cancel',
    danger = true,
    onConfirm,
    onCancel,
  }: Props = $props();

  function handleKeydown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === 'Escape') onCancel();
  }

  function handleBackdrop(e: MouseEvent) {
    if (e.target === e.currentTarget) onCancel();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="confirm-backdrop" onclick={handleBackdrop} role="presentation">
    <div class="confirm-dialog" role="alertdialog" aria-modal="true" aria-label={title}>
      <div class="confirm-header">
        <span class="confirm-icon" class:danger>{danger ? '⚠' : '?'}</span>
        <span class="confirm-title">{title}</span>
      </div>
      <div class="confirm-body">
        <p>{message}</p>
      </div>
      <div class="confirm-actions">
        <button class="btn-cancel" onclick={onCancel}>{cancelLabel}</button>
        <button class="btn-confirm" class:danger onclick={onConfirm}>{confirmLabel}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .confirm-backdrop {
    position: fixed;
    inset: 0;
    background-color: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 2000;
    animation: fadeIn 0.15s ease;
  }

  @keyframes fadeIn {
    from { opacity: 0; }
    to { opacity: 1; }
  }

  .confirm-dialog {
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
    width: 100%;
    max-width: 400px;
    margin: 16px;
    overflow: hidden;
    animation: slideUp 0.15s ease;
  }

  @keyframes slideUp {
    from { transform: translateY(10px); opacity: 0; }
    to { transform: translateY(0); opacity: 1; }
  }

  .confirm-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 16px 20px 0;
  }

  .confirm-icon {
    font-size: 20px;
    width: 36px;
    height: 36px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    background-color: var(--bg-tertiary);
    flex-shrink: 0;
  }

  .confirm-icon.danger {
    background-color: rgba(239, 68, 68, 0.1);
    color: var(--danger, #ef4444);
  }

  .confirm-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .confirm-body {
    padding: 12px 20px 20px;
  }

  .confirm-body p {
    font-size: 13px;
    line-height: 1.6;
    color: var(--text-muted);
    margin: 0;
  }

  .confirm-actions {
    display: flex;
    border-top: 1px solid var(--border);
  }

  .confirm-actions button {
    flex: 1;
    padding: 12px 16px;
    font-size: 13px;
    font-weight: 500;
    transition: background-color 0.15s ease;
  }

  .btn-cancel {
    color: var(--text-secondary);
    border-right: 1px solid var(--border);
  }

  .btn-cancel:hover {
    background-color: var(--bg-hover);
  }

  .btn-confirm {
    color: var(--accent);
  }

  .btn-confirm.danger {
    color: var(--danger, #ef4444);
  }

  .btn-confirm:hover {
    background-color: var(--bg-hover);
  }
</style>
