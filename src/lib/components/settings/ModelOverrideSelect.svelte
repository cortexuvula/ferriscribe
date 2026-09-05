<script lang="ts">
  /**
   * A per-feature model-override select ("(use generation model)" sentinel
   * + the active provider's models). Backed by the settings store: the
   * chosen id persists to `field`, the sentinel persists as null.
   */
  import { settings } from '../../stores/settings.svelte';
  import type { ModelInfo } from '../../api/chat';
  import type { FeatureModelField } from '../../utils/modelOverrides';

  let {
    id,
    label,
    field,
    hint,
    models,
  }: {
    /** DOM id — the <label for> target (must be unique in the pane). */
    id: string;
    label: string;
    field: FeatureModelField;
    /** Explanatory copy under the select. */
    hint: string;
    models: ModelInfo[];
  } = $props();

  async function handleChange(e: Event) {
    const val = (e.currentTarget as HTMLSelectElement).value;
    await settings.updateField(field, val || null);
  }
</script>

<div class="form-group">
  <label for={id} class="form-label">{label}</label>
  <div class="model-select-row">
    <select id={id} value={settings.state[field] ?? ''} onchange={handleChange}>
      <option value="">(use generation model)</option>
      {#each models as m (m.id)}
        <option value={m.id}>{m.name}</option>
      {/each}
    </select>
  </div>
  <p class="form-hint">{hint}</p>
</div>

<!-- Scoped copies of the Models.svelte select styles this markup used to
     live under — Svelte scopes per component, so sharing isn't possible
     without going global. Keep them in sync with Models.svelte. -->
<style>
  .model-select-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .model-select-row select {
    flex: 1;
  }

  .form-hint {
    font-size: 11px;
    color: var(--text-muted);
    margin: 4px 0 0;
    line-height: 1.5;
  }
</style>
