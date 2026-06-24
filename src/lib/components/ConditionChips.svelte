<script lang="ts">
  import { settings } from '../stores/settings.svelte';

  interface Props {
    /// Called when the user clicks a chip to add it to the conditions textarea.
    onAdd: (condition: string) => void;
  }
  let { onAdd }: Props = $props();

  // The default list shown when custom_conditions is empty (fresh install or
  // backend default). If the backend returns values, those take precedence.
  const DEFAULT_CONDITIONS = [
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

  // Use the user's custom list if populated, otherwise fall back to defaults.
  const conditions = $derived(
    settings.state.custom_conditions.length > 0
      ? settings.state.custom_conditions
      : DEFAULT_CONDITIONS
  );

  let adding = $state(false);
  let newCondition = $state('');

  async function persistConditions(list: string[]) {
    await settings.updateField('custom_conditions', list);
  }

  async function addNewCondition() {
    const trimmed = newCondition.trim();
    if (!trimmed) {
      adding = false;
      return;
    }
    // Dedup: don't add if already present (case-insensitive).
    const exists = conditions.some(
      (c) => c.toLowerCase() === trimmed.toLowerCase()
    );
    if (!exists) {
      const next = [...conditions, trimmed];
      await persistConditions(next);
    }
    newCondition = '';
    adding = false;
  }

  async function removeCondition(condition: string) {
    const next = conditions.filter((c) => c !== condition);
    await persistConditions(next);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      addNewCondition();
    } else if (e.key === 'Escape') {
      adding = false;
      newCondition = '';
    }
  }
</script>

<div class="condition-chips" role="group" aria-label="Common conditions quick-add">
  {#each conditions as condition (condition)}
    <div class="condition-chip-wrapper">
      <button
        class="condition-chip"
        type="button"
        onclick={() => onAdd(condition)}
        title={`Add "${condition}" to the list`}
      >
        {condition}
      </button>
      <button
        class="chip-remove"
        type="button"
        onclick={() => removeCondition(condition)}
        title={`Remove "${condition}" from chips`}
        aria-label="Remove {condition}"
      >
        ×
      </button>
    </div>
  {/each}
  {#if adding}
    <input
      class="chip-input"
      type="text"
      bind:value={newCondition}
      onkeydown={handleKeydown}
      onblur={addNewCondition}
      placeholder="Condition name…"
      maxlength="60"
    />
  {:else}
    <button
      class="chip-add"
      type="button"
      onclick={() => { adding = true; newCondition = ''; }}
      title="Add a new condition chip"
    >
      +
    </button>
  {/if}
</div>

<style>
  .condition-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin-bottom: 6px;
  }

  .condition-chip-wrapper {
    display: inline-flex;
    align-items: stretch;
    border-radius: 12px;
    overflow: hidden;
  }

  .condition-chip {
    padding: 3px 9px;
    font-size: 11px;
    font-weight: 500;
    color: var(--success, #22c55e);
    background-color: color-mix(in srgb, var(--success, #22c55e) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--success, #22c55e) 30%, transparent);
    border-right: none;
    border-radius: 12px 0 0 12px;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .condition-chip:hover {
    background-color: color-mix(in srgb, var(--success, #22c55e) 20%, transparent);
  }

  .chip-remove {
    padding: 3px 6px;
    font-size: 12px;
    line-height: 1;
    color: var(--text-muted);
    background-color: color-mix(in srgb, var(--success, #22c55e) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--success, #22c55e) 30%, transparent);
    border-radius: 0 12px 12px 0;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s ease, color 0.15s ease;
  }

  .condition-chip-wrapper:hover .chip-remove {
    opacity: 1;
  }

  .chip-remove:hover {
    color: var(--danger, #ef4444);
  }

  .chip-add {
    padding: 3px 12px;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-muted);
    background: none;
    border: 1px dashed var(--border, #444);
    border-radius: 12px;
    cursor: pointer;
    transition: color 0.15s ease, border-color 0.15s ease;
  }

  .chip-add:hover {
    color: var(--success, #22c55e);
    border-color: var(--success, #22c55e);
  }

  .chip-input {
    padding: 3px 9px;
    font-size: 11px;
    color: var(--text-primary);
    background-color: var(--bg-primary, #1a1a1a);
    border: 1px solid var(--success, #22c55e);
    border-radius: 12px;
    width: 140px;
    box-sizing: border-box;
  }

  .chip-input:focus {
    outline: none;
  }
</style>
