<script lang="ts">
  import {
    upsertContextTemplate,
    renameContextTemplate,
    deleteContextTemplate,
    type ContextTemplate,
  } from '../api/contextTemplates';
  import { contextTemplates } from '../stores/contextTemplates.svelte';
  import { formatError } from '../types/errors';
  import { onEscape } from '../actions/onEscape';

  interface Props {
    open: boolean;
    onclose: () => void;
  }

  const { open, onclose }: Props = $props();

  // Escape-to-close is handled by the onEscape action (see <svelte:window>
  // below). The open guard prevents close when the dialog is hidden.

  let loading = $state(false);
  let searchText = $state('');

  // Add/Edit form
  let editing = $state<ContextTemplate | null>(null);
  let showForm = $state(false);
  let formName = $state('');
  let formBody = $state('');
  let formError = $state('');

  async function loadTemplates() {
    loading = true;
    try {
      await contextTemplates.load();
    } finally {
      loading = false;
    }
  }

  function filtered(): ContextTemplate[] {
    if (!searchText.trim()) return contextTemplates.list;
    const q = searchText.toLowerCase();
    return contextTemplates.list.filter(
      (t) => t.name.toLowerCase().includes(q) || t.body.toLowerCase().includes(q),
    );
  }

  function openAdd() {
    editing = null;
    formName = '';
    formBody = '';
    formError = '';
    showForm = true;
  }

  function openEdit(t: ContextTemplate) {
    editing = t;
    formName = t.name;
    formBody = t.body;
    formError = '';
    showForm = true;
  }

  function closeForm() {
    showForm = false;
    editing = null;
    formError = '';
  }

  async function handleSave() {
    const name = formName.trim();
    const body = formBody.trim();
    if (!name) { formError = 'Name is required.'; return; }
    if (!body) { formError = 'Body is required.'; return; }
    try {
      if (editing) {
        if (editing.name !== name) {
          await renameContextTemplate(editing.name, name);
        }
        await upsertContextTemplate(name, body);
      } else {
        if (contextTemplates.list.some((t) => t.name === name)) {
          formError = `A template named "${name}" already exists.`;
          return;
        }
        await upsertContextTemplate(name, body);
      }
      closeForm();
      await loadTemplates();
    } catch (err) {
      formError = formatError(err) || 'Failed to save template.';
    }
  }

  async function handleDelete(t: ContextTemplate) {
    if (!confirm(`Delete template "${t.name}"?`)) return;
    try {
      await deleteContextTemplate(t.name);
      await loadTemplates();
    } catch (err) {
      console.error('Failed to delete template:', err);
    }
  }

  $effect(() => {
    if (open) {
      loadTemplates();
    }
  });
</script>

