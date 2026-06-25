<script lang="ts">
  import { untrack } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import { formatError } from '../../../types/errors';

  type Props = {
    onclose: () => void;
    onsuccess: (corpusDir: string, pairs: number, warnings: number) => void;
    promotedCount: number;
    availableModels: string[]; // distinct ai_model values seen in promoted rows
  };
  const { onclose, onsuccess, promotedCount, availableModels }: Props = $props();

  let outputDir = $state<string | null>(null);
  // Pre-select the first available model. The intent is initial-only — once
  // the user toggles checkboxes, selectedModels diverges from availableModels.
  // untrack() makes that explicit and silences `state_referenced_locally`.
  let selectedModels = $state<string[]>(untrack(() => availableModels.slice(0, 1)));
  let strictness: 'standard' | 'aggressive' = $state('standard');
  let exporting = $state(false);
  let error: string | null = $state(null);

  async function pickDirectory() {
    try {
      const selected = await openDialog({ directory: true, multiple: false });
      if (typeof selected === 'string') outputDir = selected;
    } catch (e) {
      error = String(e);
    }
  }

  async function runExport() {
    if (!outputDir) {
      error = 'Choose an output directory first.';
      return;
    }
    exporting = true;
    error = null;
    try {
      const resp = await invoke<{
        corpus_dir: string;
        pairs_written: number;
        warning_count: number;
      }>('training_corpus_export', {
        req: {
          output_dir: outputDir,
          base_model_filter: selectedModels,
          redaction_strictness: strictness,
        },
      });
      onsuccess(resp.corpus_dir, resp.pairs_written, resp.warning_count);
    } catch (e) {
      error = formatError(e);
    } finally {
      exporting = false;
    }
  }
</script>

<div
  class="modal-backdrop"
  role="presentation"
  onclick={onclose}
>
  <div
    class="modal"
    role="dialog"
    aria-modal="true"
    tabindex="0"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => {
      if (e.key === 'Escape') {
        onclose();
      } else {
        e.stopPropagation();
      }
    }}
  >
    <header><h3>Export training corpus</h3></header>

    <p>
      Will export <strong>{promotedCount}</strong> promoted SOAP pair{promotedCount === 1 ? '' : 's'}
      to a timestamped subdirectory.
    </p>

    <fieldset>
      <legend>Base model filter</legend>
      {#each availableModels as model}
        <label>
          <input
            type="checkbox"
            checked={selectedModels.includes(model)}
            onchange={(e) => {
              if ((e.target as HTMLInputElement).checked) {
                selectedModels = [...selectedModels, model];
              } else {
                selectedModels = selectedModels.filter((m) => m !== model);
              }
            }}
          />
          <code>{model}</code>
        </label>
      {/each}
      {#if availableModels.length === 0}
        <p class="hint">(no model info available — exporting all rows)</p>
      {/if}
    </fieldset>

    <fieldset>
      <legend>Redaction strictness</legend>
      <label
        ><input type="radio" name="strict" value="standard" bind:group={strictness} /> Standard (default)</label
      >
      <label
        ><input
          type="radio"
          name="strict"
          value="aggressive"
          bind:group={strictness}
          disabled
        /> Aggressive (v2, coming later)</label
      >
    </fieldset>

    <fieldset>
      <legend>Output directory</legend>
      <div class="dir-row">
        <button onclick={pickDirectory}>Choose folder…</button>
        {#if outputDir}<code>{outputDir}</code>{/if}
      </div>
    </fieldset>

    {#if error}<div class="error">{error}</div>{/if}

    <footer class="modal-actions">
      <button onclick={onclose} disabled={exporting}>Cancel</button>
      <button class="primary" onclick={runExport} disabled={exporting || !outputDir}>
        {exporting ? 'Exporting…' : 'Export'}
      </button>
    </footer>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }
  .modal {
    background: var(--bg, white);
    border-radius: 8px;
    padding: 1.5rem;
    min-width: 520px;
    max-width: 700px;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }
  fieldset {
    border: 1px solid var(--border, #ddd);
    border-radius: 6px;
    padding: 0.75rem;
  }
  legend {
    font-weight: 600;
    padding: 0 0.5rem;
  }
  fieldset label {
    display: block;
    padding: 0.2rem 0;
  }
  .dir-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .dir-row code {
    font-size: 0.85rem;
    color: var(--muted-foreground, #888);
  }
  .hint {
    color: var(--muted-foreground, #888);
    font-size: 0.85rem;
  }
  .error {
    background: #fee;
    color: #991b1b;
    padding: 0.5rem;
    border-radius: 4px;
  }
  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    padding-top: 0.5rem;
    border-top: 1px solid var(--border, #ddd);
  }
  button.primary {
    background: #0066cc;
    color: white;
    padding: 0.4rem 1rem;
    border-radius: 4px;
    border: none;
    cursor: pointer;
  }
  button.primary:disabled {
    background: #ccc;
    cursor: not-allowed;
  }
</style>
