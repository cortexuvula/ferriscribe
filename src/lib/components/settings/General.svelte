<script lang="ts">
  import { settings } from '../../stores/settings.svelte';
  import GeneralBasics from './sections/GeneralBasics.svelte';
  import DatabaseSecurity from './sections/DatabaseSecurity.svelte';
  import DataManagement from './sections/DataManagement.svelte';
  import AdvancedSettings from './sections/AdvancedSettings.svelte';
  import Callout from './Callout.svelte';
</script>

<section class="settings-section">
  {#if settings.state.allow_public_endpoint}
    <div class="banner-slot">
      <Callout kind="danger">
        ⚠ <strong>Public endpoints enabled.</strong> AI / STT requests may leave your device.
      </Callout>
    </div>
  {/if}

  <GeneralBasics />
  <DatabaseSecurity />
  <DataManagement />
  <AdvancedSettings />
</section>

<style>
  .banner-slot {
    margin-bottom: 12px;
  }
</style>
