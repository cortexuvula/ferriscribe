<script lang="ts">
  import { pushOverlay, isTopmostOverlay, trapTabWithin } from '../stores/overlay';
  import { recordings } from '../stores/recordings.svelte';

  /// Restore-by-deletion-date (D6): ONE dialog — native date input, count
  /// preview from the DEDICATED count command (never derived from the
  /// paginated trash pages), restore, and an actual-vs-preview report.
  /// The interval is the EXACT local calendar day (Andre's ruling — not
  /// "on or after"); the store converts it to a half-open UTC window.
  interface Props {
    onClose: () => void;
    /// Announces to the Trash panel's single polite status region.
    onAnnounce: (message: string) => void;
  }

  const { onClose, onAnnounce }: Props = $props();

  type Phase = 'idle' | 'counting' | 'preview' | 'restoring';
  let dateValue = $state('');
  let phase = $state<Phase>('idle');
  let previewCount = $state<number | null>(null);
  let errorMsg = $state<string | null>(null);

  let root: HTMLElement | undefined = $state();
  let unregister: (() => void) | null = null;
  let restoreFocus: HTMLElement | null = null;

  $effect(() => {
    if (root) {
      unregister = pushOverlay(root);
      restoreFocus = (document.activeElement as HTMLElement) ?? null;
      root.querySelector<HTMLInputElement>('#restore-date-input')?.focus();
      return () => {
        unregister?.();
        unregister = null;
        restoreFocus?.focus();
        restoreFocus = null;
      };
    }
  });

  function parseLocalDate(value: string): Date | null {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
    if (!m) return null;
    const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
    // Reject calendar-impossible dates (e.g. Feb 30 rolls to Mar 2).
    if (
      d.getFullYear() !== Number(m[1]) ||
      d.getMonth() !== Number(m[2]) - 1 ||
      d.getDate() !== Number(m[3])
    ) {
      return null;
    }
    return d;
  }

  function isFuture(day: Date): boolean {
    const today = new Date();
    const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
    return startOfDay(day) > startOfDay(today);
  }

  const chosenDay = $derived(
    dateValue.trim() !== '' ? parseLocalDate(dateValue.trim()) : null,
  );
  const futureDate = $derived(chosenDay !== null && isFuture(chosenDay));

  async function handleDateChange() {
    errorMsg = null;
    if (chosenDay === null) {
      phase = 'idle';
      previewCount = null;
      return;
    }
    if (futureDate) {
      phase = 'idle';
      previewCount = null;
      return;
    }
    phase = 'counting';
    previewCount = null;
    try {
      const n = await recordings.countTrashedOnDate(chosenDay);
      previewCount = n;
      phase = 'preview';
    } catch (err) {
      console.error('Failed to count trashed recordings by date:', err);
      errorMsg = "Couldn't count recordings. Try again.";
      phase = 'idle';
    }
  }

  async function handleRestore() {
    if (phase !== 'preview' || chosenDay === null || previewCount === null || previewCount === 0) {
      return;
    }
    phase = 'restoring';
    errorMsg = null;
    try {
      const actual = await recordings.restoreTrashedOnDate(chosenDay);
      const noun = actual === 1 ? 'recording' : 'recordings';
      // Actual vs preview: a purge or concurrent restore may have changed
      // the candidate set between the two calls — report both honestly.
      const missing = previewCount - actual;
      onAnnounce(
        missing > 0
          ? `${actual} ${noun} restored to Active. ${missing} ${missing === 1 ? 'was' : 'were'} no longer in Trash.`
          : `${actual} ${noun} restored to Active.`,
      );
      onClose();
    } catch (err) {
      console.error('Failed to restore recordings by date:', err);
      // Keep the date + dialog; the preview stands for a retry.
      errorMsg = "Couldn't restore recordings. Try again.";
      phase = 'preview';
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!root) return;
    if (e.key === 'Escape') {
      if (!isTopmostOverlay(root)) return;
      onClose();
      return;
    }
    trapTabWithin(root, e);
  }

  function handleBackdrop(e: MouseEvent) {
    if (e.target === e.currentTarget && phase !== 'restoring') onClose();
  }

  const longDate = $derived(
    chosenDay === null
      ? ''
      : chosenDay.toLocaleDateString(undefined, {
          weekday: 'long',
          year: 'numeric',
          month: 'long',
          day: 'numeric',
        }),
  );
  const confirmNoun = $derived(
    previewCount === 1 ? 'recording' : 'recordings',
  );
