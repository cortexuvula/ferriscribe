<script lang="ts">
  import { upsertContextTemplate } from '../../api/contextTemplates';
  import { contextTemplates } from '../../stores/contextTemplates.svelte';
  import { formatError } from '../../types/errors';

  type Props = {
    contextText: string;
  };
  let { contextText = $bindable('') }: Props = $props();

  let selectedTemplate = $state('');
  let saveModalOpen = $state(false);
  let saveModalName = $state('');
  let saveModalError = $state('');
  let saveModalOverwriteConfirm = $state(false);

  function applyTemplate(name: string) {
    if (!name) return;
    const t = contextTemplates.list.find((x) => x.name === name);
    if (!t) return;
    if (contextText.trim() === '') {
      contextText = t.body;
    } else {
      contextText = contextText.replace(/\s+$/, '') + '\n\n' + t.body;
    }
    selectedTemplate = '';
  }

  function openSaveModal() {
    if (contextText.trim() === '') return;
    saveModalName = '';
    saveModalError = '';
    saveModalOverwriteConfirm = false;
    saveModalOpen = true;
  }

  function closeSaveModal() {
    saveModalOpen = false;
    saveModalError = '';
    saveModalOverwriteConfirm = false;
  }

  async function confirmSaveTemplate() {
    const name = saveModalName.trim();
    if (!name) {
      saveModalError = 'Name is required.';
      return;
    }
    const exists = contextTemplates.list.some((t) => t.name === name);
    if (exists && !saveModalOverwriteConfirm) {
      saveModalOverwriteConfirm = true;
      saveModalError = `A template named "${name}" exists. Click Save again to overwrite.`;
      return;
    }
    try {
      await upsertContextTemplate(name, contextText);
      await contextTemplates.load();
      closeSaveModal();
    } catch (err) {
      saveModalError = formatError(err) || 'Failed to save template.';
    }
  }
</script>

<div class="notes-tab">
  <div class="template-toolbar">
    <select
      class="template-picker"
      bind:value={selectedTemplate}
      onchange={() => applyTemplate(selectedTemplate)}
      disabled={contextTemplates.list.length === 0}
    >
      <option value="">
        {contextTemplates.list.length === 0 ? 'No templates saved' : 'Apply template…'}
      </option>
      {#each contextTemplates.list as t (t.name)}
        <option value={t.name}>{t.name}</option>
      {/each}
    </select>
    <button
      class="btn-save-template"
      onclick={openSaveModal}
      disabled={contextText.trim() === ''}
      title={contextText.trim() === '' ? 'Type something first' : 'Save current text as a new template'}
    >
      Save as template
    </button>
  </div>
  <textarea
    id="rt-notes"
    class="notes-textarea"
    placeholder="Paste chart notes, medications, history..."
    bind:value={contextText}
    rows="12"
  ></textarea>
</div>

{#if saveModalOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="save-modal-overlay" onclick={closeSaveModal}>
    <div class="save-modal" onclick={(e) => e.stopPropagation()}>
      <div class="save-modal-header">
        <h3>Save as Template</h3>
        <button class="btn-close" aria-label="Close" onclick={closeSaveModal}>&times;</button>
      </div>
      {#if saveModalError}
        <div class="save-modal-error">{saveModalError}</div>
      {/if}
      <label class="save-modal-field">
        <span>Name</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input type="text" bind:value={saveModalName} placeholder="e.g. Follow-up visit" autofocus />
      </label>
      <div class="save-modal-field">
        <span>Preview</span>
        <pre class="save-modal-preview">{contextText}</pre>
      </div>
      <div class="save-modal-actions">
        <button class="btn-save" onclick={confirmSaveTemplate}>
          {saveModalOverwriteConfirm ? 'Overwrite' : 'Save'}
        </button>
        <button class="btn-cancel" onclick={closeSaveModal}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .notes-tab {
    display: flex;
    flex-direction: column;
    padding: 8px 12px 12px;
    flex: 1;
    min-height: 0;
  }

  .template-toolbar {
    display: flex;
    gap: 8px;
    margin-bottom: 8px;
    align-items: center;
  }

  .template-picker {
    flex: 1 1 auto;
    min-width: 0;
    padding: 6px 10px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 0.88rem;
  }

  .template-picker:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-save-template {
    flex: 0 0 auto;
    padding: 6px 14px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    font-size: 0.88rem;
    white-space: nowrap;
  }

  .btn-save-template:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .btn-save-template:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .notes-textarea {
    flex: 1;
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: 4px;
    resize: vertical;
    min-height: 200px;
  }

  .notes-textarea:focus {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .notes-textarea::placeholder {
    color: var(--text-muted);
  }

  .save-modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .save-modal {
    background: var(--bg-secondary);
    color: var(--text-primary);
    border-radius: 8px;
    width: 90vw;
    max-width: 520px;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    padding: 20px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }

  .save-modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 12px;
  }

  .save-modal-header h3 {
    margin: 0;
    font-size: 1.05rem;
  }

  .save-modal .btn-close {
    background: none;
    border: none;
    color: var(--text-secondary);
    font-size: 1.4rem;
    line-height: 1;
    padding: 4px 8px;
    cursor: pointer;
    border-radius: 4px;
  }

  .save-modal .btn-close:hover {
    background: var(--bg-hover);
  }

  .save-modal-error {
    color: #ff6b6b;
    margin-bottom: 10px;
    font-size: 0.85rem;
    padding: 6px 10px;
    background: rgba(255, 107, 107, 0.1);
    border-radius: 4px;
  }

  .save-modal-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 0.85rem;
    color: var(--text-secondary);
    margin-bottom: 10px;
  }

  .save-modal-field span {
    font-weight: 500;
  }

  .save-modal-field input {
    padding: 7px 10px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 0.9rem;
  }

  .save-modal-preview {
    background: var(--bg-primary);
    padding: 10px;
    border-radius: 4px;
    border: 1px solid var(--border);
    max-height: 180px;
    overflow-y: auto;
    white-space: pre-wrap;
    font-size: 0.85rem;
    margin: 0;
    font-family: inherit;
  }

  .save-modal-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }

  .save-modal .btn-save {
    padding: 7px 18px;
    border-radius: 4px;
    border: none;
    background: var(--accent);
    color: white;
    cursor: pointer;
    font-size: 0.9rem;
  }

  .save-modal .btn-save:hover {
    filter: brightness(1.1);
  }

  .save-modal .btn-cancel {
    padding: 7px 18px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    font-size: 0.9rem;
  }
</style>
