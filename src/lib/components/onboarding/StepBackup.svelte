<script lang="ts">
  interface Props {
    onDone: () => void;
    onSetUp: () => void;
    finishing?: boolean;
    finishError?: string | null;
  }
  const { onDone, onSetUp, finishing = false, finishError = null }: Props = $props();
</script>

<h2>One last thing: protect your data</h2>
<p class="hint">
  Your recordings and notes currently exist only on this machine. FerriScribe can back
  them up — encrypted — to another computer on your network, with a printed recovery
  sheet kept in a safe as the only key needed to restore everything after a disk
  failure.
</p>
<p class="hint">
  Setting it up takes about ten minutes: generate a recovery key, print the sheet, and
  pick a daily backup time. You can also do it any time later from
  <strong>Settings → Backup</strong>.
</p>

{#if finishError}
  <p class="error" role="alert">{finishError}</p>
{/if}

<div class="actions">
  <button class="btn-secondary" onclick={onDone} disabled={finishing}>
    Set up later
  </button>
  <button class="btn-primary" onclick={onSetUp} disabled={finishing}>
    {finishing ? 'Finishing…' : 'Set up backup now'}
  </button>
</div>

<style>
  .hint {
    color: var(--text-secondary);
    font-size: 0.95rem;
    line-height: 1.6;
    margin: 0 0 12px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 20px;
  }
  .btn-secondary {
    padding: 10px 18px;
    background: transparent;
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
    font-size: 14px;
  }
  .btn-primary {
    padding: 10px 18px;
    background: var(--accent);
    color: white;
    border: none;
    border-radius: var(--radius-md);
    cursor: pointer;
    font-size: 14px;
    font-weight: 500;
  }
  .btn-primary:disabled,
  .btn-secondary:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .error {
    color: var(--danger, #ef4444);
    font-size: 0.85rem;
  }
</style>
