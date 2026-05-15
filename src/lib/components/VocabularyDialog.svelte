<script lang="ts">
  import {
    listVocabularyEntries,
    addVocabularyEntry,
    updateVocabularyEntry,
    deleteVocabularyEntry,
    deleteAllVocabularyEntries,
    testVocabularyCorrection,
    type VocabularyEntry,
    type CorrectionResult,
  } from '../api/vocabulary';
  import { toasts } from '../stores/toasts';
  import { formatError } from '../types/errors';
  import { onMount, onDestroy } from 'svelte';
  import VocabularyForm from './VocabularyForm.svelte';

  interface Props {
    open: boolean;
    onclose: () => void;
  }

  let { open, onclose }: Props = $props();

  function handleEscape(e: KeyboardEvent) {
    if (open && e.key === 'Escape') {
      onclose();
      e.stopImmediatePropagation();
    }
  }

  onMount(() => {
    window.addEventListener('keydown', handleEscape, { capture: true });
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handleEscape, { capture: true });
  });

  let entries = $state<VocabularyEntry[]>([]);
  let loading = $state(false);
  let filterCategory = $state('all');
  let searchText = $state('');

  // Add/Edit form
  let editing = $state<VocabularyEntry | null>(null);
  let showForm = $state(false);

  // Test area
  let testInput = $state('');
  let testResult = $state<CorrectionResult | null>(null);
  let testError = $state<string | null>(null);
  let testing = $state(false);

  const CATEGORIES = [
    { value: 'general', label: 'General' },
    { value: 'doctor_names', label: 'Doctor Names' },
    { value: 'medication_names', label: 'Medications' },
    { value: 'medical_terminology', label: 'Terminology' },
    { value: 'abbreviations', label: 'Abbreviations' },
  ];

  function categoryLabel(value: string): string {
    return CATEGORIES.find((c) => c.value === value)?.label ?? value;
  }

  async function loadEntries() {
    loading = true;
    try {
      const cat = filterCategory === 'all' ? undefined : filterCategory;
      entries = await listVocabularyEntries(cat);
    } catch (err) {
      console.error('Failed to load vocabulary entries:', err);
      toasts.error(`Failed to load vocabulary entries: ${err}`);
    } finally {
      loading = false;
    }
  }

  function filteredEntries(): VocabularyEntry[] {
    if (!searchText.trim()) return entries;
    const q = searchText.toLowerCase();
    return entries.filter(
      (e) =>
        e.find_text.toLowerCase().includes(q) ||
        e.replacement.toLowerCase().includes(q),
    );
  }

  function openAddForm() {
    editing = null;
    showForm = true;
  }

  function openEditForm(entry: VocabularyEntry) {
    editing = entry;
    showForm = true;
  }

  async function handleDelete(entry: VocabularyEntry) {
    if (!confirm(`Delete correction "${entry.find_text}" \u2192 "${entry.replacement}"?`)) return;
    try {
      await deleteVocabularyEntry(entry.id);
      await loadEntries();
    } catch (err) {
      console.error('Failed to delete entry:', err);
      toasts.error(`Failed to delete entry: ${err}`);
    }
  }

  async function handleDeleteAll() {
    if (!confirm(`Delete ALL ${entries.length} vocabulary entries? This cannot be undone.`)) return;
    try {
      await deleteAllVocabularyEntries();
      await loadEntries();
    } catch (err) {
      console.error('Failed to delete all entries:', err);
      toasts.error(`Failed to delete all entries: ${err}`);
    }
  }

  async function handleToggleEnabled(entry: VocabularyEntry) {
    try {
      await updateVocabularyEntry(
        entry.id,
        entry.find_text,
        entry.replacement,
        entry.category,
        entry.case_sensitive,
        entry.priority,
        !entry.enabled,
      );
      await loadEntries();
    } catch (err) {
      console.error('Failed to toggle entry:', err);
      toasts.error(`Failed to toggle entry: ${err}`);
    }
  }

  async function handleTest() {
    if (!testInput.trim()) return;
    testError = null;
    testing = true;
    try {
      testResult = await testVocabularyCorrection(testInput);
    } catch (err) {
      console.error('Test failed:', err);
      testError = formatError(err) || 'Test failed.';
    } finally {
      testing = false;
    }
  }

  $effect(() => {
    if (open) {
      loadEntries();
      testResult = null;
      testError = null;
    }
  });

  $effect(() => {
    filterCategory;
    if (open) loadEntries();
  });
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="vocab-overlay" onclick={onclose}>
    <div class="vocab-dialog" onclick={(e) => e.stopPropagation()}>
      <div class="vocab-header">
        <h2>Manage Vocabulary</h2>
        <button class="btn-close" onclick={onclose}>&times;</button>
      </div>

      <div class="vocab-toolbar">
        <select class="filter-select" bind:value={filterCategory}>
          <option value="all">All Categories</option>
          {#each CATEGORIES as cat}
            <option value={cat.value}>{cat.label}</option>
          {/each}
        </select>
        <input
          class="search-input"
          type="text"
          placeholder="Search find or replacement text..."
          bind:value={searchText}
        />
        <button class="btn-add" onclick={openAddForm}>+ Add Entry</button>
      </div>

      <div class="vocab-body">
        {#if showForm}
          <VocabularyForm
            {editing}
            categories={CATEGORIES}
            onSave={async (values) => {
              if (editing) {
                await updateVocabularyEntry(
                  editing.id,
                  values.findText,
                  values.replacement,
                  values.category,
                  values.caseSensitive,
                  values.priority,
                  values.enabled,
                );
              } else {
                await addVocabularyEntry(
                  values.findText,
                  values.replacement,
                  values.category,
                  values.caseSensitive,
                  values.priority,
                  values.enabled,
                );
              }
              showForm = false;
              editing = null;
              await loadEntries();
            }}
            onCancel={() => { showForm = false; editing = null; }}
          />
        {/if}

        <div class="vocab-table-wrap">
          {#if loading}
            <p class="loading-text">Loading...</p>
          {:else if filteredEntries().length === 0}
            <p class="empty-text">No vocabulary entries found.</p>
          {:else}
            <table class="vocab-table">
              <thead>
                <tr>
                  <th>Find</th>
                  <th>Replace With</th>
                  <th class="col-category">Category</th>
                  <th class="col-enabled">Enabled</th>
                  <th class="col-actions">Actions</th>
                </tr>
              </thead>
              <tbody>
                {#each filteredEntries() as entry (entry.id)}
                  <tr class:disabled={!entry.enabled}>
                    <td class="mono">{entry.find_text}</td>
                    <td class="truncate">{entry.replacement}</td>
                    <td class="col-category">{categoryLabel(entry.category)}</td>
                    <td class="col-enabled">
                      <input
                        type="checkbox"
                        checked={entry.enabled}
                        onchange={() => handleToggleEnabled(entry)}
                      />
                    </td>
                    <td class="col-actions actions">
                      <button class="btn-edit" onclick={() => openEditForm(entry)}>Edit</button>
                      <button class="btn-delete" onclick={() => handleDelete(entry)}>Del</button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {/if}
        </div>

      </div>

      <div class="vocab-test">
        <h3>Test Corrections</h3>
        <textarea
          bind:value={testInput}
          placeholder="Paste sample text to test corrections..."
          rows="3"
        ></textarea>
        <button class="btn-test" onclick={handleTest} disabled={!testInput.trim() || testing}>
          {testing ? 'Testing...' : 'Test'}
        </button>
        {#if testError}
          <div class="test-error">{testError}</div>
        {/if}
        {#if testResult}
          <div class="test-result">
            <strong>{testResult.total_replacements} replacement{testResult.total_replacements !== 1 ? 's' : ''}</strong>
            <pre>{testResult.corrected_text}</pre>
          </div>
        {/if}
      </div>

      <div class="vocab-footer">
        <span class="footer-count">
          {filteredEntries().length} shown{searchText || filterCategory !== 'all' ? ` of ${entries.length}` : ''}
        </span>
        <button class="btn-delete-all" onclick={handleDeleteAll} disabled={entries.length === 0}>
          Delete All ({entries.length})
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .vocab-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }
  .vocab-dialog {
    background: var(--bg-secondary, #1e1e1e);
    color: var(--text-primary, #e0e0e0);
    border-radius: 8px;
    width: 90vw;
    max-width: 880px;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }

  /* Header */
  .vocab-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 14px 20px;
    border-bottom: 1px solid var(--border-color, #333);
    flex: 0 0 auto;
  }
  .vocab-header h2 { margin: 0; font-size: 1.1rem; font-weight: 600; }
  .btn-close {
    background: none;
    border: none;
    color: var(--text-secondary, #aaa);
    font-size: 1.4rem;
    line-height: 1;
    padding: 4px 8px;
    cursor: pointer;
    border-radius: 4px;
  }
  .btn-close:hover { background: rgba(255, 255, 255, 0.08); }

  /* Toolbar */
  .vocab-toolbar {
    display: flex;
    gap: 8px;
    padding: 10px 20px;
    border-bottom: 1px solid var(--border-color, #333);
    flex: 0 0 auto;
    align-items: center;
  }
  .filter-select {
    flex: 0 0 180px;
    padding: 6px 10px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: var(--bg-primary, #111);
    color: var(--text-primary, #e0e0e0);
    font-size: 0.9rem;
  }
  .search-input {
    flex: 1 1 auto;
    min-width: 0;
    padding: 6px 10px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: var(--bg-primary, #111);
    color: var(--text-primary, #e0e0e0);
    font-size: 0.9rem;
  }
  .btn-add {
    flex: 0 0 auto;
    padding: 6px 14px;
    border-radius: 4px;
    border: none;
    background: var(--accent-color, #4a9eff);
    color: white;
    cursor: pointer;
    white-space: nowrap;
    font-size: 0.9rem;
  }
  .btn-add:hover { filter: brightness(1.1); }

  /* Body (single scroll container) */
  .vocab-body {
    flex: 1 1 auto;
    overflow-y: auto;
    min-height: 0;
  }

  /* Override global input { width: 100% } for all checkboxes in this dialog */
  .vocab-dialog input[type="checkbox"] {
    width: 14px !important;
    height: 14px;
    min-width: 14px;
    padding: 0;
    margin: 0;
    vertical-align: middle;
  }
  /* Table */
  .vocab-table-wrap {
    padding: 8px 20px 16px;
  }
  .loading-text, .empty-text {
    text-align: center;
    color: var(--text-secondary, #888);
    padding: 32px;
    font-size: 0.9rem;
  }
  .vocab-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.88rem;
    table-layout: fixed;
  }
  .vocab-table th {
    text-align: left;
    padding: 8px 8px;
    border-bottom: 1px solid var(--border-color, #333);
    color: var(--text-secondary, #888);
    font-weight: 500;
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    position: sticky;
    top: 0;
    background: var(--bg-secondary, #1e1e1e);
    z-index: 1;
  }
  .vocab-table td {
    padding: 8px;
    border-bottom: 1px solid var(--border-color, #222);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .vocab-table tr.disabled td { opacity: 0.45; }
  .vocab-table tr:hover td { background: rgba(255, 255, 255, 0.03); }
  .mono { font-family: 'SF Mono', Menlo, Consolas, monospace; color: var(--text-primary, #e0e0e0); }
  .truncate { max-width: 0; }
  .col-category { width: 140px; color: var(--text-secondary, #aaa); }
  .col-enabled { width: 70px; text-align: center; }
  .col-enabled input { cursor: pointer; }
  .col-actions { width: 110px; }
  .actions { display: flex; gap: 4px; }
  .btn-edit, .btn-delete {
    padding: 3px 10px;
    border-radius: 3px;
    border: 1px solid var(--border-color, #444);
    background: transparent;
    color: var(--text-secondary, #bbb);
    cursor: pointer;
    font-size: 0.78rem;
  }
  .btn-edit:hover { background: rgba(255, 255, 255, 0.05); }
  .btn-delete { color: #ff6b6b; border-color: #ff6b6b44; }
  .btn-delete:hover { background: rgba(255, 107, 107, 0.08); }

  /* Test area */
  .vocab-test {
    flex: 0 0 auto;
    padding: 12px 20px 16px;
    border-top: 1px solid var(--border-color, #333);
    background: var(--bg-primary, #111);
    max-height: 40vh;
    overflow-y: auto;
  }
  .vocab-test h3 { margin: 0 0 8px; font-size: 0.9rem; font-weight: 600; }
  .vocab-test textarea {
    width: 100%;
    padding: 8px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: var(--bg-secondary, #1e1e1e);
    color: var(--text-primary, #e0e0e0);
    resize: vertical;
    font-family: inherit;
    font-size: 0.88rem;
    box-sizing: border-box;
  }
  .btn-test {
    margin-top: 8px;
    padding: 6px 14px;
    border-radius: 4px;
    border: none;
    background: var(--accent-color, #4a9eff);
    color: white;
    cursor: pointer;
    font-size: 0.88rem;
  }
  .btn-test:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn-test:not(:disabled):hover { filter: brightness(1.1); }
  .test-error {
    margin-top: 10px;
    padding: 8px 10px;
    border-radius: 4px;
    background: rgba(255, 107, 107, 0.1);
    color: #ff6b6b;
    font-size: 0.85rem;
    border: 1px solid rgba(255, 107, 107, 0.3);
    word-wrap: break-word;
  }
  .test-result { margin-top: 10px; }
  .test-result strong { font-size: 0.85rem; color: var(--accent-color, #4a9eff); }
  .test-result pre {
    background: var(--bg-secondary, #1e1e1e);
    padding: 10px;
    border-radius: 4px;
    white-space: pre-wrap;
    word-wrap: break-word;
    font-size: 0.85rem;
    margin-top: 6px;
    border: 1px solid var(--border-color, #333);
  }

  /* Footer */
  .vocab-footer {
    padding: 10px 20px;
    border-top: 1px solid var(--border-color, #333);
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex: 0 0 auto;
  }
  .footer-count {
    font-size: 0.82rem;
    color: var(--text-secondary, #888);
  }
  .btn-delete-all {
    padding: 6px 14px;
    border-radius: 4px;
    border: 1px solid #ff6b6b44;
    background: transparent;
    color: #ff6b6b;
    cursor: pointer;
    font-size: 0.88rem;
  }
  .btn-delete-all:not(:disabled):hover { background: rgba(255, 107, 107, 0.08); }
  .btn-delete-all:disabled { opacity: 0.4; cursor: not-allowed; }
</style>
