<script lang="ts">
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
    onAction: (id: string, action: 'promote' | 'reject' | 'unpromote' | 'restore') => void;
    mode: 'candidate' | 'promoted' | 'rejected';
  };
  let { generation, onAction, mode }: Props = $props();

  function previewOf(text: string | null, max = 150): string {
    if (!text) return '(no saved version — rejected draft)';
    if (text.length <= max) return text;
    return text.slice(0, max).trimEnd() + '…';
  }

  function editRatioChip(): { label: string; cls: string } {
    if (generation.final_text === null) return { label: 'no save', cls: 'chip-red' };
    const r = generation.edit_ratio;
    if (r === null) return { label: 'computing…', cls: 'chip-gray' };
    if (r < 0.15) return { label: 'light edit', cls: 'chip-green' };
    if (r < 0.4) return { label: 'moderate edit', cls: 'chip-yellow' };
    return { label: 'heavy edit', cls: 'chip-orange' };
  }

  let chip = $derived(editRatioChip());
</script>

<article class="gen-card">
  <header class="gen-card-header">
    <span class="gen-date">{new Date(generation.created_at).toLocaleString()}</span>
    <span class="gen-model">{generation.ai_model}</span>
    {#if generation.regeneration_seq > 1}
      <span class="gen-regen">#{generation.regeneration_seq}</span>
    {/if}
    <span class="chip {chip.cls}">{chip.label}</span>
  </header>

  <div class="gen-bodies">
    <div class="gen-half">
      <div class="gen-label">Draft</div>
      <div class="gen-preview">{previewOf(generation.draft_text)}</div>
    </div>
    <div class="gen-half">
      <div class="gen-label">Final</div>
      <div class="gen-preview">{previewOf(generation.final_text)}</div>
    </div>
  </div>

  <footer class="gen-actions">
    {#if mode === 'candidate'}
      <button class="action promote" onclick={() => onAction(generation.id, 'promote')}>Promote</button>
      <button class="action reject" onclick={() => onAction(generation.id, 'reject')}>Reject</button>
    {:else if mode === 'promoted'}
      <button class="action unpromote" onclick={() => onAction(generation.id, 'unpromote')}>Unpromote</button>
    {:else if mode === 'rejected'}
      <button class="action restore" onclick={() => onAction(generation.id, 'restore')}>Restore</button>
    {/if}
  </footer>
</article>

<style>
  .gen-card { border: 1px solid var(--border, #ddd); border-radius: 6px; padding: 0.75rem; margin-bottom: 0.5rem; }
  .gen-card-header { display: flex; gap: 0.5rem; align-items: center; font-size: 0.85rem; margin-bottom: 0.5rem; }
  .gen-date { color: var(--muted-foreground, #888); }
  .gen-model { font-family: var(--font-mono, monospace); font-size: 0.8rem; opacity: 0.7; }
  .gen-regen { background: #fef3c7; padding: 0.1rem 0.4rem; border-radius: 3px; font-size: 0.75rem; }
  .chip { padding: 0.1rem 0.5rem; border-radius: 10px; font-size: 0.75rem; }
  .chip-green { background: #d1fae5; color: #065f46; }
  .chip-yellow { background: #fef3c7; color: #92400e; }
  .chip-orange { background: #fed7aa; color: #9a3412; }
  .chip-red { background: #fecaca; color: #991b1b; }
  .chip-gray { background: #e5e7eb; color: #4b5563; }
  .gen-bodies { display: grid; grid-template-columns: 1fr 1fr; gap: 0.75rem; margin-bottom: 0.5rem; }
  .gen-half { background: var(--muted, #f5f5f5); padding: 0.5rem; border-radius: 4px; }
  .gen-label { font-size: 0.7rem; text-transform: uppercase; opacity: 0.7; margin-bottom: 0.25rem; }
  .gen-preview { white-space: pre-wrap; font-size: 0.85rem; line-height: 1.4; }
  .gen-actions { display: flex; gap: 0.5rem; }
  .action { padding: 0.35rem 0.8rem; border-radius: 4px; border: 1px solid; cursor: pointer; font-size: 0.85rem; }
  .promote { background: #059669; color: white; border-color: #059669; }
  .reject { background: white; color: #dc2626; border-color: #dc2626; }
  .unpromote { background: white; color: #6b7280; border-color: #d1d5db; }
  .restore { background: white; color: #0066cc; border-color: #0066cc; }
</style>
