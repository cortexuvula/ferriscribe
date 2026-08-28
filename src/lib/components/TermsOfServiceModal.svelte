<script lang="ts">
  /**
   * Read-only Terms of Service viewer (Settings → About). The acceptance
   * gate for first use lives in TermsGate.svelte; this modal is for
   * re-reading the Terms at any time, with the recorded acceptance date.
   */
  import Modal from './Modal.svelte';
  import { settings } from '../stores/settings.svelte';
  import { TERMS_OF_SERVICE_TEXT } from '../terms';

  let { open, onClose }: { open: boolean; onClose: () => void } = $props();

  const acceptedAt = $derived(
    settings.state.tos_accepted_at
      ? new Date(settings.state.tos_accepted_at).toLocaleString()
      : null,
  );
</script>

<Modal {open} title="Terms of Service" {onClose}>
  <div class="tos-view">
    {#if acceptedAt}
      <p class="accepted-line">You accepted these Terms on {acceptedAt}.</p>
    {:else}
      <p class="accepted-line pending">Not yet accepted — you will be asked to accept on next launch.</p>
    {/if}
    <div class="tos-scroll">
      <pre>{TERMS_OF_SERVICE_TEXT}</pre>
    </div>
  </div>
</Modal>

<style>
  .tos-view {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-height: 0;
  }
  .accepted-line {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary);
  }
  .accepted-line.pending { color: var(--text-muted); }
  .tos-scroll {
    max-height: 60vh;
    overflow-y: auto;
    border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px);
    background-color: var(--bg-primary, #111);
    padding: 14px;
  }
  .tos-scroll pre {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    line-height: 1.55;
    color: var(--text-primary);
  }
</style>
