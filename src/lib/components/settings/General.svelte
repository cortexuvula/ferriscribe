<script lang="ts">
  import { settings } from '../../stores/settings.svelte';
  import GeneralBasics from './sections/GeneralBasics.svelte';
  import DatabaseSecurity from './sections/DatabaseSecurity.svelte';
  import DataManagement from './sections/DataManagement.svelte';
  import AdvancedSettings from './sections/AdvancedSettings.svelte';
</script>

<section class="settings-section">
  {#if settings.state.allow_public_endpoint}
    <div class="public-endpoint-banner" role="alert">
      ⚠ <strong>Public endpoints enabled.</strong> AI / STT requests may leave your device.
    </div>
  {/if}

  <GeneralBasics />
  <DatabaseSecurity />
  <DataManagement />
  <AdvancedSettings />
</section>

<style>
  .public-endpoint-banner {
    background: #fef2f2;
    color: #991b1b;
    border: 1px solid #fca5a5;
    border-radius: 4px;
    padding: 8px 12px;
    margin-bottom: 12px;
    font-size: 0.9rem;
  }
</style>
