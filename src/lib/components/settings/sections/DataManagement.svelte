<script lang="ts">
  import { onMount } from 'svelte';
  import { settings } from '../../../stores/settings.svelte';
  import { toasts } from '../../../stores/toasts.svelte';
  import { formatError } from '../../../types/errors';
  import { contextTemplates } from '../../../stores/contextTemplates.svelte';
  import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
  import VocabularyDialog from '../../VocabularyDialog.svelte';
  import ContextTemplateDialog from '../../ContextTemplateDialog.svelte';
  import DictionaryDialog from '../../DictionaryDialog.svelte';
  import { getVocabularyCount, importVocabularyJson, exportVocabularyJson } from '../../../api/vocabulary';
  import { importContextTemplatesJson, exportContextTemplatesJson } from '../../../api/contextTemplates';
  import { listUserDict } from '../../../api/userDictionary';

  let vocabDialogOpen = $state(false);
  let vocabCount = $state<[number, number]>([0, 0]);
  let ctxTemplateDialogOpen = $state(false);
  const ctxTemplateCount = $derived(contextTemplates.list.length);
  let dictDialogOpen = $state(false);
  let dictCount = $state(0);

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
        toasts.success(`Imported ${count} vocabulary entries`);
        await loadVocabCount();
      } catch (err) {
        toasts.error(formatError(err));
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
        toasts.success(`Exported ${count} vocabulary entries`);
      } catch (err) {
        toasts.error(formatError(err));
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
        toasts.success(`Imported ${count} context templates`);
        await contextTemplates.load();
      } catch (err) {
        toasts.error(formatError(err));
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
        toasts.success(`Exported ${count} context templates`);
      } catch (err) {
        toasts.error(formatError(err));
      }
    }
  }

  function handleCtxTemplateDialogClose() {
    ctxTemplateDialogOpen = false;
  }

  async function handleRetentionChange(e: Event) {
    const days = Number((e.currentTarget as HTMLSelectElement).value);
    // "Never (keep forever)" maps to null so old configs and an explicit
    // off switch serialize identically on the backend (Option<u32>).
    await settings.updateField('retention_days', days > 0 ? days : null);
  }

  onMount(async () => {
    const results = await Promise.allSettled([loadVocabCount(), contextTemplates.load(), loadDictCount()]);
    const labels = ['loadVocabCount', 'contextTemplates.load', 'loadDictCount'];
    for (const [i, r] of results.entries()) {
      if (r.status === 'rejected') {
        console.error(`DataManagement init: ${labels[i]} failed:`, r.reason);
      }
    }
  });
</script>

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
  <span class="form-label">{vocabCount[0]} entries ({vocabCount[1]} enabled)</span>
  <div class="vocab-buttons">
    <button class="btn-browse" onclick={() => { vocabDialogOpen = true; }}>Manage Vocabulary</button>
    <button class="btn-browse" onclick={handleImportVocabulary}>Import JSON</button>
    <button class="btn-browse" onclick={handleExportVocabulary}>Export JSON</button>
  </div>
</div>

<h3 class="section-title" style="margin-top: 24px">Context Templates</h3>
<p class="section-desc">Reusable snippets of clinical context that can be applied to the Patient Context field on the Record tab.</p>

<div class="form-group">
  <span class="form-label">{ctxTemplateCount} template{ctxTemplateCount === 1 ? '' : 's'} saved</span>
  <div class="vocab-buttons">
    <button class="btn-browse" onclick={() => { ctxTemplateDialogOpen = true; }}>Manage Templates</button>
    <button class="btn-browse" onclick={handleImportCtxTemplates}>Import JSON</button>
    <button class="btn-browse" onclick={handleExportCtxTemplates}>Export JSON</button>
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
  <span class="form-label">{dictCount} word{dictCount === 1 ? '' : 's'} saved</span>
  <div class="vocab-buttons">
    <button class="btn-browse" onclick={() => { dictDialogOpen = true; }}>Manage Dictionary</button>
  </div>
</div>

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

<VocabularyDialog open={vocabDialogOpen} onclose={handleVocabDialogClose} />
<ContextTemplateDialog open={ctxTemplateDialogOpen} onclose={handleCtxTemplateDialogClose} />
<DictionaryDialog open={dictDialogOpen} onclose={handleDictDialogClose} />

<style>
  .section-desc {
    font-size: 12px;
    color: var(--text-muted);
    margin-top: -8px;
  }

  .vocab-buttons {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }

  .btn-browse {
    flex-shrink: 0;
    padding: 6px 12px;
    font-size: 12px;
    font-weight: 500;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
    background-color: var(--accent);
    color: var(--text-inverse);
  }

  .btn-browse:hover {
    background-color: var(--accent-hover);
  }

  .toggle-label {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    user-select: none;
    font-size: 13px;
    color: var(--text-secondary);
  }
</style>
