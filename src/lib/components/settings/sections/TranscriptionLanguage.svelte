<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';

  async function handleLanguageChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value;
    await settings.updateField('language', value);
  }

</script>

<div class="form-group">
  <label for="lang-select" class="form-label">Transcription Language</label>
  <select id="lang-select" value={settings.state.language} onchange={handleLanguageChange}>
    <option value="en-US">English</option>
    <option value="es-ES">Spanish (Español)</option>
    <option value="fr-FR">French (Français)</option>
    <option value="de-DE">German (Deutsch)</option>
    <option value="zh-CN">Chinese (中文)</option>
    <option value="pt-BR">Portuguese (Português)</option>
    <option value="ar-SA">Arabic (العربية)</option>
    <option value="ja-JP">Japanese (日本語)</option>
    <option value="hi-IN">Hindi (हिन्दी)</option>
    <option value="">Auto-detect (less reliable)</option>
  </select>
  <span class="form-hint">Controls the language hint passed to the transcription engine. Affects every recording.</span>
</div>

<style>
  .form-hint { color: var(--text-secondary); }
</style>
