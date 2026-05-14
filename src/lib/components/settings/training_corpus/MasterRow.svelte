<script lang="ts">
  import { firstChangeSnippet } from './diff';

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

  type Props = {
    generation: Generation;
    selected: boolean;
    onclick: () => void;
  };
  let { generation, selected, onclick }: Props = $props();

  function editChip(): { label: string; cls: string } {
    if (generation.final_text === null) return { label: 'no save', cls: 'chip-red' };
    const r = generation.edit_ratio;
    if (r === null) return { label: 'computing…', cls: 'chip-gray' };
    if (r < 0.15) return { label: 'light', cls: 'chip-green' };
    if (r < 0.4) return { label: 'moderate', cls: 'chip-yellow' };
    return { label: 'heavy', cls: 'chip-orange' };
  }

  function shortDate(iso: string): string {
    const d = new Date(iso);
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
      + ', '
      + d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  }

  let chip = $derived(editChip());
  let snippet = $derived(firstChangeSnippet(generation.draft_text, generation.final_text));
</script>

<button class="master-row" class:selected type="button" aria-pressed={selected} {onclick}>
  <div class="row-head">
    <span class="date">{shortDate(generation.created_at)}</span>
    <span class="chip {chip.cls}">{chip.label}</span>
  </div>
  <div class="model">{generation.ai_model}</div>
  {#if snippet}
    <div class="snippet">
      {#if snippet.removed !== null}<div class="snip-removed">− {snippet.removed}</div>{/if}
      {#if snippet.added !== null}<div class="snip-added">+ {snippet.added}</div>{/if}
    </div>
  {/if}
</button>

<style>
  .master-row {
    display: block;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    border-left: 3px solid transparent;
    padding: 0.5rem 0.75rem;
    cursor: pointer;
    font: inherit;
    color: inherit;
  }
  .master-row:hover { background: var(--bg-hover); }
  .master-row.selected {
    background: rgba(59,130,246,0.10);
    border-left-color: #3b82f6;
  }
  .row-head { display: flex; justify-content: space-between; align-items: center; }
  .date { font-size: 0.78rem; font-weight: 600; color: var(--text-primary); }
  .model { font-family: var(--font-mono, monospace); font-size: 0.7rem; color: var(--text-muted); margin-top: 0.15rem; }
  .snippet {
    margin-top: 0.4rem;
    background: var(--bg-code);
    border-radius: 3px;
    padding: 0.3rem 0.4rem;
    font-family: var(--font-mono, monospace);
    font-size: 0.72rem;
    line-height: 1.4;
  }
  .snip-removed { color: #fca5a5; }
  .snip-added { color: #86efac; }
  .chip { font-size: 0.68rem; padding: 0.05rem 0.45rem; border-radius: 9px; }
  .chip-green { background: #0a3b2a; color: #34d399; }
  .chip-yellow { background: #3b2a0a; color: #fbbf24; }
  .chip-orange { background: #3b1d0a; color: #fb923c; }
  .chip-red { background: #3b1d1d; color: #fca5a5; }
  .chip-gray { background: #1f2937; color: #9ca3af; }
</style>