<svelte:window use:onEscape={() => open && onclose()} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="ct-overlay" onclick={onclose}>
    <div class="ct-dialog" role="dialog" aria-modal="true" tabindex="-1" aria-label="Manage context templates" onclick={(e) => e.stopPropagation()}>
      <div class="ct-header">
        <h2>Manage Context Templates</h2>
        <button class="btn-close" aria-label="Close" onclick={onclose}>&times;</button>
      </div>

      <div class="ct-toolbar">
        <input
          class="search-input"
          type="text"
          placeholder="Search name or body..."
          bind:value={searchText}
        />
        <button class="btn-add" onclick={openAdd}>+ Add Template</button>
      </div>

      <div class="ct-body">
        {#if showForm}
          <div class="ct-form">
            <div class="form-header">
              <h3>{editing ? 'Edit' : 'Add'} Template</h3>
              <button class="btn-close-form" aria-label="Close form" onclick={closeForm}>&times;</button>
            </div>
            {#if formError}
              <div class="form-error">{formError}</div>
            {/if}
            <label class="field">
              <span>Name</span>
              <input type="text" bind:value={formName} placeholder="e.g. Follow-up visit" />
            </label>
            <label class="field">
              <span>Body</span>
              <textarea bind:value={formBody} rows="5" placeholder="Template body text..."></textarea>
            </label>
            <div class="form-actions">
              <button class="btn-save" onclick={handleSave}>Save</button>
              <button class="btn-cancel" onclick={closeForm}>Cancel</button>
            </div>
          </div>
        {/if}

        <div class="ct-list-wrap">
          {#if loading}
            <p class="loading-text">Loading...</p>
          {:else if filtered().length === 0}
            <p class="empty-text">
              {contextTemplates.list.length === 0 ? 'No templates yet. Click "+ Add Template" to create one.' : 'No templates match the search.'}
            </p>
          {:else}
            <table class="ct-table">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Body Preview</th>
                  <th class="col-actions">Actions</th>
                </tr>
              </thead>
              <tbody>
                {#each filtered() as t (t.name)}
                  <tr>
                    <td class="name-cell">{t.name}</td>
                    <td class="body-cell truncate">{t.body.replace(/\n/g, ' ')}</td>
                    <td class="col-actions actions">
                      <button class="btn-edit" onclick={() => openEdit(t)}>Edit</button>
                      <button class="btn-delete" onclick={() => handleDelete(t)}>Del</button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {/if}
        </div>
      </div>

      <div class="ct-footer">
        <span class="footer-count">
          {filtered().length} shown{searchText ? ` of ${contextTemplates.list.length}` : ''}
        </span>
      </div>
    </div>
  </div>
{/if}

<style>
  .ct-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }
  .ct-dialog {
    background: var(--bg-secondary, #1e1e1e);
    color: var(--text-primary, #e0e0e0);
    border-radius: 8px;
    width: 90vw;
    max-width: 820px;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }
  .ct-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 14px 20px;
    border-bottom: 1px solid var(--border-color, #333);
    flex: 0 0 auto;
  }
  .ct-header h2 { margin: 0; font-size: 1.1rem; font-weight: 600; }
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

  .ct-toolbar {
    display: flex;
    gap: 8px;
    padding: 10px 20px;
    border-bottom: 1px solid var(--border-color, #333);
    flex: 0 0 auto;
    align-items: center;
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

  .ct-body { flex: 1 1 auto; overflow-y: auto; min-height: 0; }

  .ct-form {
    padding: 14px 20px;
    border-bottom: 1px solid var(--border-color, #333);
    background: var(--bg-primary, #111);
  }
  .form-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 10px; }
  .form-header h3 { margin: 0; font-size: 0.95rem; font-weight: 600; }
  .btn-close-form {
    background: none; border: none; color: var(--text-secondary, #888);
    font-size: 1.2rem; line-height: 1; padding: 2px 6px; cursor: pointer; border-radius: 3px;
  }
  .btn-close-form:hover { background: rgba(255, 255, 255, 0.08); }
  .form-error {
    color: #ff6b6b; margin-bottom: 10px; font-size: 0.85rem;
    padding: 6px 10px; background: rgba(255, 107, 107, 0.1); border-radius: 4px;
  }
  .field { display: flex; flex-direction: column; gap: 4px; font-size: 0.8rem; color: var(--text-secondary, #aaa); margin-bottom: 10px; }
  .field span { font-weight: 500; }
  .field input, .field textarea {
    padding: 7px 10px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: var(--bg-secondary, #1e1e1e);
    color: var(--text-primary, #e0e0e0);
    font-size: 0.9rem;
    font-family: inherit;
  }
  .field textarea { resize: vertical; min-height: 80px; }
  .form-actions { display: flex; gap: 8px; }
  .btn-save {
    padding: 7px 18px; border-radius: 4px; border: none;
    background: var(--accent-color, #4a9eff); color: white; cursor: pointer; font-size: 0.9rem;
  }
  .btn-save:hover { filter: brightness(1.1); }
  .btn-cancel {
    padding: 7px 18px; border-radius: 4px;
    border: 1px solid var(--border-color, #444); background: transparent;
    color: var(--text-primary, #e0e0e0); cursor: pointer; font-size: 0.9rem;
  }
  .btn-cancel:hover { background: rgba(255, 255, 255, 0.05); }

  .ct-list-wrap { padding: 8px 20px 16px; }
  .loading-text, .empty-text { text-align: center; color: var(--text-secondary, #888); padding: 32px; font-size: 0.9rem; }
  .ct-table { width: 100%; border-collapse: collapse; font-size: 0.88rem; table-layout: fixed; }
  .ct-table th {
    text-align: left; padding: 8px; border-bottom: 1px solid var(--border-color, #333);
    color: var(--text-secondary, #888); font-weight: 500; font-size: 0.8rem;
    text-transform: uppercase; letter-spacing: 0.03em;
    position: sticky; top: 0; background: var(--bg-secondary, #1e1e1e); z-index: 1;
  }
  .ct-table td {
    padding: 8px; border-bottom: 1px solid var(--border-color, #222);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .ct-table tr:hover td { background: rgba(255, 255, 255, 0.03); }
  .name-cell { width: 200px; font-weight: 500; }
  .body-cell { color: var(--text-secondary, #bbb); }
  .truncate { max-width: 0; }
  .col-actions { width: 110px; }
  .actions { display: flex; gap: 4px; }
  .btn-edit, .btn-delete {
    padding: 3px 10px; border-radius: 3px; border: 1px solid var(--border-color, #444);
    background: transparent; color: var(--text-secondary, #bbb); cursor: pointer; font-size: 0.78rem;
  }
  .btn-edit:hover { background: rgba(255, 255, 255, 0.05); }
  .btn-delete { color: #ff6b6b; border-color: #ff6b6b44; }
  .btn-delete:hover { background: rgba(255, 107, 107, 0.08); }

  .ct-footer {
    padding: 10px 20px; border-top: 1px solid var(--border-color, #333);
    display: flex; justify-content: flex-end; align-items: center; flex: 0 0 auto;
  }
  .footer-count { font-size: 0.82rem; color: var(--text-secondary, #888); }
</style>
