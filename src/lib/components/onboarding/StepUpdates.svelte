<script lang="ts">
  import { settings } from '../../stores/settings.svelte';

  interface Props {
    onNext: () => void;
    onSkip: () => void;
  }
  const { onNext, onSkip }: Props = $props();

  let choice = $state<'auto' | 'manual'>(settings.state.auto_update_check ? 'auto' : 'manual');

  async function saveAndNext() {
    await settings.updateField('auto_update_check', choice === 'auto');
    onNext();
  }

  async function skip() {
    // Default to auto-check ON (recommended) when skipping.
    await settings.updateField('auto_update_check', true);
    onSkip();
  }
</script>

<h2>Automatic updates</h2>
<p class="hint">
  FerriScribe can check for new versions automatically. The check contacts
  GitHub Releases for the latest version manifest — no patient data is ever sent.
</p>

<div class="mode-cards">
  <button class="mode-card" class:selected={choice === 'auto'} onclick={() => choice = 'auto'}>
    <span class="mode-icon" aria-hidden="true">🔄</span>
    <div class="mode-info">
      <strong>Check automatically (recommended)</strong>
      <span>Checks on launch and every 12 hours. Shows a discreet banner when an update is available.</span>
    </div>
  </button>

  <button class="mode-card" class:selected={choice === 'manual'} onclick={() => choice = 'manual'}>
    <span class="mode-icon" aria-hidden="true">👐</span>
    <div class="mode-info">
      <strong>I'll check manually</strong>
      <span>No automatic checks. You can check at any time via Settings → About → Check for updates.</span>
    </div>
  </button>
</div>

<div class="actions">
  <button class="btn-skip" onclick={skip}>Skip for now</button>
  <button class="btn-primary" onclick={saveAndNext}>Next →</button>
</div>

<style>
  h2 { font-size: 18px; font-weight: 600; margin: 0 0 4px; color: var(--text-primary); }
  .hint { font-size: 13px; color: var(--text-muted); margin: 0 0 16px; line-height: 1.5; }
  .mode-cards { display: flex; flex-direction: column; gap: 12px; }
  .mode-card {
    display: flex; align-items: flex-start; gap: 14px; text-align: left;
    padding: 16px; background-color: var(--bg-primary, #1a1a1a);
    border: 1px solid var(--border, #333); border-radius: var(--radius-md, 8px);
    cursor: pointer; transition: border-color 0.15s ease, background-color 0.15s ease;
  }
  .mode-card.selected { border-color: var(--accent, #3b82f6); background-color: color-mix(in srgb, var(--accent, #3b82f6) 8%, transparent); }
  .mode-card:hover { border-color: var(--accent, #3b82f6); }
  .mode-icon { font-size: 26px; line-height: 1; flex-shrink: 0; }
  .mode-info { display: flex; flex-direction: column; gap: 3px; }
  .mode-info strong { font-size: 14px; color: var(--text-primary); }
  .mode-info span { font-size: 12px; color: var(--text-muted); line-height: 1.4; }
  .actions { display: flex; justify-content: space-between; align-items: center; margin-top: 16px; }
  .btn-skip { padding: 6px 10px; font-size: 12px; color: var(--text-muted); background: none; border: none; cursor: pointer; text-decoration: underline; }
  .btn-primary {
    padding: 8px 20px; font-size: 13px; font-weight: 600; color: white;
    background-color: var(--accent, #3b82f6); border: none;
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-primary:hover { background-color: var(--accent-hover, #2563eb); }
</style>
