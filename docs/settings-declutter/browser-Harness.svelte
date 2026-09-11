<script lang="ts">
import SettingsDialog from '../../src/lib/dialogs/SettingsDialog.svelte';
import ToastContainer from '../../src/lib/components/ToastContainer.svelte';
import ConfirmHost from '../../src/lib/components/ConfirmHost.svelte';
let open = $state(true);
</script>
<div class="fixture-label">SYNTHETIC FIXTURE · production SettingsDialog · all saves simulated in memory</div>
<button onclick={() => open = true}>Reopen fixture settings</button>
<SettingsDialog bind:open />
<ConfirmHost />
<ToastContainer />
<style>.fixture-label { pointer-events: none; position: fixed; top: 4px; left: 8px; font: 11px system-ui; z-index: 3000; color: var(--text-primary); background: var(--bg-primary); }</style>
