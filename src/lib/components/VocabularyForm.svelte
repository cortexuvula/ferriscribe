<script lang="ts">
  import type { VocabularyEntry } from '../api/vocabulary';

  interface Category { value: string; label: string; }

  interface Props {
    editing: VocabularyEntry | null;
    categories: Category[];
    onSave: (values: {
      findText: string;
      replacement: string;
      category: string;
      caseSensitive: boolean;
      priority: number;
      enabled: boolean;
    }) => Promise<void> | void;
    onCancel: () => void;
  }

  let { editing, categories, onSave, onCancel }: Props = $props();

  let formFind = $state('');
  let formReplace = $state('');
  let formCategory = $state('general');
  let formCaseSensitive = $state(false);
  let formPriority = $state(0);
  let formEnabled = $state(true);
  let formError = $state('');

  $effect(() => {
    if (editing) {
      formFind = editing.find_text;
      formReplace = editing.replacement;
      formCategory = editing.category;
      formCaseSensitive = editing.case_sensitive;
      formPriority = editing.priority;
      formEnabled = editing.enabled;
    } else {
      formFind = '';
      formReplace = '';
      formCategory = 'general';
      formCaseSensitive = false;
      formPriority = 0;
      formEnabled = true;
    }
    formError = '';
  });

  async function handleSave() {
    if (!formFind.trim() || !formReplace.trim()) {
      formError = 'Find and replacement text are required.';
      return;
    }
    try {
      await onSave({
        findText: formFind.trim(),
        replacement: formReplace.trim(),
        category: formCategory,
        caseSensitive: formCaseSensitive,
        priority: formPriority,
        enabled: formEnabled,
      });
    } catch (err) {
      formError = String(err) || 'Failed to save entry.';
    }
  }
</script>

<div class="vocab-form">
  <div class="form-header">
    <h3>{editing ? 'Edit' : 'Add'} Entry</h3>
    <button class="btn-close-form" aria-label="Close form" onclick={onCancel}>&times;</button>
  </div>
  {#if formError}
    <div class="form-error">{formError}</div>
  {/if}
  <div class="form-grid">
    <label class="field">
      <span>Find Text</span>
      <input type="text" bind:value={formFind} placeholder="e.g. htn" />
    </label>
    <label class="field">
      <span>Replacement</span>
      <input type="text" bind:value={formReplace} placeholder="e.g. hypertension" />
    </label>
    <label class="field">
      <span>Category</span>
      <select bind:value={formCategory}>
        {#each categories as cat}
          <option value={cat.value}>{cat.label}</option>
        {/each}
      </select>
    </label>
    <label class="field">
      <span>Priority</span>
      <input type="number" bind:value={formPriority} min="0" max="100" />
    </label>
  </div>
  <div class="form-toggles">
    <label class="vocab-toggle">
      <input type="checkbox" bind:checked={formCaseSensitive} />
      <span class="toggle-text">Case sensitive</span>
    </label>
    <label class="vocab-toggle">
      <input type="checkbox" bind:checked={formEnabled} />
      <span class="toggle-text">Enabled</span>
    </label>
  </div>
  <div class="form-actions">
    <button class="btn-save" onclick={handleSave}>Save</button>
    <button class="btn-cancel" onclick={onCancel}>Cancel</button>
  </div>
</div>

<style>
  .vocab-form {
    padding: 14px 20px;
    border-bottom: 1px solid var(--border-color, #333);
    background: var(--bg-primary, #111);
  }
  .form-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 10px;
  }
  .form-header h3 { margin: 0; font-size: 0.95rem; font-weight: 600; }
  .btn-close-form {
    background: none;
    border: none;
    color: var(--text-secondary, #888);
    font-size: 1.2rem;
    line-height: 1;
    padding: 2px 6px;
    cursor: pointer;
    border-radius: 3px;
  }
  .btn-close-form:hover { background: rgba(255, 255, 255, 0.08); }
  .form-error {
    color: #ff6b6b;
    margin-bottom: 10px;
    font-size: 0.85rem;
    padding: 6px 10px;
    background: rgba(255, 107, 107, 0.1);
    border-radius: 4px;
  }
  .form-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    margin-bottom: 10px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 0.8rem;
    color: var(--text-secondary, #aaa);
  }
  .field span { font-weight: 500; }
  .field input,
  .field select {
    padding: 7px 10px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: var(--bg-secondary, #1e1e1e);
    color: var(--text-primary, #e0e0e0);
    font-size: 0.9rem;
  }
  .form-toggles {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 24px;
    margin-bottom: 14px;
    padding: 4px 0;
  }
  .vocab-toggle {
    display: inline-flex !important;
    flex: 0 0 auto;
    align-items: center;
    gap: 8px;
    font-size: 0.88rem;
    line-height: 1;
    cursor: pointer;
    user-select: none;
    white-space: nowrap;
    color: var(--text-primary, #e0e0e0);
  }
  .vocab-toggle input[type="checkbox"] {
    flex: 0 0 auto;
    margin: 0;
    padding: 0;
    cursor: pointer;
    width: 14px !important;
    height: 14px;
    min-width: 14px;
  }
  .toggle-text {
    display: inline-block;
    white-space: nowrap;
  }
  .form-actions { display: flex; gap: 8px; }
  .btn-save {
    padding: 7px 18px;
    border-radius: 4px;
    border: none;
    background: var(--accent-color, #4a9eff);
    color: white;
    cursor: pointer;
    font-size: 0.9rem;
  }
  .btn-save:hover { filter: brightness(1.1); }
  .btn-cancel {
    padding: 7px 18px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: transparent;
    color: var(--text-primary, #e0e0e0);
    cursor: pointer;
    font-size: 0.9rem;
  }
  .btn-cancel:hover { background: rgba(255, 255, 255, 0.05); }
</style>
