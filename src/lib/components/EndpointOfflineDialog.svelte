<script lang="ts">
  import { endpointOfflineStore } from '../stores/endpointOffline.svelte.ts';
  import type {
    EndpointOfflinePayload,
    OfflineReason,
    ServiceKind,
  } from '../api/invokeWithOfflineHandling';

  type Props = {
    onopenSettings: (service: ServiceKind) => void;
  };
  let { onopenSettings }: Props = $props();

  let dialogEl: HTMLDivElement | undefined = $state(undefined);
  let retryBtn: HTMLButtonElement | undefined = $state(undefined);

  // When the dialog opens, focus the Retry button on the next microtask.
  $effect(() => {
    if (endpointOfflineStore.state && retryBtn) {
      setTimeout(() => retryBtn?.focus(), 0);
    }
  });

  function reasonSentence(payload: EndpointOfflinePayload): string {
    const { reason, provider_name, endpoint } = payload;
    switch (reason as OfflineReason) {
      case 'ConnectionRefused':
        return `The ${provider_name} server at ${endpoint} didn't respond.`;
      case 'Timeout':
        return `The ${provider_name} server at ${endpoint} took too long to respond.`;
      case 'DnsFailure':
        return `The address "${endpoint}" couldn't be found on the network.`;
      case 'TlsFailure':
        return `Couldn't establish a secure connection to ${provider_name} at ${endpoint}.`;
    }
  }

  function onRetry() {
    endpointOfflineStore._resolve('retry');
  }
  function onCancel() {
    endpointOfflineStore._resolve('cancel');
  }
  function onOpenSettingsClick() {
    if (endpointOfflineStore.state) {
      onopenSettings(endpointOfflineStore.state.payload.service);
    }
    endpointOfflineStore._resolve('opened_settings');
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onCancel();
      return;
    }
    if (e.key === 'Tab' && dialogEl) {
      const focusable = dialogEl.querySelectorAll<HTMLElement>(
        'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
      );
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  }

  function onBackdrop(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      onCancel();
    }
  }
</script>

{#if endpointOfflineStore.state}
  <div
    class="modal-backdrop"
    role="presentation"
    onclick={onBackdrop}
  >
    <div
      bind:this={dialogEl}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="endpoint-offline-title"
      aria-describedby="endpoint-offline-body"
      class="modal"
      tabindex="-1"
      onkeydown={onKeydown}
    >
      <h2 id="endpoint-offline-title">Office server isn't responding</h2>
      <div id="endpoint-offline-body">
        <p>{reasonSentence(endpointOfflineStore.state.payload)}</p>
        <p>Common causes:</p>
        <ul>
          <li>The server app isn't running on your Mac</li>
          <li>Your Mac is asleep or has lost network</li>
          <li>The address in Settings has changed</li>
        </ul>
        <p><strong>Your recording is saved.</strong> You can process it once the server is back online.</p>
      </div>
      <div class="modal-actions">
        <button type="button" onclick={onOpenSettingsClick}>
          Open Settings
        </button>
        <button type="button" onclick={onCancel}>
          Cancel
        </button>
        <button type="button" class="primary" bind:this={retryBtn} onclick={onRetry}>
          Retry
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }
  .modal {
    background: var(--bg, white);
    border-radius: 8px;
    padding: 1.5rem;
    min-width: 32rem;
    max-width: 700px;
    display: flex;
    flex-direction: column;
    gap: 1rem;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.2);
  }
  h2 {
    margin: 0;
    font-size: 1.25rem;
  }
  ul {
    margin: 0.5rem 0;
    padding-left: 1.5rem;
  }
  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    padding-top: 0.5rem;
    border-top: 1px solid var(--border, #ddd);
  }
  button {
    padding: 0.4rem 1rem;
    border: 1px solid var(--border, #ccc);
    border-radius: 4px;
    cursor: pointer;
    background: var(--bg, white);
    color: inherit;
  }
  button.primary {
    background: #0066cc;
    color: white;
    border: none;
    cursor: pointer;
  }
  button.primary:disabled {
    background: #ccc;
    cursor: not-allowed;
  }
</style>