</script>

<svelte:window onkeydown={handleKeydown} />

<div
  class="restore-backdrop"
  bind:this={root}
  onclick={handleBackdrop}
  role="presentation"
  tabindex="-1"
>
    <div
      class="restore-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="restore-date-title"
      aria-describedby="restore-date-helper"
    >
      <div class="restore-header">
        <span class="restore-title" id="restore-date-title">Restore by deletion date</span>
      </div>
      <div class="restore-body">
        <label class="restore-label" for="restore-date-input">Deleted on</label>
        <input
          id="restore-date-input"
          type="date"
          bind:value={dateValue}
          onchange={handleDateChange}
        />
        {#if futureDate}
          <p class="restore-error" role="alert">
            Choose a date that has already happened.
          </p>
        {/if}
        <p class="restore-helper" id="restore-date-helper">
          Uses the date recordings were moved to Trash, not the date they were recorded.
          Searches all of Trash, regardless of the current search. Times use this
          machine's timezone.
        </p>

        {#if phase === 'counting'}
          <p class="restore-preview">Counting…</p>
        {:else if phase === 'preview' && previewCount !== null}
          {#if previewCount === 0}
            <p class="restore-preview">No recordings currently in Trash match this date.</p>
          {:else}
            <p class="restore-preview">
              Restore {previewCount} {confirmNoun} moved to Trash on {longDate}?
            </p>
            <p class="restore-sub">
              They will return to Active with their saved audio and documents.
            </p>
          {/if}
        {:else if phase === 'restoring'}
          <p class="restore-preview">Restoring…</p>
        {/if}

        {#if errorMsg !== null}
          <p class="restore-error" role="alert">{errorMsg}</p>
        {/if}
      </div>
      <div class="restore-actions">
        <button
          class="btn-cancel"
          onclick={() => phase !== 'restoring' && onClose()}
          disabled={phase === 'restoring'}
        >
          Cancel
        </button>
        <button
          class="btn-confirm"
          onclick={handleRestore}
          disabled={phase !== 'preview' || previewCount === null || previewCount === 0}
        >
          {previewCount !== null && previewCount > 0
            ? `Restore ${previewCount} ${confirmNoun}`
            : 'Restore'}
        </button>
      </div>
    </div>
  </div>

<style>
  .restore-backdrop {
    position: fixed;
    inset: 0;
    background-color: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 2000;
  }

  .restore-backdrop:focus {
    outline: none;
  }

  .restore-dialog {
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
    width: 100%;
    max-width: 440px;
    margin: 16px;
    overflow: hidden;
  }

  .restore-header {
    padding: 16px 20px 0;
  }

  .restore-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .restore-body {
    padding: 12px 20px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .restore-label {
    font-size: 13px;
    color: var(--text-secondary);
  }

  input[type='date'] {
    padding: 8px 10px;
    font-size: 13px;
    border-radius: var(--radius-sm);
    background-color: var(--bg-input);
    color: var(--text-primary);
  }

  .restore-helper {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-muted);
    margin: 0;
  }

  .restore-preview {
    font-size: 13px;
    color: var(--text-primary);
    margin: 0;
  }

  .restore-sub {
    font-size: 12px;
    color: var(--text-muted);
    margin: 0;
  }

  .restore-error {
    font-size: 13px;
    color: var(--danger, #ef4444);
    margin: 0;
  }

  .restore-actions {
    display: flex;
    border-top: 1px solid var(--border);
  }

  .restore-actions button {
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

  .btn-cancel:hover:not(:disabled) {
    background-color: var(--bg-hover);
  }

  .btn-confirm {
    color: var(--accent);
  }

  .btn-confirm:hover:not(:disabled) {
    background-color: var(--bg-hover);
  }

  .btn-confirm:disabled,
  .btn-cancel:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
