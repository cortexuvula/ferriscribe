<script lang="ts">
  import { pushOverlay, isTopmostOverlay, trapTabWithin } from '../stores/overlay';

  interface Props {
    open: boolean;
    title?: string;
    message: string;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
    /** Informational mode: hides the cancel button (replaces alert()). */
    confirmOnly?: boolean;
    /** Scrollable monospace body for long text (e.g. stored prompts). */
    tallBody?: boolean;
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
    confirmOnly = false,
    tallBody = false,
    onConfirm,
    onCancel,
  }: Props = $props();

  let root: HTMLElement | undefined = $state();
  let unregister: (() => void) | null = null;
  let restoreFocus: HTMLElement | null = null;

  // Overlay-stack membership + focus management: only the topmost overlay
  // in the app may act on Escape, focus starts on the safe action, and the
  // invoker's focus is restored on close.
  $effect(() => {
    if (open && root) {
      unregister = pushOverlay(root);
      restoreFocus = (document.activeElement as HTMLElement) ?? null;
      const cancel = root.querySelector<HTMLButtonElement>('.btn-cancel');
      (cancel ?? root.querySelector<HTMLButtonElement>('.btn-confirm'))?.focus();
      return () => {
        unregister?.();
        unregister = null;
        restoreFocus?.focus();
        restoreFocus = null;
      };
    }
  });

  function handleKeydown(e: KeyboardEvent) {
    if (!open || !root) return;
    if (e.key === 'Escape') {
      if (!isTopmostOverlay(root)) return;
      onCancel();
      return;
    }
    trapTabWithin(root, e);
  }

  function handleBackdrop(e: MouseEvent) {
    if (e.target === e.currentTarget) onCancel();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <div class="confirm-backdrop" bind:this={root} onclick={handleBackdrop} role="presentation" tabindex="-1">
    <div class="confirm-dialog" role="alertdialog" aria-modal="true" aria-label={title}>
      <div class="confirm-header">
        <span class="confirm-icon" class:danger>{danger ? '⚠' : '?'}</span>
        <span class="confirm-title">{title}</span>
      </div>
      <div class="confirm-body" class:tall={tallBody}>
        <p>{message}</p>
      </div>
      <div class="confirm-actions">
        {#if !confirmOnly}
          <button class="btn-cancel" onclick={onCancel}>{cancelLabel}</button>
        {/if}
        <button
          class="btn-confirm"
          class:danger={!confirmOnly && danger}
          class:solo={confirmOnly}
          onclick={onConfirm}
        >
          {confirmOnly ? (confirmLabel === 'Delete' ? 'Close' : confirmLabel) : confirmLabel}
        </button>
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

  .confirm-backdrop:focus {
    outline: none;
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
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .confirm-body.tall {
    max-height: 50vh;
    overflow-y: auto;
  }

  .confirm-body.tall p {
    font-family: var(--font-mono, monospace);
    font-size: 12px;
    color: var(--text-primary);
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 10px 12px;
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
