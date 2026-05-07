<script lang="ts">
  import { upsertContextTemplate } from '../../api/contextTemplates';
  import { contextTemplates } from '../../stores/contextTemplates';
  import { formatError } from '../../types/errors';

  interface Props {
    contextText?: string;
    medicationsText?: string;
    allergiesText?: string;
    conditionsText?: string;
    contextCollapsed?: boolean;
  }
  let {
    contextText = $bindable(''),
    medicationsText = $bindable(''),
    allergiesText = $bindable(''),
    conditionsText = $bindable(''),
    contextCollapsed = $bindable(true),
  }: Props = $props();

  const hasActiveContext = $derived(
    contextText.trim().length > 0 ||
      medicationsText.trim().length > 0 ||
      allergiesText.trim().length > 0 ||
      conditionsText.trim().length > 0,
  );

  // Template picker state
  let selectedTemplate = $state('');

  // Save-as-template modal state
  let saveModalOpen = $state(false);
  let saveModalName = $state('');
  let saveModalError = $state('');
  let saveModalOverwriteConfirm = $state(false);

  function applyTemplate(name: string) {
    if (!name) return;
    const t = $contextTemplates.find((x) => x.name === name);
    if (!t) return;
    if (contextText.trim() === '') {
      contextText = t.body;
    } else {
      contextText = contextText.replace(/\s+$/, '') + '\n\n' + t.body;
    }
    // Reset dropdown so the same template can be applied again
    selectedTemplate = '';
    // Ensure the accordion is open so the user sees the inserted text
    contextCollapsed = false;
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
    const exists = $contextTemplates.some((t) => t.name === name);
    if (exists && !saveModalOverwriteConfirm) {
      saveModalOverwriteConfirm = true;
      saveModalError = `A template named "${name}" exists. Click Save again to overwrite.`;
      return;
    }
    try {
      await upsertContextTemplate(name, contextText);
      await contextTemplates.load();
      closeSaveModal();
    } catch (err: any) {
      saveModalError = formatError(err) || 'Failed to save template.';
    }
  }
</script>

