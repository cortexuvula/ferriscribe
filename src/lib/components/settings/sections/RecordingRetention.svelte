<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';
  async function handleRetentionChange(e: Event) {
    const days = Number((e.currentTarget as HTMLSelectElement).value);
    // "Never (keep forever)" maps to null so old configs and an explicit
    // off switch serialize identically on the backend (Option<u32>).
    await settings.updateField('retention_days', days > 0 ? days : null);
  }

</script>

<h3 class="section-title" style="margin-top: 24px">Recording Retention</h3>
<p class="section-desc">Automatically move aging recordings to trash once a day when a retention window is set.</p>

<div class="form-group">
  <label for="retention-select" class="form-label">Automatically move recordings to trash when older than</label>
  <select id="retention-select" value={settings.state.retention_days ?? 0} onchange={handleRetentionChange}>
    <option value={0}>Never (keep forever)</option>
    <option value={30}>30 days</option>
    <option value={90}>90 days</option>
    <option value={180}>180 days</option>
    <option value={365}>365 days</option>
  </select>
  <span class="form-hint">Trashed recordings keep a 30-day undo window before permanent deletion.
    Restoring a recording exempts it from future automatic cleanup.</span>
</div>

