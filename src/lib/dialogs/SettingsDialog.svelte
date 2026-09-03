<script lang="ts">
  import Modal from '../components/Modal.svelte';
  import SettingsContent from '../components/SettingsContent.svelte';

  interface Props {
    open: boolean;
  }

  let { open = $bindable() }: Props = $props();

  let content: SettingsContent | null = $state(null);

  // Close paths (×, Escape, backdrop) all funnel through here — veto the
  // close while the Prompts editor has unsaved draft edits. Async: the
  // discard guard may show a styled confirm dialog first; Modal's onClose
  // accepts it (Promise<void> is assignable to its () => void contract).
  async function handleClose() {
    if ((await content?.confirmDiscardEdits()) ?? true) {
      open = false;
    }
  }
</script>

<Modal {open} title="Settings" onClose={handleClose}>
  <SettingsContent bind:this={content} />
</Modal>