<!-- Context Panel (collapsible, top) -->
<div class="context-panel" class:collapsed={contextCollapsed}>
  <button class="context-toggle" onclick={() => (contextCollapsed = !contextCollapsed)}>
    <span class="toggle-arrow">{contextCollapsed ? '▶' : '▼'}</span>
    Patient Context
    {#if hasActiveContext}
      <span class="context-badge">Active</span>
    {:else}
      <span class="context-hint">(optional)</span>
    {/if}
  </button>
  {#if !contextCollapsed}
    <label class="field-label" for="rt-medications">Medications (one per line)</label>
    <textarea
      id="rt-medications"
      class="context-textarea structured"
      placeholder="Lisinopril 10mg PO daily"
      bind:value={medicationsText}
      rows="3"
    ></textarea>

    <label class="field-label" for="rt-allergies">Allergies (one per line)</label>
    <textarea
      id="rt-allergies"
      class="context-textarea structured"
      placeholder="Penicillin (rash)"
      bind:value={allergiesText}
      rows="2"
    ></textarea>

    <label class="field-label" for="rt-conditions">Known conditions (one per line)</label>
    <textarea
      id="rt-conditions"
      class="context-textarea structured"
      placeholder="Type 2 diabetes"
      bind:value={conditionsText}
      rows="3"
    ></textarea>

    <label class="field-label" for="rt-notes">Notes</label>
    <div class="template-toolbar">
      <select
        class="template-picker"
        bind:value={selectedTemplate}
        onchange={() => applyTemplate(selectedTemplate)}
        disabled={$contextTemplates.length === 0}
      >
        <option value="">
          {$contextTemplates.length === 0 ? 'No templates saved' : 'Apply template…'}
        </option>
        {#each $contextTemplates as t (t.name)}
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
      class="context-textarea"
      placeholder="Paste chart notes, medications, history..."
      bind:value={contextText}
      rows="5"
    ></textarea>
  {/if}
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
  /* Context Panel */
  .context-panel {
    background-color: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .context-toggle {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    text-align: left;
    cursor: pointer;
    background: none;
    border: none;
  }

  .context-toggle:hover {
    color: var(--text-primary);
  }

  .toggle-arrow {
    font-size: 10px;
    width: 12px;
    text-align: center;
  }

  .context-hint {
    font-weight: 400;
    color: var(--text-muted);
    font-size: 12px;
  }

  .context-textarea {
    display: block;
    width: 100%;
    padding: 8px 16px 12px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: none;
    border-top: 1px solid var(--border-light, var(--border));
    resize: vertical;
    min-height: 80px;
    max-height: 200px;
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-top: 8px;
    margin-bottom: 4px;
    display: block;
  }

  .context-textarea.structured {
    min-height: 56px;
  }

  .context-badge {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--accent);
    background-color: color-mix(in srgb, var(--accent) 15%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
    border-radius: var(--radius-sm);
    padding: 1px 6px;
    margin-left: 6px;
  }

  .context-textarea:focus {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .context-textarea::placeholder {
    color: var(--text-muted);
  }

  .template-toolbar {
    display: flex;
    gap: 8px;
    padding: 8px 16px 0;
    align-items: center;
  }
  .template-picker {
    flex: 1 1 auto;
    min-width: 0;
    padding: 6px 10px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: var(--bg-primary, #111);
    color: var(--text-primary, #e0e0e0);
    font-size: 0.88rem;
  }
  .template-picker:disabled { opacity: 0.6; cursor: not-allowed; }
  .btn-save-template {
    flex: 0 0 auto;
    padding: 6px 14px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: transparent;
    color: var(--text-primary, #e0e0e0);
    cursor: pointer;
    font-size: 0.88rem;
    white-space: nowrap;
  }
  .btn-save-template:hover:not(:disabled) { background: rgba(255, 255, 255, 0.05); }
  .btn-save-template:disabled { opacity: 0.4; cursor: not-allowed; }

  .save-modal-overlay {
    position: fixed; inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex; align-items: center; justify-content: center;
    z-index: 1000;
  }
  .save-modal {
    background: var(--bg-secondary, #1e1e1e);
    color: var(--text-primary, #e0e0e0);
    border-radius: 8px;
    width: 90vw; max-width: 520px; max-height: 85vh;
    display: flex; flex-direction: column;
    padding: 20px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }
  .save-modal-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px; }
  .save-modal-header h3 { margin: 0; font-size: 1.05rem; }
  .save-modal .btn-close {
    background: none; border: none; color: var(--text-secondary, #aaa);
    font-size: 1.4rem; line-height: 1; padding: 4px 8px; cursor: pointer; border-radius: 4px;
  }
  .save-modal .btn-close:hover { background: rgba(255, 255, 255, 0.08); }
  .save-modal-error {
    color: #ff6b6b; margin-bottom: 10px; font-size: 0.85rem;
    padding: 6px 10px; background: rgba(255, 107, 107, 0.1); border-radius: 4px;
  }
  .save-modal-field { display: flex; flex-direction: column; gap: 4px; font-size: 0.85rem; color: var(--text-secondary, #aaa); margin-bottom: 10px; }
  .save-modal-field span { font-weight: 500; }
  .save-modal-field input {
    padding: 7px 10px; border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: var(--bg-primary, #111);
    color: var(--text-primary, #e0e0e0); font-size: 0.9rem;
  }
  .save-modal-preview {
    background: var(--bg-primary, #111); padding: 10px; border-radius: 4px;
    border: 1px solid var(--border-color, #333); max-height: 180px; overflow-y: auto;
    white-space: pre-wrap; font-size: 0.85rem; margin: 0; font-family: inherit;
  }
  .save-modal-actions { display: flex; gap: 8px; margin-top: 8px; }
  .save-modal .btn-save {
    padding: 7px 18px; border-radius: 4px; border: none;
    background: var(--accent-color, #4a9eff); color: white; cursor: pointer; font-size: 0.9rem;
  }
  .save-modal .btn-save:hover { filter: brightness(1.1); }
  .save-modal .btn-cancel {
    padding: 7px 18px; border-radius: 4px;
    border: 1px solid var(--border-color, #444); background: transparent;
    color: var(--text-primary, #e0e0e0); cursor: pointer; font-size: 0.9rem;
  }
</style>
