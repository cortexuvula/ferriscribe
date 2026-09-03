<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import CandidatesList from './training_corpus/CandidatesList.svelte';
  import PromotedList from './training_corpus/PromotedList.svelte';
  import RejectedList from './training_corpus/RejectedList.svelte';
  import Callout from './Callout.svelte';
  import { formatError } from '../../types/errors';

  type CorpusCounts = {
    candidates: number;
    promoted: number;
    rejected: number;
    excluded: number;
  };

  type View = 'candidates' | 'promoted' | 'rejected';
  const VIEWS: View[] = ['candidates', 'promoted', 'rejected'];
  const VIEW_LABELS: Record<View, string> = {
    candidates: 'Candidates',
    promoted: 'Promoted',
    rejected: 'Rejected',
  };

  let activeView: View = $state('candidates');
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

  // WAI-ARIA tabs pattern: arrow/Home/End keys move between tabs, focus
  // follows, inactive tabs leave the tab order.
  function selectView(view: View) {
    activeView = view;
  }

  async function onTablistKeydown(e: KeyboardEvent) {
    const idx = VIEWS.indexOf(activeView);
    let next: number | null = null;
    if (e.key === 'ArrowRight') next = (idx + 1) % VIEWS.length;
    else if (e.key === 'ArrowLeft') next = (idx - 1 + VIEWS.length) % VIEWS.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = VIEWS.length - 1;
    if (next === null) return;
    e.preventDefault();
    activeView = VIEWS[next];
    await tick();
    document.getElementById(`tc-tab-${VIEWS[next]}`)?.focus();
  }
</script>

<section class="training-corpus">
  <header class="tc-header">
    <h2>Training corpus</h2>
    <p class="tc-summary">
      {counts.candidates} candidate{counts.candidates === 1 ? '' : 's'} ·
      <strong>{counts.promoted}</strong> promoted ·
      {counts.rejected} rejected ·
      {counts.excluded} excluded
    </p>
  </header>

  <!-- Roving tabindex lives on the tab buttons themselves (active tab 0,
       inactive -1) per the WAI-ARIA tabs pattern; the tablist container is
       not focusable. -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div class="tc-tabs" role="tablist" aria-label="Training corpus views" onkeydown={onTablistKeydown}>
    {#each VIEWS as view (view)}
      <button
        role="tab"
        id={`tc-tab-${view}`}
        aria-selected={activeView === view}
        aria-controls="tc-panel"
        tabindex={activeView === view ? 0 : -1}
        class:active={activeView === view}
        onclick={() => selectView(view)}
      >
        {VIEW_LABELS[view]} ({counts[view]})
      </button>
    {/each}
  </div>

  {#if error}
    <Callout kind="danger">{error}</Callout>
  {/if}

  <div class="tc-view" role="tabpanel" id="tc-panel" aria-labelledby={`tc-tab-${activeView}`}>
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
  .tc-summary { color: var(--text-muted); margin: 0; }
  .tc-tabs { display: flex; gap: 0.5rem; border-bottom: 1px solid var(--border); }
  .tc-tabs button {
    background: none;
    border: none;
    padding: 0.5rem 1rem;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    color: var(--text-muted);
  }
  .tc-tabs button.active { color: var(--text-primary); border-bottom-color: var(--accent); }
  .tc-view { flex: 1; overflow: auto; }
</style>
