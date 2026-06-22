<script lang="ts">
  import { settings } from '../../stores/settings.svelte';
  import { updater } from '../../stores/updater.svelte';

  const appVersion = __APP_VERSION__;

  async function toggleAutoUpdate(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    await settings.updateField('auto_update_check', checked);
    if (checked) {
      updater.startAutoCheck();
    } else {
      updater.stopAutoCheck();
    }
  }

  async function checkNow() {
    await updater.checkForUpdate();
  }
</script>

<div class="about-pane">
  <div class="version-section">
    <span class="app-icon" aria-hidden="true">🎙️</span>
    <div class="version-info">
      <h2>FerriScribe</h2>
      <span class="version-num">Version {appVersion}</span>
    </div>
  </div>

  <div class="form-group">
    <label class="checkbox-row">
      <input
        type="checkbox"
        checked={settings.state.auto_update_check}
        onchange={toggleAutoUpdate}
      />
      <span>
        <strong>Check for updates automatically</strong><br />
        <span class="form-hint">Checks GitHub Releases on launch and every 12 hours. No patient data is sent — only an anonymous version check.</span>
      </span>
    </label>
  </div>

  <div class="form-group">
    <button
      class="btn-check"
      onclick={checkNow}
      disabled={updater.state === 'checking' || updater.state === 'downloading'}
    >
      {updater.state === 'checking' ? 'Checking…' : 'Check for updates now'}
    </button>
    {#if updater.lastCheckedAt}
      <span class="last-checked">Last checked: {updater.lastCheckedAt.toLocaleString()}</span>
    {/if}
  </div>

  {#if updater.state === 'available'}
    <div class="update-status available">
      🟢 FerriScribe {updater.availableVersion} is available.
      <button class="btn-install" onclick={() => updater.downloadAndInstall()}>Download &amp; Install</button>
    </div>
  {:else if updater.state === 'downloading'}
    <div class="update-status downloading">
      Downloading… {updater.downloadProgress}%
      <div class="progress-bar"><div class="progress-fill" style="width: {updater.downloadProgress}%"></div></div>
    </div>
  {:else if updater.state === 'installed'}
    <div class="update-status installed">
      ✓ Update installed.
      <button class="btn-install" onclick={() => updater.relaunch()}>Restart now</button>
    </div>
  {:else if updater.state === 'error'}
    <div class="update-status error">
      ⚠ {updater.errorMessage}
      <button class="btn-install" onclick={() => updater.downloadAndInstall()}>Retry</button>
    </div>
  {/if}

  <p class="privacy-note">
    FerriScribe never sends patient data to any server. Update checks contact
    only <code>github.com/cortexuvula/rustMedicalAssistant</code> for the
    latest version manifest.
  </p>
</div>

<style>
  .about-pane { display: flex; flex-direction: column; gap: 20px; }
  .version-section { display: flex; align-items: center; gap: 16px; }
  .app-icon { font-size: 48px; }
  .version-info { display: flex; flex-direction: column; gap: 2px; }
  .version-info h2 { margin: 0; font-size: 22px; font-weight: 700; }
  .version-num { font-size: 13px; color: var(--text-muted); }
  .form-group { display: flex; flex-direction: column; gap: 8px; }
  .checkbox-row { display: flex; gap: 10px; align-items: flex-start; cursor: pointer; font-size: 13px; }
  .checkbox-row input { margin-top: 3px; flex-shrink: 0; }
  .form-hint { font-size: 12px; color: var(--text-muted); }
  .btn-check {
    align-self: flex-start; padding: 8px 16px; font-size: 13px; font-weight: 500;
    color: var(--text-primary); background-color: transparent;
    border: 1px solid var(--border, #333); border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-check:hover:not(:disabled) { border-color: var(--accent, #3b82f6); }
  .btn-check:disabled { opacity: 0.6; cursor: not-allowed; }
  .last-checked { font-size: 12px; color: var(--text-muted); }
  .update-status {
    padding: 12px; border-radius: var(--radius-md, 8px); font-size: 13px;
    display: flex; align-items: center; gap: 12px; flex-wrap: wrap;
  }
  .update-status.available { background-color: color-mix(in srgb, var(--success, #22c55e) 10%, transparent); }
  .update-status.downloading { background-color: color-mix(in srgb, var(--accent, #3b82f6) 10%, transparent); }
  .update-status.installed { background-color: color-mix(in srgb, var(--success, #22c55e) 10%, transparent); }
  .update-status.error { background-color: color-mix(in srgb, var(--danger, #ef4444) 10%, transparent); }
  .btn-install {
    padding: 4px 14px; font-size: 12px; font-weight: 600; color: white;
    background-color: var(--accent, #3b82f6); border: none;
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-install:hover { background-color: var(--accent-hover, #2563eb); }
  .progress-bar { flex: 1; min-width: 100px; height: 6px; background-color: var(--border, #333); border-radius: 3px; overflow: hidden; }
  .progress-fill { height: 100%; background-color: var(--accent, #3b82f6); transition: width 0.3s ease; }
  .privacy-note { font-size: 11px; color: var(--text-muted); line-height: 1.5; margin: 0; }
  .privacy-note code { font-family: ui-monospace, monospace; }
</style>
