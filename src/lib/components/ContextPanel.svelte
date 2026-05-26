<script lang="ts">
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
  }

  let {
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
  }: Props = $props();

  const CONTEXT_TEMPLATES = [
    { label: 'Follow-up', text: 'Follow-up visit for ongoing condition. Previous visit findings:\n\n' },
    { label: 'New Patient', text: 'New patient consultation. No prior history available.\n\n' },
    { label: 'Lab Results', text: 'Recent lab results:\n- \n- \n- \n\n' },
    { label: 'Referral Info', text: 'Referred by: \nReason for referral: \nRelevant history: \n\n' },
  ];
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
</style>
