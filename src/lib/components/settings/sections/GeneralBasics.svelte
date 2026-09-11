<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';
  import { theme } from '../../../stores/theme.svelte';
  import { playSoapCompleteChime } from '../../../utils/notificationSound';

  $effect(() => { theme.set(settings.state.theme); });

  async function handleThemeChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value as 'light' | 'dark';
    theme.set(value);
    try {
      await settings.updateField('theme', value);
    } catch {
      // The store reports persistence failure; reflect the reloaded value.
      theme.set(settings.state.theme as 'light' | 'dark');
    }
  }

  async function handleAutosaveChange(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    try { await settings.updateField('autosave_enabled', checked); } catch { /* SettingsContent shows saveError. */ }
  }

  async function handleSoapSoundChange(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    try {
      await settings.updateField('soap_notification_sound', checked);
    } catch { return; } // Do not preview a change that failed to save.
    if (checked) {
      // Preview the chime so the user knows exactly what they enabled.
      playSoapCompleteChime();
    }
  }

  /** Inline validation — an out-of-range interval is never persisted, so
   *  the field is reverted and told why instead of silently lying. */
  let autosaveError = $state('');

  async function handleAutosaveIntervalChange(e: Event) {
    const input = e.target as HTMLInputElement;
    const value = parseInt(input.value, 10);
    if (!isNaN(value) && value >= 10 && value <= 600) {
      autosaveError = '';
      try { await settings.updateField('autosave_interval_secs', value); } catch { /* SettingsContent shows saveError. */ }
    } else {
      autosaveError = 'Interval must be between 10 and 600 seconds.';
      input.value = String(settings.state.autosave_interval_secs);
    }
  }

</script>

<h3 class="section-title">General</h3>
<p class="section-desc">Make FerriScribe comfortable to work in.</p>
<h4 class="preference-heading">Appearance</h4>

<div class="form-group">
  <label for="theme-select" class="form-label">Theme</label>
  <select id="theme-select" value={settings.state.theme} onchange={handleThemeChange}>
    <option value="dark">Dark</option>
    <option value="light">Light</option>
  </select>
</div>

<h4 class="preference-heading">Saving</h4>
<div class="form-group">
  <label class="form-label checkbox-label">
    <input type="checkbox" checked={settings.state.autosave_enabled} onchange={handleAutosaveChange} />
    <span>Enable Autosave</span>
  </label>
</div>

<div class="form-group">
  <label for="autosave-interval" class="form-label">Autosave Interval (seconds)</label>
  <input
    id="autosave-interval"
    type="number"
    min="10"
    max="600"
    value={settings.state.autosave_interval_secs}
    onchange={handleAutosaveIntervalChange}
    disabled={!settings.state.autosave_enabled}
    aria-invalid={autosaveError ? 'true' : undefined}
  />
  {#if autosaveError}
    <span class="field-error" role="alert">{autosaveError}</span>
  {:else}
    <span class="form-hint">Between 10 and 600 seconds</span>
  {/if}
</div>

<h4 class="preference-heading">Notifications</h4>
<div class="form-group">
  <label class="form-label checkbox-label">
    <input
      type="checkbox"
      checked={settings.state.soap_notification_sound}
      onchange={handleSoapSoundChange}
    />
    <span>Play a sound when a SOAP note is generated</span>
  </label>
  <span class="form-hint">A short completion chime, played locally.</span>
</div>


<style>
  .field-error {
    font-size: 12px;
    color: var(--danger);
  }
</style>
