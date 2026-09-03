<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { pushOverlay, isTopmostOverlay, trapTabWithin } from '../stores/overlay';

  interface Props {
    open: boolean;
    title: string;
    onClose: () => void;
    children?: import('svelte').Snippet;
  }

  const { open, title, onClose, children }: Props = $props();

  let root: HTMLElement | undefined = $state();
  let unregister: (() => void) | null = null;
  let restoreFocus: HTMLElement | null = null;

  // Overlay-stack membership + focus management: with nested Modals (Terms
  // of Service inside Settings) and confirm dialogs stacked on top, only
  // the TOPMOST overlay may react to Escape — otherwise one keypress
  // closes the whole stack. Focus starts in the dialog and is restored to
  // the invoker on close.
  $effect(() => {
    if (open && root) {
      unregister = pushOverlay(root);
      restoreFocus = (document.activeElement as HTMLElement) ?? null;
      root.focus();
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
      onClose();
      return;
    }
    trapTabWithin(root, e);
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      onClose();
    }
  }

  onMount(() => {
    window.addEventListener('keydown', handleKeydown);
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handleKeydown);
  });
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="modal-backdrop" bind:this={root} onclick={handleBackdropClick} role="dialog" aria-modal="true" aria-label={title} tabindex="-1">
    <div class="modal-container">
      <div class="modal-header">
        <span class="modal-title">{title}</span>
        <button class="modal-close" onclick={onClose} aria-label="Close dialog">×</button>
      </div>
      <div class="modal-body">
        {@render children?.()}
      </div>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background-color: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal-backdrop:focus {
    outline: none;
  }

  .modal-container {
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
    width: 100%;
    max-width: 960px;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    margin: 16px;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .modal-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .modal-close {
    font-size: 20px;
    line-height: 1;
    color: var(--text-muted);
    padding: 4px 8px;
    border-radius: var(--radius-sm);
    transition: color 0.15s ease, background-color 0.15s ease;
  }

  .modal-close:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .modal-body {
    overflow-y: auto;
    flex: 1;
  }
</style>
