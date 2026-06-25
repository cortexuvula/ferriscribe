<script lang="ts">
  import { settings } from '../../stores/settings.svelte';

  interface Props {
    onNext: () => void;
    onSkip: () => void;
  }
  const { onNext, onSkip }: Props = $props();

  // The languages whisper.cpp supports. The value is the BCP-47 code stored in
  // AppConfig.language; whisper.rs extracts the first 2 chars for set_language.
  // Empty string = auto-detect (lets whisper guess — less reliable).
  const LANGUAGES = [
    { code: 'en-US', label: 'English' },
    { code: 'es-ES', label: 'Spanish (Español)' },
    { code: 'fr-FR', label: 'French (Français)' },
    { code: 'de-DE', label: 'German (Deutsch)' },
    { code: 'zh-CN', label: 'Chinese (中文)' },
    { code: 'pt-BR', label: 'Portuguese (Português)' },
    { code: 'ar-SA', label: 'Arabic (العربية)' },
    { code: 'ja-JP', label: 'Japanese (日本語)' },
    { code: 'hi-IN', label: 'Hindi (हिन्दी)' },
    { code: '', label: 'Auto-detect (less reliable)' },
  ];

  let selected = $state(settings.state.language);

  async function saveAndNext() {
    await settings.updateField('language', selected);
    onNext();
  }
</script>

<h2>Transcription language</h2>
<p class="hint">
  Pick the language your patients and staff speak. This helps the transcription
  engine recognize speech accurately. You can change this later in Settings.
</p>

<div class="field">
  <label for="ob-language">Language</label>
  <select id="ob-language" bind:value={selected}>
    {#each LANGUAGES as lang}
      <option value={lang.code}>{lang.label}</option>
    {/each}
  </select>
</div>

<div class="actions">
  <button class="btn-skip" onclick={onSkip}>Skip for now</button>
  <button class="btn-primary" onclick={saveAndNext}>Next →</button>
</div>

<style>
  h2 { font-size: 18px; font-weight: 600; margin: 0 0 4px; color: var(--text-primary); }
  .hint { font-size: 13px; color: var(--text-muted); margin: 0 0 16px; line-height: 1.5; }
  .field { display: flex; flex-direction: column; gap: 4px; margin-bottom: 12px; }
  label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; color: var(--text-secondary); }
  select {
    height: 36px; padding: 0 10px; font-size: 13px; color: var(--text-primary);
    background-color: var(--bg-primary, #1a1a1a); border: 1px solid var(--border, #333);
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  select:focus { outline: none; border-color: var(--accent, #3b82f6); }
  .actions { display: flex; justify-content: space-between; align-items: center; margin-top: 16px; }
  .btn-skip { padding: 6px 10px; font-size: 12px; color: var(--text-muted); background: none; border: none; cursor: pointer; text-decoration: underline; }
  .btn-primary {
    padding: 8px 20px; font-size: 13px; font-weight: 600; color: white;
    background-color: var(--accent, #3b82f6); border: none;
    border-radius: var(--radius-sm, 4px); cursor: pointer;
  }
  .btn-primary:hover { background-color: var(--accent-hover, #2563eb); }
</style>
