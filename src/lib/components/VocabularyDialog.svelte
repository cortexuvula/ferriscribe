<script lang="ts">
  import {
    listVocabularyEntries,
    addVocabularyEntry,
    updateVocabularyEntry,
    deleteVocabularyEntry,
    deleteAllVocabularyEntries,
    type VocabularyEntry,
  } from '../api/vocabulary';
  import { toasts } from '../stores/toasts.svelte';
  import { confirmDialog } from '../stores/confirm.svelte';
  import { pushOverlay, isTopmostOverlay } from '../stores/overlay';
  import { untrack } from 'svelte';
  import VocabularyForm from './VocabularyForm.svelte';
  import VocabularyTable from './VocabularyTable.svelte';
  import VocabularyTestPanel from './VocabularyTestPanel.svelte';
  import { filterVocabularyEntries } from '../utils/vocabularyFilter';
  import { onEscape } from '../actions/onEscape';

  interface Props {
    open: boolean;
    onclose: () => void;
  }

  const { open, onclose }: Props = $props();

  let root: HTMLElement | undefined = $state();

  // Overlay-stack membership: this dialog stacks on the Settings modal
  // (and a confirm dialog can stack on it) — the stack decides whose
  // Escape is whose.
  $effect(() => {
    if (open && root) {
      return pushOverlay(root);
    }
  });

  /** Escape closes only when this dialog is open AND topmost. When it
   *  didn't handle the keypress, the action lets it pass through (e.g. to
   *  the Settings modal behind it, or the confirm dialog above it). */
  function handleEscape(): boolean {
    if (!open || !root || !isTopmostOverlay(root)) return false;
    onclose();
    return true;
  }

  let entries = $state<VocabularyEntry[]>([]);
  let loading = $state(false);
  let filterCategory = $state('all');
  let searchText = $state('');

  // Add/Edit form
  let editing = $state<VocabularyEntry | null>(null);
  let showForm = $state(false);

  // Test panel reset signal
  let resetSignal = $state(0);

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

  function openAddForm() {
    editing = null;
    showForm = true;
  }

  function openEditForm(entry: VocabularyEntry) {
    editing = entry;
    showForm = true;
  }

  async function handleDelete(entry: VocabularyEntry) {
    const ok = await confirmDialog({
      title: 'Delete correction?',
      message: `Delete correction "${entry.find_text}" \u2192 "${entry.replacement}"?`,
      confirmLabel: 'Delete',
      danger: true,
    });
    if (!ok) return;
    try {
      await deleteVocabularyEntry(entry.id);
      await loadEntries();
    } catch (err) {
      console.error('Failed to delete entry:', err);
      toasts.error(`Failed to delete entry: ${err}`);
    }
  }

  async function handleDeleteAll() {
    const ok = await confirmDialog({
      title: 'Delete all vocabulary entries?',
      message: `Delete ALL ${entries.length} vocabulary entries? This cannot be undone.`,
      confirmLabel: `Delete all ${entries.length}`,
      danger: true,
    });
    if (!ok) return;
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

  $effect(() => {
    if (open) {
      loadEntries();
      // `resetSignal += 1` is a read+write; without untrack the read would
      // make this effect depend on resetSignal, and the write would then
      // re-trigger it — pegging the JS thread and freezing the dialog.
      untrack(() => { resetSignal += 1; });
    }
  });

  $effect(() => {
    // Reload whenever the dialog is open; reading `filterCategory` here
    // makes this effect re-run when the category filter changes.
    if (open && filterCategory !== undefined) loadEntries();
  });
</script>

<svelte:window use:onEscape={handleEscape} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="vocab-overlay" bind:this={root} onclick={onclose}>
    <div class="vocab-dialog" role="dialog" aria-modal="true" tabindex="-1" aria-label="Manage vocabulary" onclick={(e) => e.stopPropagation()}>
      <div class="vocab-header">
        <h2>Manage Vocabulary</h2>
        <button class="btn-close" aria-label="Close" onclick={onclose}>&times;</button>
      </div>

      <div class="vocab-toolbar">
        <select class="filter-select" bind:value={filterCategory}>
          <option value="all">All Categories</option>
          {#each CATEGORIES as cat (cat.value)}
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

        <VocabularyTable
          {entries}
          {loading}
          {searchText}
          {categoryLabel}
          onEdit={openEditForm}
          onDelete={handleDelete}
          onToggleEnabled={handleToggleEnabled}
        />

      </div>

      <VocabularyTestPanel {resetSignal} />

      <div class="vocab-footer">
        <span class="footer-count">
          {filterVocabularyEntries(entries, searchText).length} shown{searchText || filterCategory !== 'all' ? ` of ${entries.length}` : ''}
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
