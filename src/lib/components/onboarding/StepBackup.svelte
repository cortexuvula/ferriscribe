<script lang="ts">
  import BackupWizard from '../settings/BackupWizard.svelte';

  interface Props {
    onDone: () => void;
    onSetUp: () => void;
    finishing?: boolean;
    finishError?: string | null;
  }
  // onSetUp stays in Props so OnboardingWizard's wiring compiles; the
  // wizard now runs inline instead of deep-linking to Settings.
  const { onDone, onSetUp: _onSetUp, finishing = false, finishError = null }: Props = $props();
</script>

<h2>One last thing: protect your data</h2>
<p class="hint">
  Set up a backup now — it takes about two minutes with a USB drive or a folder that syncs to
  the cloud. Everything stays encrypted, and no patient-identifying filenames ever leave this
  machine.
</p>

{#if finishError}
  <p class="error" role="alert">{finishError}</p>
{/if}

<div class="wizard-holder">
  <BackupWizard embedded onDone={onDone} />
</div>

<div class="actions">
  <button class="btn-secondary" onclick={onDone} disabled={finishing}>
    {finishing ? 'Finishing…' : 'Skip for now'}
  </button>
</div>

<style>
  .hint {
    color: var(--text-secondary);
    font-size: 0.95rem;
    line-height: 1.6;
    margin: 0 0 12px;
  }
  .wizard-holder {
    max-height: 55vh;
    overflow-y: auto;
    padding-right: 4px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    margin-top: 14px;
  }
  .btn-secondary {
    padding: 10px 18px;
    background: transparent;
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
    font-size: 14px;
  }
  .btn-secondary:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .error {
    color: var(--danger, #ef4444);
    font-size: 0.85rem;
  }
</style>
