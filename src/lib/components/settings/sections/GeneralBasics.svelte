<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';
  import { theme } from '../../../stores/theme.svelte';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';

  async function handleThemeChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value as 'light' | 'dark';
    theme.set(value);
    await settings.updateField('theme', value);
  }

  async function handleLanguageChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value;
    await settings.updateField('language', value);
  }

  async function handleAutosaveChange(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    await settings.updateField('autosave_enabled', checked);
  }

  async function handleAutosaveIntervalChange(e: Event) {
    const value = parseInt((e.target as HTMLInputElement).value, 10);
    if (!isNaN(value) && value >= 10 && value <= 600) {
      await settings.updateField('autosave_interval_secs', value);
    }
  }

  async function handleBrowseStoragePath() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: 'Select Recording Storage Folder',
    });
    if (selected) {
      await settings.updateField('storage_path', selected);
    }
  }

  async function handleResetStoragePath() {
    await settings.updateField('storage_path', null);
  }

  async function handleRerunSetup() {
    await settings.updateField('onboarding_completed', false);
  }
</script>

<h3 class="section-title">General</h3>

<div class="form-group">
  <label for="lang-select" class="form-label">Transcription Language</label>
  <select id="lang-select" value={settings.state.language} onchange={handleLanguageChange}>
    <option value="en-US">English</option>
    <option value="es-ES">Spanish (Español)</option>
    <option value="fr-FR">French (Français)</option>
    <option value="de-DE">German (Deutsch)</option>
    <option value="zh-CN">Chinese (中文)</option>
    <option value="pt-BR">Portuguese (Português)</option>
    <option value="ar-SA">Arabic (العربية)</option>
    <option value="ja-JP">Japanese (日本語)</option>
    <option value="hi-IN">Hindi (हिन्दी)</option>
    <option value="">Auto-detect (less reliable)</option>
  </select>
  <span class="form-hint">Controls the language hint passed to the transcription engine. Affects every recording.</span>
</div>

<div class="form-group">
  <label for="theme-select" class="form-label">Theme</label>
  <select id="theme-select" value={settings.state.theme} onchange={handleThemeChange}>
    <option value="dark">Dark</option>
    <option value="light">Light</option>
  </select>
</div>

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
  />
  <span class="form-hint">Between 10 and 600 seconds</span>
</div>

<div class="form-group">
  <span class="form-label">Recording Storage Folder</span>
  <div class="storage-path-row">
    <span class="storage-path-display">
      {settings.state.storage_path || 'Default (application data)'}
    </span>
    <button class="btn-browse" onclick={handleBrowseStoragePath}>Browse</button>
    {#if settings.state.storage_path}
      <button class="btn-reset" onclick={handleResetStoragePath}>Reset</button>
    {/if}
  </div>
  <span class="form-hint">Choose where audio recordings are saved. New recordings will use this folder.</span>
</div>

<div class="form-group">
  <span class="form-label">Setup Wizard</span>
  <div class="storage-path-row">
    <button class="btn-browse" onclick={handleRerunSetup}>Re-run setup</button>
  </div>
  <span class="form-hint">Walk through the first-run setup again to reconfigure your AI provider, transcription model, or office-server pairing.</span>
</div>

<style>
  .storage-path-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .storage-path-display {
    flex: 1;
    font-size: 12px;
    color: var(--text-muted);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 6px 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .btn-browse,
  .btn-reset {
    flex-shrink: 0;
    padding: 6px 12px;
    font-size: 12px;
    font-weight: 500;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .btn-browse {
    background-color: var(--accent);
    color: var(--text-inverse);
  }

  .btn-browse:hover {
    background-color: var(--accent-hover);
  }

  .btn-reset {
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
  }

  .btn-reset:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }
</style>
