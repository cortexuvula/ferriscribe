<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import GenerationCard from './GenerationCard.svelte';
  import ExportDialog from './ExportDialog.svelte';

  type Generation = {
    id: string;
    recording_id: string;
    created_at: string;
    draft_text: string;
    final_text: string | null;
    ai_model: string;
    edit_ratio: number | null;
    regeneration_seq: number;
  };
  type Page = { items: Generation[]; total: number };

  let { onchange }: { onchange: () => void } = $props();

  let items: Generation[] = $state([]);
  let total = $state(0);
  let loading = $state(false);
  let error: string | null = $state(null);
  let cursorIndex = $state(0);
  const PAGE_SIZE = 50;
  let offset = $state(0);

  let showExport = $state(false);
  let successMessage: string | null = $state(null);

  function distinctModels(): string[] {
    const set = new Set(items.map((g) => g.ai_model));
    return Array.from(set).sort();
  }

  async function load() {
    loading = true;
    error = null;
    try {
      const page = await invoke<Page>('training_corpus_list', {
        status: 'promoted',
        limit: PAGE_SIZE,
        offset,
      });
      items = page.items;
      total = page.total;
      cursorIndex = Math.max(0, Math.min(cursorIndex, items.length - 1));
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
    // Post-load rewind: if we landed on an empty page that has data on a
    // prior page, rewind and try again. Outside the try/finally so the
    // recursive load() manages its own loading state cleanly.
    if (items.length === 0 && offset > 0 && total > 0) {
      offset = Math.max(0, offset - PAGE_SIZE);
      await load();
    }
  }

  async function act(id: string, action: 'promote' | 'reject' | 'unpromote' | 'restore') {
    const new_status =
      action === 'unpromote' || action === 'restore' ? 'candidate' :
      action === 'promote' ? 'promoted' :
      action === 'reject' ? 'rejected' :
      'candidate';
    try {
      await invoke('training_corpus_set_status', { id, newStatus: new_status });
      await load();
      onchange?.();
    } catch (e) {
      error = String(e);
    }
  }

  function onKey(ev: KeyboardEvent) {
    if (loading || items.length === 0) return;
    const key = ev.key.toLowerCase();
    if (key === 'j') { cursorIndex = Math.min(cursorIndex + 1, items.length - 1); ev.preventDefault(); }
    else if (key === 'k') { cursorIndex = Math.max(cursorIndex - 1, 0); ev.preventDefault(); }
    else if (key === 'u') { act(items[cursorIndex].id, 'unpromote'); ev.preventDefault(); }
  }

  onMount(load);
</script>

<svelte:window onkeydown={onKey} />

<div class="promoted-list">
  <div class="promoted-toolbar">
    <button onclick={() => (showExport = true)} disabled={total === 0}>
      Export training corpus…
    </button>
    {#if successMessage}<span class="success">{successMessage}</span>{/if}
  </div>

  {#if loading}<div class="info">Loading…</div>{/if}
  {#if error}<div class="error">{error}</div>{/if}
  {#if !loading && items.length === 0}
    <div class="empty">No promoted candidates yet. Promote a candidate to add it to the training corpus.</div>
  {/if}

  {#each items as g, i (g.id)}
    <div class:cursor-row={i === cursorIndex} class="row-wrap">
      <GenerationCard generation={g} mode="promoted" onAction={act} />
    </div>
  {/each}

  {#if total > PAGE_SIZE}
    <nav class="pagination">
      <button disabled={offset === 0} onclick={() => { offset = Math.max(0, offset - PAGE_SIZE); load(); }}>← Prev</button>
      <span>{offset + 1}–{Math.min(offset + PAGE_SIZE, total)} of {total}</span>
      <button disabled={offset + PAGE_SIZE >= total} onclick={() => { offset += PAGE_SIZE; load(); }}>Next →</button>
    </nav>
  {/if}

  <p class="kbd-hint">
    <kbd>J</kbd>/<kbd>K</kbd> navigate · <kbd>U</kbd> unpromote
  </p>
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
  .promoted-list { display: flex; flex-direction: column; gap: 0.25rem; }
  .promoted-toolbar { display: flex; align-items: center; gap: 0.75rem; padding: 0.5rem 0; }
  .success { font-size: 0.85rem; color: #166534; background: #dcfce7; padding: 0.3rem 0.6rem; border-radius: 4px; flex: 1; }
  .info, .empty { padding: 1rem; color: var(--muted-foreground, #888); }
  .error { padding: 0.5rem; background: #fee; color: #991b1b; border-radius: 4px; }
  .row-wrap { padding: 0.15rem; border-radius: 6px; }
  .cursor-row { background: rgba(0,102,204,0.08); }
  .pagination { display: flex; gap: 1rem; align-items: center; padding: 0.5rem; }
  .pagination button { padding: 0.35rem 0.75rem; cursor: pointer; }
  .kbd-hint { font-size: 0.8rem; color: var(--muted-foreground, #888); }
  kbd { background: var(--muted, #f5f5f5); border: 1px solid var(--border, #ccc); border-bottom-width: 2px; padding: 0.1rem 0.35rem; border-radius: 3px; font-family: var(--font-mono, monospace); font-size: 0.8rem; }
</style>
