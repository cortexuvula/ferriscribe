<script lang="ts">
  import Modal from '../components/Modal.svelte';
  import SettingsContent from '../components/SettingsContent.svelte';

  interface Props {
    open: boolean;
  }

  let { open = $bindable() }: Props = $props();

  let content: SettingsContent | null = $state(null);

  // Close paths (×, Escape, backdrop) all funnel through here — veto the
  // close while the Prompts editor has unsaved draft edits.
  function handleClose() {
    if (content?.confirmDiscardEdits() ?? true) {
      open = false;
    }
  }
</script>

<Modal {open} title="Settings" onClose={handleClose}>
  <SettingsContent bind:this={content} />
</Modal>
