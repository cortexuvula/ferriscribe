<script lang="ts">
  import type { SettingsSection } from '../../stores/settingsNav.svelte';
  import ScreenshotOcrSection from './sections/ScreenshotOcrSection.svelte';
  import AdvancedSettings from './sections/AdvancedSettings.svelte';
  import SetupWizardSection from './sections/SetupWizardSection.svelte';
  let { onNavigate }: { onNavigate: (section: SettingsSection) => void } = $props();
</script>

<section class="settings-section settings-composition">
  <h3 class="section-title">Advanced</h3>
  <p class="section-desc">Optional tools and settings that change how FerriScribe handles data.</p>
  <ScreenshotOcrSection />
  <AdvancedSettings />
  <div class="form-group">
    <span class="form-label">Training Corpus</span>
    <button class="settings-secondary-action" onclick={() => onNavigate('training-corpus')}>Open Training Corpus</button>
    <span class="form-hint">Review and export locally captured generations.</span>
  </div>
  <SetupWizardSection />
</section>
