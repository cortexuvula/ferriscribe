<script lang="ts">
  interface Props {
    onChoose: (mode: 'local' | 'server') => void;
    onSkip: () => void;
  }
  const { onChoose, onSkip }: Props = $props();
</script>

<h2>How will this computer use FerriScribe?</h2>
<p class="hint">You can change this later in Settings.</p>

<div class="mode-cards">
  <button class="mode-card" onclick={() => onChoose('local')}>
    <span class="mode-icon" aria-hidden="true">🖥️</span>
    <div class="mode-info">
      <strong>This machine runs its own AI</strong>
      <span>For a single computer, or the clinic's main server. You'll set up a local AI server (Ollama, LM Studio, or oMLX) here.</span>
    </div>
  </button>

  <button class="mode-card" onclick={() => onChoose('server')}>
    <span class="mode-icon" aria-hidden="true">🔌</span>
    <div class="mode-info">
      <strong>Connect to an office server</strong>
      <span>For a laptop that offloads transcription and AI to another machine on the network.</span>
    </div>
  </button>
</div>

<button class="btn-skip" onclick={onSkip}>Skip setup for now</button>

<style>
  h2 { font-size: 18px; font-weight: 600; margin: 0 0 4px; color: var(--text-primary); }
  .hint { font-size: 13px; color: var(--text-muted); margin: 0 0 16px; }
  .mode-cards { display: flex; flex-direction: column; gap: 12px; }
  .mode-card {
    display: flex; align-items: flex-start; gap: 14px; text-align: left;
    padding: 16px; background-color: var(--bg-primary, #1a1a1a);
    border: 1px solid var(--border, #333); border-radius: var(--radius-md, 8px);
    cursor: pointer; transition: border-color 0.15s ease, background-color 0.15s ease;
  }
  .mode-card:hover { border-color: var(--accent, #3b82f6); background-color: var(--bg-hover, #2a2a2a); }
  .mode-icon { font-size: 26px; line-height: 1; flex-shrink: 0; }
  .mode-info { display: flex; flex-direction: column; gap: 3px; }
  .mode-info strong { font-size: 14px; color: var(--text-primary); }
  .mode-info span { font-size: 12px; color: var(--text-muted); line-height: 1.4; }
  .btn-skip {
    margin-top: 16px; align-self: center; padding: 6px 14px; font-size: 12px;
    color: var(--text-muted); background: none; border: none; cursor: pointer;
    text-decoration: underline;
  }
  .btn-skip:hover { color: var(--text-secondary); }
</style>
