<script lang="ts">
  import { onMount } from 'svelte';
  import { addConditionChip, listConditionChips, removeConditionChip } from '../api/conditions';

  let { onAdd }: { onAdd: (condition: string) => void } = $props();

  // The default list shown while the backend list is loading or when it's
  // empty (fresh install / backend default). Once the backend returns a
  // non-empty list, those values take precedence.
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

  let chips = $state<string[]>([]);
  let loaded = $state(false);
  let adding = $state(false);
  let newCondition = $state('');

  // Display defaults until the backend list loads (or if it's empty).
  let conditions = $derived(loaded && chips.length > 0 ? chips : DEFAULT_CONDITIONS);

  onMount(async () => {
    try {
      chips = (await listConditionChips()).map((c) => c.text);
    } catch (e) {
      console.error('Failed to load condition chips:', e);
    }
    loaded = true;
  });

  async function addNewCondition() {
    const trimmed = newCondition.trim();
    if (!trimmed) return;
    // Dedup check (case-insensitive).
    if (conditions.some((c) => c.toLowerCase() === trimmed.toLowerCase())) {
      newCondition = '';
      adding = false;
      return;
    }
    try {
      const updated = await addConditionChip(trimmed);
      chips = updated.map((c) => c.text);
    } catch (e) {
      console.error('Failed to add condition chip:', e);
    }
    newCondition = '';
    adding = false;
  }

  async function removeCondition(condition: string) {
    try {
      const updated = await removeConditionChip(condition);
      chips = updated.map((c) => c.text);
    } catch (e) {
      console.error('Failed to remove condition chip:', e);
    }
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
    /* Guarantee the chip row can wrap fully without being clipped by a
       flex parent that has a fixed/min width. */
    width: 100%;
    box-sizing: border-box;
  }

  /* The pill background lives on the wrapper so the chip always looks like a
     complete capsule — even when the remove button's × is hidden. Previously
     the background was split across two buttons and the invisible remove
     button left a flat-edged gap, making every chip look "cut off". */
  .condition-chip-wrapper {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    border-radius: 12px;
    background-color: color-mix(in srgb, var(--success, #22c55e) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--success, #22c55e) 30%, transparent);
    overflow: hidden;
  }

  .condition-chip-wrapper:hover {
    background-color: color-mix(in srgb, var(--success, #22c55e) 18%, transparent);
    border-color: color-mix(in srgb, var(--success, #22c55e) 45%, transparent);
  }

  .condition-chip {
    /* Label fills the available width so long condition names wrap/truncate
       cleanly instead of pushing the × off the edge of the sidebar. */
    min-width: 0;
    padding: 3px 8px;
    font-size: 11px;
    font-weight: 500;
    line-height: 1.4;
    color: var(--success, #22c55e);
    background: none;
    border: none;
    cursor: pointer;
    transition: color 0.15s ease;
  }

  /* The × is the only thing that fades — the pill itself stays intact. It
    starts faintly visible so the chip reads as removable even without hover. */
  .chip-remove {
    flex: 0 0 auto;
    padding: 3px 7px 3px 4px;
    font-size: 13px;
    line-height: 1;
    color: var(--success, #22c55e);
    background: none;
    border: none;
    cursor: pointer;
    opacity: 0.45;
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
