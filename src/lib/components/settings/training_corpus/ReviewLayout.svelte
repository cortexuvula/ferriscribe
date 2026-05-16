<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import MasterRow from './MasterRow.svelte';
  import DetailPane from './DetailPane.svelte';
  import { formatError } from '../../../types/errors';

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
  type Mode = 'candidate' | 'promoted' | 'rejected';
  type Action = 'promote' | 'reject' | 'unpromote' | 'restore';

  type Props = {
    mode: Mode;
    onchange?: () => void;
  };
  let { mode, onchange }: Props = $props();

  let items: Generation[] = $state([]);
  let total = $state(0);
  let offset = $state(0);
  let selectedId: string | null = $state(null);
  let loading = $state(false);
  let error: string | null = $state(null);
  const PAGE_SIZE = 50;

  let cursorIndex = $derived(
    selectedId ? items.findIndex((g) => g.id === selectedId) : -1
  );
  let selected = $derived(items.find((g) => g.id === selectedId) ?? null);
  let position = $derived(
    cursorIndex >= 0 && total > 0
      ? { index: offset + cursorIndex, total }
      : null
  );

  async function load(opts?: { keepSelection?: boolean; selectLast?: boolean; selectFirst?: boolean }) {
    const prevSelectedId = selectedId;
    const prevCursor = cursorIndex;
    loading = true;
    error = null;
    try {
      const page = await invoke<Page>('training_corpus_list', {
        status: mode,
        limit: PAGE_SIZE,
        offset,
      });
      items = page.items;
      total = page.total;
    } catch (e) {
      error = formatError(e);
      items = [];
      total = 0;
    } finally {
      loading = false;
    }

    if (items.length === 0 && offset > 0 && total > 0) {
      offset = Math.max(0, offset - PAGE_SIZE);
      await load(opts);
      return;
    }

    if (opts?.selectFirst && items.length > 0) {
      selectedId = items[0].id;
    } else if (opts?.selectLast && items.length > 0) {
      selectedId = items[items.length - 1].id;
    } else if (opts?.keepSelection && prevSelectedId && items.some((g) => g.id === prevSelectedId)) {
      selectedId = prevSelectedId;
    } else if (items.length > 0) {
      const idx = Math.min(Math.max(prevCursor, 0), items.length - 1);
      selectedId = items[idx].id;
    } else {
      selectedId = null;
    }
  }

  async function act(id: string, action: Action) {
    const new_status =
      action === 'promote' ? 'promoted' :
      action === 'reject' ? 'rejected' :
      'candidate';
    loading = true;
    try {
      await invoke('training_corpus_set_status', { id, newStatus: new_status });
      await load();
      onchange?.();
    } catch (e) {
      error = formatError(e);
      loading = false;
    }
  }

  async function goNext() {
    if (cursorIndex < items.length - 1) {
      selectedId = items[cursorIndex + 1].id;
    } else if (offset + items.length < total) {
      offset += PAGE_SIZE;
      await load({ selectFirst: true });
    }
  }

  async function goPrev() {
    if (cursorIndex > 0) {
      selectedId = items[cursorIndex - 1].id;
    } else if (offset > 0) {
      offset = Math.max(0, offset - PAGE_SIZE);
      await load({ selectLast: true });
    }
  }

  function onKey(ev: KeyboardEvent) {
    if (loading || items.length === 0) return;
    // Don't intercept when the user is typing in an input/textarea.
    const target = ev.target as HTMLElement | null;
    if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) return;

    const key = ev.key.toLowerCase();
    if (key === 'j') { goNext(); ev.preventDefault(); }
    else if (key === 'k') { goPrev(); ev.preventDefault(); }
    else if (key === 's' && mode === 'candidate') { goNext(); ev.preventDefault(); }
    else if (key === 'p' && mode === 'candidate' && selected) { act(selected.id, 'promote'); ev.preventDefault(); }
    else if (key === 'r' && mode === 'candidate' && selected) { act(selected.id, 'reject'); ev.preventDefault(); }
    else if (key === 'u' && mode === 'promoted' && selected) { act(selected.id, 'unpromote'); ev.preventDefault(); }
    else if (key === 'r' && mode === 'rejected' && selected) { act(selected.id, 'restore'); ev.preventDefault(); }
  }

  onMount(() => load());
</script>

<svelte:window onkeydown={onKey} />

<div class="layout">
  {#if error}
    <div class="error">{error}</div>
  {/if}

  {#if loading && items.length === 0}
    <div class="empty">Loading…</div>
  {:else if !loading && items.length === 0}
    <div class="empty">
      {#if mode === 'candidate'}
        No candidates. Generate a SOAP note with capture enabled to populate this list.
      {:else if mode === 'promoted'}
        No promoted candidates yet. Promote a candidate to add it to the training corpus.
      {:else}
        No rejected candidates.
      {/if}
    </div>
  {:else}
    <div class="grid">
      <aside class="master" aria-label="Candidate list">
        {#each items as g (g.id)}
          <MasterRow
            generation={g}
            selected={g.id === selectedId}
            onclick={() => (selectedId = g.id)}
          />
        {/each}
        {#if total > PAGE_SIZE}
          <nav class="pagination">
            <button disabled={offset === 0 || loading} onclick={() => { offset = Math.max(0, offset - PAGE_SIZE); load(); }}>← Prev</button>
            <span>{offset + 1}–{Math.min(offset + PAGE_SIZE, total)} of {total}</span>
            <button disabled={offset + PAGE_SIZE >= total || loading} onclick={() => { offset += PAGE_SIZE; load(); }}>Next →</button>
          </nav>
        {/if}
      </aside>
      <section class="detail-col" aria-label="Selected candidate detail">
        <DetailPane
          generation={selected}
          mode={mode}
          loading={loading}
          position={position}
          onAction={act}
        />
      </section>
    </div>
    <p class="kbd-hint">
      <kbd>J</kbd>/<kbd>K</kbd> navigate
      {#if mode === 'candidate'}· <kbd>P</kbd> promote · <kbd>R</kbd> reject · <kbd>S</kbd> skip{/if}
      {#if mode === 'promoted'}· <kbd>U</kbd> unpromote{/if}
      {#if mode === 'rejected'}· <kbd>R</kbd> restore{/if}
    </p>
  {/if}
</div>

<style>
  .layout {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }

  .error {
    padding: 0.5rem;
    background: var(--bg-input);
    color: #991b1b;
    border-radius: 4px;
  }

  .empty {
    padding: 1rem;
    color: var(--text-muted);
  }

  .grid {
    display: grid;
    grid-template-columns: 240px 1fr;
    border: 1px solid var(--border);
    border-radius: 6px;
    min-height: 420px;
    overflow: hidden;
  }

  .master {
    border-right: 1px solid var(--border);
    overflow-y: auto;
    max-height: 70vh;
  }

  .detail-col {
    overflow: hidden;
  }

  .pagination {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    padding: 0.5rem;
    font-size: 0.75rem;
  }

  .pagination button {
    padding: 0.25rem 0.6rem;
    cursor: pointer;
  }

  .kbd-hint {
    font-size: 0.8rem;
    color: var(--text-muted);
    margin: 0.4rem 0 0 0;
  }

  kbd {
    background: var(--bg-code);
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    padding: 0.1rem 0.35rem;
    border-radius: 3px;
    font-family: var(--font-mono, monospace);
    font-size: 0.8rem;
  }
</style>
