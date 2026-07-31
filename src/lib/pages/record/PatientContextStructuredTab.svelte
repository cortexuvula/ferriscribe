<script lang="ts">
  import ConditionChips from '../../components/ConditionChips.svelte';

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
    conditionsText = next + sep + condition + '\n';
  }

  // Remove the exact-matching line (case-insensitive, trimmed) from the
  // textarea. The mirror of addCondition — invoked when an active chip is
  // clicked to toggle it off. Only the single matching line is removed;
  // hand-edited variants (e.g. "Type 2 diabetes (HbA1c 8.2)") are preserved.
  function removeCondition(condition: string) {
    const target = condition.trim().toLowerCase();
    if (target.length === 0) return;
    const kept = conditionsText
      .split('\n')
      .filter((l) => l.trim().toLowerCase() !== target);
    conditionsText = kept.join('\n');
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
  <ConditionChips
    onAdd={addCondition}
    onRemove={removeCondition}
    selectedConditions={conditionsText}
  />
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
    gap: 12px;
    padding: 8px 12px 12px;
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-top: 6px;
    margin-bottom: 2px;
    display: block;
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
    min-height: 72px;
  }

  .context-textarea:focus {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .context-textarea::placeholder {
    color: var(--text-muted);
  }
</style>
