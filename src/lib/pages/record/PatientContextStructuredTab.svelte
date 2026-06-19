<script lang="ts">
  type Props = {
    medicationsText: string;
    allergiesText: string;
    conditionsText: string;
  };
  let {
    medicationsText = $bindable(''),
    allergiesText = $bindable(''),
    conditionsText = $bindable(''),
  }: Props = $props();

  // Common chronic conditions for one-click add. Clicking a chip appends the
  // condition as a new line (no-op if already present, matched
  // case-insensitively against existing lines so the user can't double-add).
  const COMMON_CONDITIONS = [
    'Hypertension',
    'Type 2 diabetes',
    'Hyperlipidemia',
    'Asthma',
    'COPD',
    'Hypothyroidism',
    'Atrial fibrillation',
    'Coronary artery disease',
    'CKD (chronic kidney disease)',
    'GERD',
    'Anxiety',
    'Depression',
    'Osteoarthritis',
    'Obesity',
    'Sleep apnea',
  ];

  function addCondition(condition: string) {
    const existing = conditionsText
      .split('\n')
      .map((l) => l.trim().toLowerCase())
      .filter((l) => l.length > 0);
    if (existing.includes(condition.toLowerCase())) return;
    const next = conditionsText.trimEnd();
    const sep = next.length > 0 && !next.endsWith('\n') ? '\n' : '';
    conditionsText = next + sep + condition + '\n';
  }
</script>

<div class="structured-fields">
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
  <div class="condition-chips" role="group" aria-label="Common conditions quick-add">
    {#each COMMON_CONDITIONS as condition}
      <button
        class="condition-chip"
        type="button"
        onclick={() => addCondition(condition)}
        title={`Add "${condition}" to the list`}
      >
        {condition}
      </button>
    {/each}
  </div>
  <textarea
    id="rt-conditions"
    class="context-textarea structured"
    placeholder="Type 2 diabetes"
    bind:value={conditionsText}
    rows="3"
  ></textarea>
</div>

<style>
  .structured-fields {
    display: flex;
    flex-direction: column;
    padding: 8px 12px 12px;
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

  .condition-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin-bottom: 6px;
  }

  .condition-chip {
    padding: 3px 9px;
    font-size: 11px;
    font-weight: 500;
    color: var(--success, #22c55e);
    background-color: color-mix(in srgb, var(--success, #22c55e) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--success, #22c55e) 30%, transparent);
    border-radius: 12px;
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease, border-color 0.15s ease;
  }

  .condition-chip:hover {
    background-color: color-mix(in srgb, var(--success, #22c55e) 20%, transparent);
    border-color: var(--success, #22c55e);
  }

  .context-textarea {
    display: block;
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: 4px;
    resize: vertical;
    min-height: 80px;
    max-height: 200px;
  }

  .context-textarea.structured {
    min-height: 56px;
  }

  .context-textarea:focus {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .context-textarea::placeholder {
    color: var(--text-muted);
  }
</style>
