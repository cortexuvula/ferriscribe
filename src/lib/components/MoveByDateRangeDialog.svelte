<script lang="ts">
  import { pushOverlay, isTopmostOverlay, trapTabWithin } from '../stores/overlay';
  import { recordings } from '../stores/recordings.svelte';
  import { settings } from '../stores/settings.svelte';

  /// Move-by-date-range (Active list): ONE dialog — two native date
  /// inputs, count preview from the DEDICATED count command (never
  /// derived from the paginated/filtered Active list), move, and an
  /// actual-vs-preview report. Both ends of the range are inclusive; the
  /// store converts them to a half-open UTC window over `created_at`.
  interface Props {
    onClose: () => void;
    /// Reports a successful move (result + the preview count it executed
    /// against) so the parent owns the success/Undo toast wiring.
    onMoved: (result: { count: number; ids: string[] }, preview: number) => void;
    /// Reports a failed move so the parent can toast the sanitized error;
    /// the dialog keeps itself open with its own retry copy.
    onMoveFailed?: (err: unknown) => void;
  }

  const { onClose, onMoved, onMoveFailed }: Props = $props();

  type Phase = 'idle' | 'counting' | 'preview' | 'deleting';
  let startDateValue = $state('');
  let endDateValue = $state('');
  let phase = $state<Phase>('idle');
  let previewCount = $state<number | null>(null);
  let errorMsg = $state<string | null>(null);
  /// Monotonic request id guarding the count preview (same discipline as
  /// RestoreByDateDialog / the store's list/select tokens): rapid date
  /// changes race their counts, and a slow count for range A landing
  /// after range B's would display A's N under B's heading.
  let previewRequestId = 0;

  const retentionDays = $derived(settings.trashRetentionDays);

  let root: HTMLElement | undefined = $state();
  let unregister: (() => void) | null = null;
  let restoreFocus: HTMLElement | null = null;

  $effect(() => {
    if (root) {
      unregister = pushOverlay(root);
      restoreFocus = (document.activeElement as HTMLElement) ?? null;
      root.querySelector<HTMLInputElement>('#move-range-start-input')?.focus();
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

  const startDay = $derived(
    startDateValue.trim() !== '' ? parseLocalDate(startDateValue.trim()) : null,
  );
  const endDay = $derived(
    endDateValue.trim() !== '' ? parseLocalDate(endDateValue.trim()) : null,
  );
  const futureDate = $derived(
    (startDay !== null && isFuture(startDay)) || (endDay !== null && isFuture(endDay)),
  );
  /// A reversed range is only knowable once both ends parse — one end
  /// alone never triggers this alert. Same-day ranges are valid.
  const reversedRange = $derived(
    startDay !== null && endDay !== null && endDay.getTime() < startDay.getTime(),
  );
  const rangeValid = $derived(
    startDay !== null && endDay !== null && !futureDate && !reversedRange,
  );

  /// Re-query the count preview for the chosen range. Shared by the
  /// date-change path and the move-failure retry path: a preview captured
  /// before a failed move may be stale (a concurrent deletion may have
  /// changed the candidate set), so a retry must re-count before it can
  /// execute again — never move against the pre-failure N.
  async function refreshPreview(): Promise<void> {
    if (startDay === null || endDay === null) return;
    const token = ++previewRequestId;
    phase = 'counting';
    previewCount = null;
    try {
      const count = await recordings.countActiveInRange(startDay, endDay);
      if (token !== previewRequestId) return;
      previewCount = count;
      phase = 'preview';
    } catch (err) {
      if (token !== previewRequestId) return;
      console.error('Failed to count recordings by date range:', err);
      errorMsg = "Couldn't count recordings. Try again.";
      phase = 'idle';
    }
  }

  async function handleDateChange() {
    errorMsg = null;
    if (!rangeValid) {
      // Invalidate any in-flight count: its result belongs to a range the
      // user has already abandoned.
      previewRequestId++;
      phase = 'idle';
      previewCount = null;
      return;
    }
    await refreshPreview();
  }

  async function handleMove() {
    if (
      phase !== 'preview' ||
      startDay === null ||
      endDay === null ||
      previewCount === null ||
      previewCount === 0
    ) {
      return;
    }
    phase = 'deleting';
    errorMsg = null;
    try {
      const result = await recordings.moveActiveRangeToTrash(startDay, endDay);
      onMoved(result, previewCount);
      onClose();
    } catch (err) {
      console.error('Failed to move recordings by date range:', err);
      // Keep the dates + dialog — but the stored preview may be stale (the
      // failure may have happened before or after a concurrent change), so
      // it is invalidated and re-queried here: the next Move click
      // executes against a fresh count, never the pre-failure N.
      errorMsg = "Couldn't move recordings. Try again.";
      onMoveFailed?.(err);
      await refreshPreview();
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!root) return;
    if (e.key === 'Escape') {
      if (phase === 'deleting' || !isTopmostOverlay(root)) return;
      onClose();
      return;
    }
    trapTabWithin(root, e);
  }

  function handleBackdrop(e: MouseEvent) {
    if (e.target === e.currentTarget && phase !== 'deleting') onClose();
  }

  const longDate = (day: Date) =>
    day.toLocaleDateString(undefined, {
      weekday: 'long',
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    });
  const longStartDate = $derived(startDay === null ? '' : longDate(startDay));
  const longEndDate = $derived(endDay === null ? '' : longDate(endDay));
  const confirmNoun = $derived(
    previewCount === 1 ? 'recording' : 'recordings',
  );
</script>

<svelte:window onkeydown={handleKeydown} />

<div
  class="move-backdrop"
  bind:this={root}
  onclick={handleBackdrop}
  role="presentation"
  tabindex="-1"
>
    <div
      class="move-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="move-range-title"
      aria-describedby="move-range-helper"
    >
      <div class="move-header">
        <span class="move-title" id="move-range-title">Move recordings by date to Trash</span>
      </div>
      <div class="move-body">
        <div class="move-fields">
          <div class="move-field">
            <label class="move-label" for="move-range-start-input">Recorded from</label>
            <input
              id="move-range-start-input"
              type="date"
              bind:value={startDateValue}
              onchange={handleDateChange}
              disabled={phase === 'deleting'}
            />
          </div>
          <div class="move-field">
            <label class="move-label" for="move-range-end-input">Recorded to</label>
            <input
              id="move-range-end-input"
              type="date"
              bind:value={endDateValue}
              onchange={handleDateChange}
              disabled={phase === 'deleting'}
            />
          </div>
        </div>
        {#if futureDate}
          <p class="move-error" role="alert">
            Choose a date that has already happened.
          </p>
        {/if}
        {#if reversedRange}
          <p class="move-error" role="alert">
            The end date can't be before the start date.
          </p>
        {/if}
        <p class="move-helper" id="move-range-helper">
          Uses the date each recording was made, not the date it was moved to Trash.
          Includes all active recordings in the range, regardless of the current search
          and selected date. Times use this machine's timezone.
        </p>

        {#if phase === 'counting'}
          <p class="move-preview">Counting…</p>
        {:else if phase === 'preview' && previewCount !== null}
          {#if previewCount === 0}
            <p class="move-preview">No active recordings match this date range.</p>
          {:else}
            <p class="move-preview">
              Move {previewCount} {confirmNoun} recorded {longStartDate} through {longEndDate} to Trash?
            </p>
            <p class="move-sub">
              They’ll stay in Trash and can be restored for {retentionDays} days.
              After that, cleanup permanently deletes their audio, transcripts, SOAP
              notes and generated documents.
            </p>
          {/if}
        {:else if phase === 'deleting'}
          <p class="move-preview">Moving…</p>
        {/if}

        {#if errorMsg !== null}
          <p class="move-error" role="alert">{errorMsg}</p>
        {/if}
      </div>
      <div class="move-actions">
        <button
          class="btn-cancel"
          onclick={() => phase !== 'deleting' && onClose()}
          disabled={phase === 'deleting'}
        >
          Cancel
        </button>
        <button
          class="btn-confirm"
          onclick={handleMove}
          disabled={phase !== 'preview' || previewCount === null || previewCount === 0}
        >
          {phase === 'deleting'
            ? 'Moving…'
            : previewCount !== null && previewCount > 0
              ? `Move ${previewCount} ${confirmNoun} to Trash`
              : 'Move to Trash'}
        </button>
      </div>
    </div>
  </div>

<style>
  .move-backdrop {
    position: fixed;
    inset: 0;
    background-color: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 2000;
  }

  .move-backdrop:focus {
    outline: none;
  }

  .move-dialog {
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
    width: 100%;
    max-width: 440px;
    margin: 16px;
    overflow: hidden;
  }

  .move-header {
    padding: 16px 20px 0;
  }

  .move-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .move-body {
    padding: 12px 20px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .move-fields {
    display: flex;
    gap: 12px;
  }

  .move-field {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .move-label {
    font-size: 13px;
    color: var(--text-secondary);
  }

  input[type='date'] {
    padding: 8px 10px;
    font-size: 13px;
    border-radius: var(--radius-sm);
    background-color: var(--bg-input);
    color: var(--text-primary);
    width: 100%;
    box-sizing: border-box;
  }

  .move-helper {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-muted);
    margin: 0;
  }

  .move-preview {
    font-size: 13px;
    color: var(--text-primary);
    margin: 0;
  }

  .move-sub {
    font-size: 12px;
    color: var(--text-muted);
    margin: 0;
  }

  .move-error {
    font-size: 13px;
    color: var(--danger, #ef4444);
    margin: 0;
  }

  .move-actions {
    display: flex;
    border-top: 1px solid var(--border);
  }

  .move-actions button {
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
