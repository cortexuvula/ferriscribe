<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import CandidatesList from './training_corpus/CandidatesList.svelte';
  import PromotedList from './training_corpus/PromotedList.svelte';
  import RejectedList from './training_corpus/RejectedList.svelte';
  import { formatError } from '../../types/errors';

  type CorpusCounts = {
    candidates: number;
    promoted: number;
    rejected: number;
    excluded: number;
  };

  let activeView: 'candidates' | 'promoted' | 'rejected' = $state('candidates');
  let counts: CorpusCounts = $state({ candidates: 0, promoted: 0, rejected: 0, excluded: 0 });
  let error: string | null = $state(null);

  async function refreshCounts() {
    try {
      counts = await invoke<CorpusCounts>('training_corpus_counts');
    } catch (e) {
      error = formatError(e);
    }
  }

  onMount(refreshCounts);
</script>

<section class="training-corpus">
  <header class="tc-header">
    <h2>Training corpus</h2>
    <p class="tc-summary">
      {counts.candidates} candidate{counts.candidates === 1 ? '' : 's'} ·
      <strong>{counts.promoted}</strong> promoted ·
      {counts.rejected} rejected
    </p>
  </header>

  <div class="tc-tabs" role="tablist">
    <button
      role="tab"
      aria-selected={activeView === 'candidates'}
      class:active={activeView === 'candidates'}
      onclick={() => (activeView = 'candidates')}
    >
      Candidates ({counts.candidates})
    </button>
    <button
      role="tab"
      aria-selected={activeView === 'promoted'}
      class:active={activeView === 'promoted'}
      onclick={() => (activeView = 'promoted')}
    >
      Promoted ({counts.promoted})
    </button>
    <button
      role="tab"
      aria-selected={activeView === 'rejected'}
      class:active={activeView === 'rejected'}
      onclick={() => (activeView = 'rejected')}
    >
      Rejected ({counts.rejected})
    </button>
  </div>

  {#if error}
    <div class="tc-error">{error}</div>
  {/if}

  <div class="tc-view">
    {#if activeView === 'candidates'}
      <CandidatesList onchange={refreshCounts} />
    {:else if activeView === 'promoted'}
      <PromotedList onchange={refreshCounts} />
    {:else if activeView === 'rejected'}
      <RejectedList onchange={refreshCounts} />
    {/if}
  </div>
</section>

<style>
  .training-corpus { display: flex; flex-direction: column; gap: 1rem; padding: 1rem; }
  .tc-header h2 { margin: 0 0 0.25rem 0; }
  .tc-summary { color: var(--muted-foreground, #888); margin: 0; }
  .tc-tabs { display: flex; gap: 0.5rem; border-bottom: 1px solid var(--border, #ddd); }
  .tc-tabs button {
    background: none;
    border: none;
    padding: 0.5rem 1rem;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    color: var(--muted-foreground, #888);
  }
  .tc-tabs button.active { color: var(--foreground, #222); border-bottom-color: var(--accent, #0066cc); }
  .tc-error { background: #fee; border: 1px solid #fbb; padding: 0.5rem; border-radius: 4px; }
  .tc-view { flex: 1; overflow: auto; }
</style>
