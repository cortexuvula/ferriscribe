<script lang="ts">
  /**
   * The single, app-wide host for the promise-based confirmDialog() service
   * (stores/confirm.svelte.ts), rendered once in App.svelte next to
   * ToastContainer. Draws the shared styled ConfirmDialog; per-instance
   * ConfirmDialog usages (Record/Recordings/Chat tabs) keep working
   * independently and cooperate through the shared overlay stack.
   */
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { confirmStore } from '../stores/confirm.svelte';

  const opts = $derived(confirmStore.state.options);
</script>

{#if opts}
  <ConfirmDialog
    open={confirmStore.state.open}
    title={opts.title}
    message={opts.message}
    confirmLabel={opts.confirmLabel ?? (opts.confirmOnly ? 'Close' : 'Confirm')}
    cancelLabel={opts.cancelLabel ?? 'Cancel'}
    danger={opts.danger === true}
    confirmOnly={opts.confirmOnly === true}
    tallBody={opts.confirmOnly === true}
    onConfirm={() => confirmStore.settle(true)}
    onCancel={() => confirmStore.settle(false)}
  />
{/if}
