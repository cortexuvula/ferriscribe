<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { settings } from '../../stores/settings.svelte';
  import { theme } from '../../stores/theme.svelte.ts';
  import { contextTemplates } from '../../stores/contextTemplates.svelte';
  import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
  import VocabularyDialog from '../VocabularyDialog.svelte';
  import ContextTemplateDialog from '../ContextTemplateDialog.svelte';
  import DictionaryDialog from '../DictionaryDialog.svelte';
  import { getVocabularyCount, importVocabularyJson, exportVocabularyJson } from '../../api/vocabulary';
  import { importContextTemplatesJson, exportContextTemplatesJson } from '../../api/contextTemplates';
  import { listUserDict } from '../../api/userDictionary';
  import { reinitProviders } from '../../api/chat';

  let vocabDialogOpen = $state(false);
  let vocabCount = $state<[number, number]>([0, 0]);
  let ctxTemplateDialogOpen = $state(false);
  let ctxTemplateCount = $derived(contextTemplates.list.length);
  let dictDialogOpen = $state(false);
  let dictCount = $state(0);
  let encryptionState = $state<'no-database' | 'plaintext' | 'encrypted' | 'unknown'>('unknown');

  async function handleThemeChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value as 'light' | 'dark';
    theme.set(value);
    await settings.updateField('theme', value);
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

  // Re-run the first-run onboarding wizard. Flipping onboarding_completed to
  // false makes App.svelte's reactive gate unmount the app shell (and this
  // settings modal) and render <OnboardingWizard> in its place. The wizard's
  // Done button flips it back to true. The wizard pre-fills from current
  // settings, so this is a no-risk way to reconfigure after a provider switch.
  async function handleRerunSetup() {
    await settings.updateField('onboarding_completed', false);
  }

  async function loadVocabCount() {
    try {
      vocabCount = await getVocabularyCount();
    } catch (err) {
      console.error('Failed to load vocabulary count:', err);
    }
  }

  async function handleImportVocabulary() {
    const selected = await openDialog({
      multiple: false,
      title: 'Import Vocabulary JSON',
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (selected) {
      try {
        const count = await importVocabularyJson(selected as string);
        alert(`Imported ${count} vocabulary entries.`);
        await loadVocabCount();
      } catch (err) {
        alert(`Import failed: ${err}`);
      }
    }
  }

  async function handleExportVocabulary() {
    const selected = await saveDialog({
      title: 'Export Vocabulary JSON',
      defaultPath: 'vocabulary.json',
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (selected) {
      try {
        const count = await exportVocabularyJson(selected);
        alert(`Exported ${count} vocabulary entries.`);
      } catch (err) {
        alert(`Export failed: ${err}`);
      }
    }
  }

  function handleVocabDialogClose() {
    vocabDialogOpen = false;
    loadVocabCount();
  }

  async function loadDictCount() {
    try {
      const words = await listUserDict();
      dictCount = words.length;
    } catch (err) {
      console.error('Failed to load dictionary count:', err);
    }
  }

  function handleDictDialogClose() {
    dictDialogOpen = false;
    loadDictCount();
  }

  async function handleImportCtxTemplates() {
    const selected = await openDialog({
      multiple: false,
      title: 'Import Context Templates JSON',
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (selected) {
      try {
        const count = await importContextTemplatesJson(selected as string);
        alert(`Imported ${count} context templates.`);
        await contextTemplates.load();
      } catch (err) {
        alert(`Import failed: ${err}`);
      }
    }
  }

  async function handleExportCtxTemplates() {
    const selected = await saveDialog({
      title: 'Export Context Templates JSON',
      defaultPath: 'context_templates.json',
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (selected) {
      try {
        const count = await exportContextTemplatesJson(selected);
        alert(`Exported ${count} context templates.`);
      } catch (err) {
        alert(`Export failed: ${err}`);
      }
    }
  }

  function handleCtxTemplateDialogClose() {
    ctxTemplateDialogOpen = false;
  }

  onMount(async () => {
    const results = await Promise.allSettled([
      loadVocabCount(),
      contextTemplates.load(),
      loadEncryptionStatus(),
      loadDictCount(),
    ]);
    const labels = ['loadVocabCount', 'contextTemplates.load', 'loadEncryptionStatus', 'loadDictCount'];
    for (const [i, r] of results.entries()) {
      if (r.status === 'rejected') {
        console.error(`Settings init: ${labels[i]} failed:`, r.reason);
      }
    }
  });

  async function loadEncryptionStatus() {
    try {
      const result = await invoke<{ state: string; key_present?: boolean }>('database_encryption_status');
      encryptionState = (result.state as 'no-database' | 'plaintext' | 'encrypted') || 'unknown';
    } catch (e) {
      console.error('Failed to query database encryption status:', e);
      encryptionState = 'unknown';
    }
  }
</script>

<section class="settings-section">
  {#if settings.state.allow_public_endpoint}
    <div class="public-endpoint-banner" role="alert">
      ⚠ <strong>Public endpoints enabled.</strong> AI / STT requests may leave your device.
    </div>
  {/if}

  <h3 class="section-title">General</h3>

  <div class="form-group">
    <label for="theme-select" class="form-label">Theme</label>
    <select
      id="theme-select"
      value={settings.state.theme}
      onchange={handleThemeChange}
    >
      <option value="dark">Dark</option>
      <option value="light">Light</option>
    </select>
  </div>

  <div class="form-group">
    <label class="form-label checkbox-label">
      <input
        type="checkbox"
        checked={settings.state.autosave_enabled}
        onchange={handleAutosaveChange}
      />
      <span>Enable Autosave</span>
    </label>
  </div>

  <div class="form-group">
    <label for="autosave-interval" class="form-label">
      Autosave Interval (seconds)
    </label>
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
      <button class="btn-browse" onclick={handleBrowseStoragePath}>
        Browse
      </button>
      {#if settings.state.storage_path}
        <button class="btn-reset" onclick={handleResetStoragePath}>
          Reset
        </button>
      {/if}
    </div>
    <span class="form-hint">Choose where audio recordings are saved. New recordings will use this folder.</span>
  </div>

  <div class="form-group">
    <span class="form-label">Setup Wizard</span>
    <div class="storage-path-row">
      <button class="btn-browse" onclick={handleRerunSetup}>
        Re-run setup
      </button>
    </div>
    <span class="form-hint">Walk through the first-run setup again to reconfigure your AI provider, transcription model, or office-server pairing.</span>
  </div>

  <h3 class="section-title" style="margin-top: 24px">Database Security</h3>
  <p class="section-desc">
    Your medical records are stored in a SQLite database. The encryption
    key is stored in your operating system's keychain. Back up your
    database regularly — if the keychain entry is lost, the data cannot
    be recovered.
  </p>
  <div class="form-group">
    <span class="form-label">Encryption status</span>
    {#if encryptionState === 'encrypted'}
      <span class="status-pill encrypted">✓ Encrypted (key in OS keychain)</span>
    {:else if encryptionState === 'plaintext'}
      <span class="status-pill plaintext">⚠ Plaintext (encryption disabled)</span>
    {:else if encryptionState === 'no-database'}
      <span class="status-pill">No database yet</span>
    {:else}
      <span class="status-pill">Checking…</span>
    {/if}
  </div>

  <h3 class="section-title" style="margin-top: 24px">Custom Vocabulary</h3>
  <p class="section-desc">Automatically correct words in transcripts after speech-to-text.</p>

  <div class="form-group">
    <label class="toggle-label">
      <input
        type="checkbox"
        checked={settings.state.vocabulary_enabled}
        onchange={() => settings.updateField('vocabulary_enabled', !settings.state.vocabulary_enabled)}
      />
      <span>Enable vocabulary corrections</span>
    </label>
  </div>

  <div class="form-group">
    <span class="form-label">
      {vocabCount[0]} entries ({vocabCount[1]} enabled)
    </span>
    <div class="vocab-buttons">
      <button class="btn-browse" onclick={() => { vocabDialogOpen = true; }}>
        Manage Vocabulary
      </button>
      <button class="btn-browse" onclick={handleImportVocabulary}>
        Import JSON
      </button>
      <button class="btn-browse" onclick={handleExportVocabulary}>
        Export JSON
      </button>
    </div>
  </div>

  <h3 class="section-title" style="margin-top: 24px">Context Templates</h3>
  <p class="section-desc">Reusable snippets of clinical context that can be applied to the Patient Context field on the Record tab.</p>

  <div class="form-group">
    <span class="form-label">
      {ctxTemplateCount} template{ctxTemplateCount === 1 ? '' : 's'} saved
    </span>
    <div class="vocab-buttons">
      <button class="btn-browse" onclick={() => { ctxTemplateDialogOpen = true; }}>
        Manage Templates
      </button>
      <button class="btn-browse" onclick={handleImportCtxTemplates}>
        Import JSON
      </button>
      <button class="btn-browse" onclick={handleExportCtxTemplates}>
        Export JSON
      </button>
    </div>
  </div>

  <h3 class="section-title" style="margin-top: 24px">Spellcheck Dictionary</h3>
  <p class="section-desc">Accepted spellings for the in-app spellchecker. Words added here (or via right-click → Add to dictionary in the editor) won't be flagged as misspelled.</p>

  <div class="form-group">
    <label class="toggle-label">
      <input
        type="checkbox"
        checked={settings.state.medical_dict_enabled}
        onchange={() => settings.updateField('medical_dict_enabled', !settings.state.medical_dict_enabled)}
      />
      <span>Use bundled medical wordlist (~110,000 terms)</span>
    </label>
    <span class="form-hint">Drug names, anatomy, conditions, and syndromes. Toggling applies on the next document open.</span>
  </div>

  <div class="form-group">
    <span class="form-label">
      {dictCount} word{dictCount === 1 ? '' : 's'} saved
    </span>
    <div class="vocab-buttons">
      <button class="btn-browse" onclick={() => { dictDialogOpen = true; }}>
        Manage Dictionary
      </button>
    </div>
  </div>

  <details class="advanced-section">
    <summary>Advanced</summary>
    <div class="advanced-content">
      <label class="form-row">
        <input
          type="checkbox"
          checked={settings.state.allow_public_endpoint}
          onchange={async (e) => {
            await settings.updateField('allow_public_endpoint', (e.target as HTMLInputElement).checked);
            // allow_public is captured at provider construction; reinit so the
            // new policy takes effect immediately (not on next app launch).
            try { await reinitProviders(); } catch (err) { console.error('Failed to reinit providers after allow_public change:', err); }
          }}
        />
        <span>
          Allow public AI / STT endpoints
          <p class="hint">
            By default, FerriScribe blocks public-internet AI or STT hosts to keep
            PHI on-device. Enable this only if you understand that data may leave
            your machine.
          </p>
        </span>
      </label>

      <label class="form-row">
        <input
          type="checkbox"
          checked={settings.state.capture_for_training ?? false}
          onchange={(e) => settings.updateField('capture_for_training', (e.target as HTMLInputElement).checked)}
        />
        <span>
          Capture generations for training corpus
          <p class="hint">
            Records every SOAP generation and your edited final version into a
            local-device pool (encrypted whenever your database is encrypted).
            Useful for fine-tuning a model on your own dictation style later.
            Data stays on this device — nothing is sent anywhere.
          </p>
        </span>
      </label>
    </div>
  </details>
</section>

<VocabularyDialog open={vocabDialogOpen} onclose={handleVocabDialogClose} />
<ContextTemplateDialog open={ctxTemplateDialogOpen} onclose={handleCtxTemplateDialogClose} />
<DictionaryDialog open={dictDialogOpen} onclose={handleDictDialogClose} />

<style>
  .section-desc {
    font-size: 12px;
    color: var(--text-muted);
    margin-top: -8px;
  }

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

  .vocab-buttons {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }

  .status-pill {
    display: inline-block;
    padding: 4px 10px;
    border-radius: 4px;
    font-size: 13px;
    background: var(--bg-tertiary, #f1f3f5);
    color: var(--text-secondary, #495057);
    border: 1px solid var(--border, #dee2e6);
  }

  .status-pill.encrypted {
    background: rgba(40, 167, 69, 0.1);
    color: #155724;
    border-color: rgba(40, 167, 69, 0.3);
  }

  .status-pill.plaintext {
    background: rgba(255, 193, 7, 0.1);
    color: #856404;
    border-color: rgba(255, 193, 7, 0.3);
  }

  .public-endpoint-banner {
    background: #fef2f2;
    color: #991b1b;
    border: 1px solid #fca5a5;
    border-radius: 4px;
    padding: 8px 12px;
    margin-bottom: 12px;
    font-size: 0.9rem;
  }

  .advanced-section summary {
    cursor: pointer;
    font-weight: 600;
    margin-top: 16px;
  }

  .advanced-content {
    margin-top: 8px;
    padding-left: 16px;
  }

  .form-row {
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }

  .hint {
    color: var(--text-muted);
    font-size: 0.8rem;
    margin: 4px 0 0 0;
  }

  /* Inline a checkbox with its label text on a single row. */
  .toggle-label,
  .checkbox-label {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    user-select: none;
    font-size: 13px;
    color: var(--text-secondary);
  }
</style>
