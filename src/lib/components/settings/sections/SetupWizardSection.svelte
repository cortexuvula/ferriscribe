<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';
  async function handleRerunSetup() {
    await settings.updateField('onboarding_completed', false);
  }
</script>

<div class="form-group">
  <span class="form-label">Setup Wizard</span>
  <div class="storage-path-row">
    <button class="btn-reset" onclick={handleRerunSetup}>Re-run setup</button>
  </div>
  <span class="form-hint">Walk through the first-run setup again to reconfigure your AI provider, transcription model, or office-server pairing.</span>
</div>

