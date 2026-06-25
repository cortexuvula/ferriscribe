<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import ReviewLayout from './ReviewLayout.svelte';
  import ExportDialog from './ExportDialog.svelte';

  type Generation = {
    id: string;
    ai_model: string;
  };

  const { onchange }: { onchange: () => void } = $props();

  // The toolbar shows distinct models from the promoted set. We do a single
  // separate fetch here (paginated, model field only) — this is cheap and
  // keeps ReviewLayout free of toolbar concerns.
  let allPromoted: Generation[] = $state([]);
  let total = $state(0);
  let showExport = $state(false);
  let successMessage: string | null = $state(null);

  async function loadModels() {
    try {
      const page = await invoke<{ items: Generation[]; total: number }>(
        'training_corpus_list',
        { status: 'promoted', limit: 200, offset: 0 },
      );
      allPromoted = page.items;
      total = page.total;
    } catch {
      allPromoted = [];
      total = 0;
    }
  }

  function distinctModels(): string[] {
    return Array.from(new Set(allPromoted.map((g) => g.ai_model))).sort();
  }

  function onChildChange() {
    loadModels();
    onchange?.();
  }

  onMount(loadModels);
</script>

<div class="promoted-wrap">
  <div class="promoted-toolbar">
    <button onclick={() => (showExport = true)} disabled={total === 0}>
      Export training corpus…
    </button>
    {#if successMessage}<span class="success">{successMessage}</span>{/if}
  </div>

  <ReviewLayout mode="promoted" onchange={onChildChange} />
</div>

{#if showExport}
  <ExportDialog
    promotedCount={total}
    availableModels={distinctModels()}
    onclose={() => (showExport = false)}
    onsuccess={(dir, pairs, warnings) => {
      showExport = false;
      successMessage =
        `Exported ${pairs} pair${pairs === 1 ? '' : 's'} to ${dir}` +
        (warnings > 0 ? ` (${warnings} redaction warning${warnings === 1 ? '' : 's'} — see manifest.json)` : '');
    }}
  />
{/if}

<style>
  .promoted-wrap { display: flex; flex-direction: column; gap: 0.5rem; }
  .promoted-toolbar { display: flex; align-items: center; gap: 0.75rem; padding: 0.25rem 0; }
  .success { font-size: 0.85rem; color: #166534; background: #dcfce7; padding: 0.3rem 0.6rem; border-radius: 4px; flex: 1; }
</style>
