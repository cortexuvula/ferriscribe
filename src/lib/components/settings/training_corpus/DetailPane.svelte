<script lang="ts">
  import { diffLines } from './diff';

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

  type Action = 'promote' | 'reject' | 'unpromote' | 'restore';
  type Mode = 'candidate' | 'promoted' | 'rejected';

  type Props = {
    generation: Generation | null;
    mode: Mode;
    loading: boolean;
    position: { index: number; total: number } | null;
    onAction: (id: string, action: Action) => void;
  };
  let { generation, mode, loading, position, onAction }: Props = $props();

  function fullDate(iso: string): string {
    return new Date(iso).toLocaleString();
  }

  function chip(g: Generation): { label: string; cls: string } {
    const r = g.edit_ratio;
    const pct = r === null ? null : Math.round(r * 100);
    if (g.final_text === null) return { label: 'no save', cls: 'chip-red' };
    if (r === null) return { label: 'computing…', cls: 'chip-gray' };
    if (r < 0.15) return { label: `light edit · ${pct}% changed`, cls: 'chip-green' };
    if (r < 0.4) return { label: `moderate edit · ${pct}% changed`, cls: 'chip-yellow' };
    return { label: `heavy edit · ${pct}% changed`, cls: 'chip-orange' };
  }

  // Compute the diff for the currently selected generation. final_text being
  // null shouldn't happen in `candidate` mode after Task 3, but `promoted`
  // and `rejected` may still have it — show draft only in that case.
  let diff = $derived.by(() => {
    if (!generation) return [];
    if (generation.final_text === null) return [];
    return diffLines(generation.draft_text, generation.final_text);
  });

  let chipInfo = $derived(generation ? chip(generation) : { label: '', cls: '' });
</script>

{#if !generation}
  <div class="empty">Nothing to review.</div>
{:else}
  <div class="detail" class:dimmed={loading} aria-busy={loading}>
    <header class="head">
      <div class="head-meta">
        <div class="date">{fullDate(generation.created_at)}</div>
        <div class="meta">
          <span class="model">{generation.ai_model}</span>
          {#if generation.regeneration_seq > 1}
            <span class="regen">#{generation.regeneration_seq}</span>
          {/if}
          <span class="chip {chipInfo.cls}">{chipInfo.label}</span>
        </div>
      </div>
      <div class="actions">
        {#if mode === 'candidate'}
          <button class="btn promote" disabled={loading} onclick={() => onAction(generation.id, 'promote')}>Promote (P)</button>
          <button class="btn reject" disabled={loading} onclick={() => onAction(generation.id, 'reject')}>Reject (R)</button>
        {:else if mode === 'promoted'}
          <button class="btn neutral" disabled={loading} onclick={() => onAction(generation.id, 'unpromote')}>Unpromote (U)</button>
        {:else if mode === 'rejected'}
          <button class="btn neutral" disabled={loading} onclick={() => onAction(generation.id, 'restore')}>Restore (R)</button>
        {/if}
      </div>
    </header>

    <div class="body">
      {#if generation.final_text === null}
        <pre class="draft-only">{generation.draft_text}</pre>
      {:else}
        {#each diff as line, i (i)}
          <div class="line line-{line.kind}">
            <span class="sign">{line.kind === 'add' ? '+' : line.kind === 'remove' ? '−' : ' '}</span><span class="text">{line.text}</span>
          </div>
        {/each}
      {/if}
    </div>

    {#if position}
      <footer class="foot">
        <span>{mode === 'candidate' ? 'Candidate' : mode === 'promoted' ? 'Promoted' : 'Rejected'} <strong>{position.index + 1} of {position.total}</strong></span>
      </footer>
    {/if}
  </div>
{/if}

<style>
  .empty {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2rem;
    color: var(--text-muted);
  }

  .detail {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .detail.dimmed .body {
    opacity: 0.55;
    pointer-events: none;
  }

  .head {
    position: sticky;
    top: 0;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-tertiary);
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 0.75rem;
    z-index: 1;
  }

  .head-meta {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    min-width: 0;
  }

  .date {
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--text-primary);
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.75rem;
    color: var(--text-muted);
    flex-wrap: wrap;
  }

  .model {
    font-family: var(--font-mono, monospace);
  }

  .regen {
    background: #3b2a0a;
    color: #fbbf24;
    padding: 0.05rem 0.4rem;
    border-radius: 3px;
    font-size: 0.7rem;
  }

  .chip {
    font-size: 0.68rem;
    padding: 0.05rem 0.45rem;
    border-radius: 9px;
  }
  .chip-green  { background: #0a3b2a; color: #34d399; }
  .chip-yellow { background: #3b2a0a; color: #fbbf24; }
  .chip-orange { background: #3b1d0a; color: #fb923c; }
  .chip-red    { background: #3b1d1d; color: #fca5a5; }
  .chip-gray   { background: #1f2937; color: #9ca3af; }

  .actions {
    display: flex;
    gap: 0.4rem;
    flex-shrink: 0;
    align-items: center;
  }

  .btn {
    padding: 0.3rem 0.8rem;
    border-radius: 4px;
    font-size: 0.8rem;
    background: transparent;
    border: 1px solid;
    cursor: pointer;
    font: inherit;
    font-size: 0.8rem;
    line-height: 1.4;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn.promote {
    background: #059669;
    color: white;
    border-color: #059669;
  }

  .btn.reject {
    color: #dc2626;
    border-color: #dc2626;
  }

  .btn.neutral {
    color: var(--text-primary);
    border-color: var(--bg-tertiary);
  }

  .body {
    flex: 1;
    overflow-y: auto;
    padding: 0.75rem 1rem;
    font-family: var(--font-mono, monospace);
    font-size: 0.78rem;
    line-height: 1.55;
  }

  .line {
    white-space: pre-wrap;
    word-break: break-word;
  }

  .line-context { color: var(--text-primary); }
  .line-add     { background: rgba(34, 197, 94, 0.10); color: #86efac; }
  .line-remove  { background: rgba(239, 68, 68, 0.10); color: #fca5a5; }

  .sign {
    display: inline-block;
    width: 1ch;
    opacity: 0.7;
    user-select: none;
  }

  .draft-only {
    white-space: pre-wrap;
    word-break: break-word;
    font-family: var(--font-mono, monospace);
    font-size: 0.78rem;
    line-height: 1.55;
    margin: 0;
    color: var(--text-primary);
  }

  .foot {
    padding: 0.4rem 1rem;
    border-top: 1px solid var(--border);
    font-size: 0.72rem;
    color: var(--text-muted);
    background: var(--bg-tertiary);
  }
</style>
