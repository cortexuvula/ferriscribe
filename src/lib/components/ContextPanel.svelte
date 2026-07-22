<script lang="ts">
  import { contextTemplates } from '../stores/contextTemplates.svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import ConditionChips from './ConditionChips.svelte';

  /** Status of a single OCR-processed document chip. */
  interface OcrFileStatus {
    id: string;
    filename: string;
    status: 'done' | 'loading' | 'error';
    pageCount: number;
  }

  interface Props {
    medicationsText: string;
    allergiesText: string;
    conditionsText: string;
    contextText: string;
    expanded: boolean;
    hasActiveContext: boolean;
    onToggle: () => void;
    onInsertTemplate: (text: string) => void;
    onClearContext: () => void;
    onMedicationsChange: (value: string) => void;
    onAllergiesChange: (value: string) => void;
    onConditionsChange: (value: string) => void;
    onContextChange: (value: string) => void;
    ocrFiles: OcrFileStatus[];
    ocrText: string;
    ocrLoading: boolean;
    onOcrFilesSelected: (paths: string[]) => void;
    onOcrTextChange: (text: string) => void;
    onRemoveOcrFile: (id: string) => void;
  }

  const {
    medicationsText,
    allergiesText,
    conditionsText,
    contextText,
    expanded,
    hasActiveContext,
    onToggle,
    onInsertTemplate,
    onClearContext,
    onMedicationsChange,
    onAllergiesChange,
    onConditionsChange,
    onContextChange,
    ocrFiles,
    ocrText,
    ocrLoading,
    onOcrFilesSelected,
    onOcrTextChange,
    onRemoveOcrFile,
  }: Props = $props();

  let isDragging = $state(false);

  async function handleBrowse() {
    const selected = await open({
      multiple: true,
      filters: [
        {
          name: 'Documents',
          extensions: ['pdf', 'png', 'jpg', 'jpeg', 'bmp', 'webp', 'txt', 'md', 'csv'],
        },
      ],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    onOcrFilesSelected(paths);
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    isDragging = true;
  }

  function handleDragLeave() {
    isDragging = false;
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    isDragging = false;
    const files = e.dataTransfer?.files;
    if (files && files.length > 0) {
      const paths: string[] = [];
      for (let i = 0; i < files.length; i++) {
        const f = files[i] as unknown as { path?: string };
        if (f.path) paths.push(f.path);
      }
      if (paths.length > 0) {
        onOcrFilesSelected(paths);
      }
    }
  }

  const CONTEXT_TEMPLATES = [
    { label: 'Follow-up', text: 'Follow-up visit for ongoing condition. Previous visit findings:\n\n' },
    { label: 'New Patient', text: 'New patient consultation. No prior history available.\n\n' },
    { label: 'Lab Results', text: 'Recent lab results:\n- \n- \n- \n\n' },
    { label: 'Referral Info', text: 'Referred by: \nReason for referral: \nRelevant history: \n\n' },
  ];

  // Append a condition to the textarea (called by the shared ConditionChips
  // component). No-op if already present (case-insensitive line match).
  function addCondition(condition: string) {
    const existing = conditionsText
      .split('\n')
      .map((l) => l.trim().toLowerCase())
      .filter((l) => l.length > 0);
    if (existing.includes(condition.toLowerCase())) return;
    const next = conditionsText.trimEnd();
    const sep = next.length > 0 && !next.endsWith('\n') ? '\n' : '';
    onConditionsChange(next + sep + condition + '\n');
  }

  // Refresh saved templates when the panel is first expanded, so newly-created
  // templates (from Settings or the Record tab) appear without a manual reload.
  let lastLoadedExpanded = false;
  $effect(() => {
    if (expanded && !lastLoadedExpanded) {
      contextTemplates.load();
    }
    lastLoadedExpanded = expanded;
  });
</script>

<div class="context-panel" class:expanded>
  <button class="context-toggle" onclick={onToggle}>
    <span class="toggle-arrow">{expanded ? '▾' : '▸'}</span>
    <span class="toggle-label">Additional Context</span>
    {#if hasActiveContext}
      <span class="context-badge">Active</span>
    {/if}
  </button>

  {#if expanded}
    <div class="context-body">
      <p class="context-hint">
        Add medications, allergies, and known conditions as structured lists below. Use the Notes textarea for everything else (lab values, prior visit narrative, family/social history, etc.).
      </p>

      <label class="field-label" for="ctx-medications">Medications (one per line)</label>
      <textarea
        id="ctx-medications"
        class="context-textarea structured"
        placeholder="Lisinopril 10mg PO daily"
        value={medicationsText}
        oninput={(e) => onMedicationsChange(e.currentTarget.value)}
        rows="3"
      ></textarea>

      <label class="field-label" for="ctx-allergies">Allergies (one per line)</label>
      <textarea
        id="ctx-allergies"
        class="context-textarea structured"
        placeholder="Penicillin (rash)"
        value={allergiesText}
        oninput={(e) => onAllergiesChange(e.currentTarget.value)}
        rows="2"
      ></textarea>

      <label class="field-label" for="ctx-conditions">Known conditions (one per line)</label>
      <ConditionChips onAdd={addCondition} />
      <textarea
        id="ctx-conditions"
        class="context-textarea structured"
        placeholder="Type 2 diabetes"
        value={conditionsText}
        oninput={(e) => onConditionsChange(e.currentTarget.value)}
        rows="3"
      ></textarea>

      <label class="field-label" for="ctx-notes">Notes</label>
      <div class="context-templates">
        {#each CONTEXT_TEMPLATES as tmpl}
          <button class="template-chip" onclick={() => onInsertTemplate(tmpl.text)}>
            {tmpl.label}
          </button>
        {/each}
        {#if contextTemplates.list.length > 0}
          <span class="template-divider" aria-hidden="true"></span>
          {#each contextTemplates.list as tmpl (tmpl.name)}
            <button
              class="template-chip saved"
              title={tmpl.body}
              onclick={() => onInsertTemplate(tmpl.body)}
            >
              {tmpl.name}
            </button>
          {/each}
        {/if}
      </div>
      <textarea
        id="ctx-notes"
        class="context-textarea"
        placeholder="Free-form notes (lab values, prior visit narrative, family/social history)..."
        value={contextText}
        oninput={(e) => onContextChange(e.currentTarget.value)}
        rows="6"
      ></textarea>
      {#if contextText.trim()}
        <button class="context-clear" onclick={onClearContext}>
          Clear notes
        </button>
      {/if}

      <!-- OCR Drop Zone -->
      <div class="ocr-section">
        <div
          class="dropzone"
          class:dragging={isDragging}
          ondragover={handleDragOver}
          ondragleave={handleDragLeave}
          ondrop={handleDrop}
          onclick={handleBrowse}
          role="button"
          tabindex="0"
          onkeydown={(e) => { if (e.key === 'Enter') handleBrowse(); }}
        >
          <span class="dropzone-icon">📎</span>
          <span class="dropzone-text">Drop documents here</span>
          <span class="dropzone-hint">or click to browse — PDF, PNG, JPG, TXT</span>
        </div>

        {#if ocrFiles.length > 0}
          <div class="ocr-files">
            {#each ocrFiles as file (file.id)}
              <span class="ocr-file-chip" class:chip-error={file.status === 'error'}>
                <span class="chip-name">{file.filename}</span>
                {#if file.status === 'done'}
                  <span class="chip-status">✓ {file.pageCount}p</span>
                {:else if file.status === 'loading'}
                  <span class="chip-status">⏳</span>
                {:else}
                  <span class="chip-status">⚠</span>
                {/if}
                <button
                  class="chip-remove"
                  onclick={(e) => { e.stopPropagation(); onRemoveOcrFile(file.id); }}
                  aria-label="Remove file"
                >×</button>
              </span>
            {/each}
          </div>
        {/if}

        {#if ocrLoading}
          <div class="ocr-status">Extracting text…</div>
        {/if}

        {#if ocrText || ocrLoading}
          <details class="ocr-preview-details">
            <summary>Preview extracted text (editable)</summary>
            <textarea
              class="ocr-preview"
              placeholder="Extracted text will appear here…"
              value={ocrText}
              oninput={(e) => onOcrTextChange((e.currentTarget as HTMLTextAreaElement).value)}
              rows="6"
            ></textarea>
          </details>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .context-panel {
    margin-bottom: 16px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background-color: var(--bg-card);
    overflow: hidden;
  }

  .context-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 14px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    transition: color 0.15s ease;
  }

  .context-toggle:hover {
    color: var(--text-primary);
  }

  .toggle-arrow {
    font-size: 11px;
    color: var(--text-muted);
  }

  .toggle-label {
    flex: 1;
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
  }

  .context-body {
    padding: 0 14px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .context-hint {
    font-size: 12px;
    color: var(--text-muted);
    line-height: 1.5;
    margin: 0;
  }

  .context-templates {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .template-chip {
    padding: 4px 10px;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: 12px;
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease, border-color 0.15s ease;
  }

  .template-chip:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .template-divider {
    width: 1px;
    align-self: stretch;
    margin: 2px 2px;
    background-color: var(--border);
  }

  .template-chip.saved {
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 40%, transparent);
    background-color: color-mix(in srgb, var(--accent) 8%, transparent);
  }

  .template-chip.saved:hover {
    background-color: color-mix(in srgb, var(--accent) 18%, transparent);
  }

  .context-textarea {
    width: 100%;
    resize: vertical;
    min-height: 80px;
    padding: 10px;
    font-size: 13px;
    font-family: inherit;
    line-height: 1.5;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    transition: border-color 0.15s ease;
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-top: 4px;
    margin-bottom: -4px;
  }

  .context-textarea.structured {
    min-height: 56px;
  }

  .context-textarea::placeholder {
    color: var(--text-muted);
  }

  .context-textarea:focus {
    outline: none;
    border-color: var(--accent);
  }

  .context-clear {
    align-self: flex-end;
    padding: 4px 10px;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-muted);
    background: none;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: color 0.15s ease, border-color 0.15s ease;
  }

  .context-clear:hover {
    color: var(--danger, #ef4444);
    border-color: var(--danger, #ef4444);
  }

  .ocr-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 4px;
    border-top: 1px solid var(--border);
    margin-top: 4px;
  }

  .dropzone {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    padding: 20px;
    border: 2px dashed var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
    transition: border-color 0.15s ease, background-color 0.15s ease;
    text-align: center;
  }

  .dropzone:hover,
  .dropzone.dragging {
    border-color: var(--accent);
    background-color: var(--bg-hover);
  }

  .dropzone.dragging {
    border-style: solid;
  }

  .dropzone-icon {
    font-size: 24px;
  }

  .dropzone-text {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
  }

  .dropzone-hint {
    font-size: 11px;
    color: var(--text-muted);
  }

  .ocr-files {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .ocr-file-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 8px;
    border-radius: var(--radius-sm);
    background-color: var(--bg-hover);
    font-size: 12px;
    color: var(--text-secondary);
  }

  .ocr-file-chip.chip-error {
    background-color: rgba(239, 68, 68, 0.1);
    color: var(--danger, #ef4444);
  }

  .chip-remove {
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0 2px;
    opacity: 0.6;
  }

  .chip-remove:hover {
    opacity: 1;
  }

  .ocr-status {
    font-size: 12px;
    color: var(--text-muted);
    font-style: italic;
  }

  .ocr-preview-details summary {
    cursor: pointer;
    font-size: 12px;
    color: var(--text-muted);
    margin-bottom: 6px;
  }

  .ocr-preview {
    width: 100%;
    font-size: 13px;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background-color: var(--bg-primary);
    color: var(--text-primary);
    resize: vertical;
    font-family: inherit;
  }
</style>
