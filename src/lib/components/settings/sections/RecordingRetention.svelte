<script lang="ts">
  import { tick } from 'svelte';
  import Callout from '../Callout.svelte';
  import { settings } from '../../../stores/settings.svelte';
  import { countSoftDeletedOlderThan } from '../../../api/recordings';
  import { confirmDialog } from '../../../stores/confirm.svelte';
  import { toasts } from '../../../stores/toasts.svelte';

  let savingTrashWindow = $state(false);
  let pendingTrashDays = $state<number | null>(null);
  const trashDays = $derived(pendingTrashDays ?? settings.state.trash_retention_days ?? 30);

  async function handleTrashRetentionChange(e: Event) {
    const select = e.currentTarget as HTMLSelectElement;
    const previous = settings.state.trash_retention_days ?? 30;
    const days = Number(select.value);
    // Keep the invoker focusable while previewing/confirming. Ignore repeated
    // changes without letting the native select display an unsaved second value.
    if (savingTrashWindow) {
      select.value = String(trashDays);
      return;
    }
    if (days === previous) return;
    savingTrashWindow = true;
    pendingTrashDays = days;
    try {
      let confirmedCount: number | undefined;
      if (days < previous) {
        confirmedCount = await countSoftDeletedOlderThan(days);
        if (!Number.isInteger(confirmedCount) || confirmedCount < 0) {
          throw new Error('Invalid purge preview');
        }
        if (confirmedCount > 0 || days === 7) {
          const noun = confirmedCount === 1 ? 'recording' : 'recordings';
          const impact = confirmedCount > 0
            ? `${confirmedCount} ${noun} already exceed the new limit and will be permanently deleted at the next cleanup.`
            : `No recordings currently exceed the ${days}-day limit.`;
          // Capture the enabled invoker even if focus moved during the preview.
          select.focus();
          const confirmed = await confirmDialog({
            title: `Shorten Trash retention to ${days} days?`,
            message: `This change affects recordings already in Trash and future recordings. ${impact} Cleanup runs about five minutes after launch, then once daily while FerriScribe is open. Their audio, transcripts, SOAP notes and generated documents cannot be restored from Trash afterward.`,
            confirmLabel: `Set ${days}-day window`,
            cancelLabel: `Keep ${previous} days`,
            danger: true,
          });
          if (!confirmed) {
            pendingTrashDays = null;
            select.value = String(previous);
            await tick();
            select.focus();
            return;
          }
        }
      }
      await settings.updateField('trash_retention_days', days, confirmedCount);
    } catch {
      // Includes a stale preview rejected at the shared persistence boundary.
      pendingTrashDays = null;
      select.value = String(settings.state.trash_retention_days ?? 30);
      toasts.error('Could not change the Trash window. Refresh and try again.');
      await tick();
      select.focus();
    } finally {
      pendingTrashDays = null;
      savingTrashWindow = false;
    }
  }
  async function handleRetentionChange(e: Event) {
    const days = Number((e.currentTarget as HTMLSelectElement).value);
    // "Never automatically move to Trash" maps to null so old configs and
    // an explicit off switch serialize identically on the backend
    // (Option<u32>).
    await settings.updateField('retention_days', days > 0 ? days : null);
  }

</script>

<h3 class="section-title" style="margin-top: 24px">Recording retention</h3>
<p class="section-desc">Choose when recordings move to Trash and how long they remain available to restore. Cleanup runs about five minutes after launch, then once daily while FerriScribe is open.</p>

<h4 class="preference-heading">Move to Trash</h4>

<div class="form-group">
  <label for="retention-select" class="form-label">Move recordings to Trash when older than</label>
  <select id="retention-select" aria-describedby="retention-hint" value={settings.state.retention_days ?? 0} onchange={handleRetentionChange}>
    <option value={0}>Never automatically move to Trash</option>
    <option value={30}>30 days</option>
    <option value={90}>90 days</option>
    <option value={180}>180 days</option>
    <option value={365}>365 days</option>
  </select>
  <span id="retention-hint" class="form-hint">Restoring a recording exempts it from future automatic cleanup.</span>
</div>

<h4 class="preference-heading">Permanent deletion</h4>
<div class="form-group">
  <label for="trash-retention-select" class="form-label">Permanently delete from Trash after</label>
  <select id="trash-retention-select" aria-describedby={trashDays === 7 ? 'trash-retention-hint trash-retention-warning' : 'trash-retention-hint'} value={trashDays} onchange={handleTrashRetentionChange}>
    <option value={7}>7 days</option>
    <option value={14}>14 days</option>
    <option value={21}>21 days</option>
    <option value={30}>30 days (default)</option>
  </select>
  <span id="trash-retention-hint" class="form-hint">When the window ends, cleanup permanently deletes the audio, transcript, SOAP notes and generated documents. They cannot be restored from Trash.</span>
  {#if trashDays === 7}
    <div id="trash-retention-warning" class="trash-warning">
      <Callout kind="warning">Seven days is the shortest recovery window. Review Trash before saving: recordings already there may become eligible for permanent deletion.</Callout>
    </div>
  {/if}
</div>

<style>
  .trash-warning { grid-column: 1 / -1; }
</style>

