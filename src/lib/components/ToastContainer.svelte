<script lang="ts">
  import { toasts, type Toast } from '../stores/toasts.svelte';

  interface Props {
    onNavigate?: (tab: string, recordingId: string) => void;
  }
  const { onNavigate }: Props = $props();

  function handleView(toast: Toast) {
    if (toast.recordingId && onNavigate) {
      onNavigate('soap', toast.recordingId);
    }
    toasts.dismiss(toast.id);
  }
</script>

{#if toasts.list.length > 0}
  <div class="toast-container">
    {#each toasts.list as toast (toast.id)}
      <div class="toast" class:toast-success={toast.type === 'success'} class:toast-error={toast.type === 'error'}>
        <span class="toast-message">{toast.message}</span>
        <div class="toast-actions">
          {#if toast.type === 'success' && toast.recordingId}
            <button class="toast-btn toast-btn-view" onclick={() => handleView(toast)}>View</button>
          {/if}
          <button class="toast-btn toast-btn-dismiss" onclick={() => toasts.dismiss(toast.id)}>Dismiss</button>
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .toast-container {
    position: fixed;
    top: 16px;
    right: 16px;
    z-index: 9999;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 400px;
  }

  .toast {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    border-radius: var(--radius-md, 8px);
    font-size: 13px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
    animation: slideIn 0.2s ease-out;
  }

  @keyframes slideIn {
    from { transform: translateX(100%); opacity: 0; }
    to { transform: translateX(0); opacity: 1; }
  }

  .toast-success {
    background-color: var(--bg-secondary, #1f2937);
    border: 1px solid var(--success, #22c55e);
    color: var(--text-primary, #f9fafb);
  }

  .toast-error {
    background-color: var(--bg-secondary, #1f2937);
    border: 1px solid var(--danger, #ef4444);
    color: var(--text-primary, #f9fafb);
  }

  .toast-message {
    flex: 1;
  }

  .toast-actions {
    display: flex;
    gap: 6px;
    flex-shrink: 0;
  }

  .toast-btn {
    padding: 4px 10px;
    border-radius: var(--radius-sm, 4px);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .toast-btn-view {
    background-color: var(--accent, #3b82f6);
    color: white;
  }

  .toast-btn-view:hover {
    background-color: var(--accent-hover, #2563eb);
  }

  .toast-btn-dismiss {
    background-color: transparent;
    color: var(--text-muted, #9ca3af);
    border: 1px solid var(--border, #374151);
  }

  .toast-btn-dismiss:hover {
    background-color: var(--bg-hover, #374151);
    color: var(--text-primary, #f9fafb);
  }
</style>
