<script lang="ts">
  import StepWelcome from './onboarding/StepWelcome.svelte';
  import StepMode from './onboarding/StepMode.svelte';
  import StepProvider from './onboarding/StepProvider.svelte';
  import StepModel from './onboarding/StepModel.svelte';
  import StepPair from './onboarding/StepPair.svelte';
  import StepFolder from './onboarding/StepFolder.svelte';
  import { settings } from '../stores/settings.svelte';

  /// Linear step index. The mode step (1) sets `mode`, which determines
  /// whether steps 2/3 render the local-path (Provider → Model) or the
  /// server-path (Pair) bodies. Step 4 (Folder + Done) is shared.
  ///   0 = Welcome
  ///   1 = Mode (branch point)
  ///   2 = Provider (local) / Pair (server)
  ///   3 = Model (local) / Folder+Done (server — skips ahead)
  ///   4 = Folder+Done (local)
  type Mode = 'local' | 'server' | null;
  let step = $state(0);
  let mode = $state<Mode>(null);

  // The server path is shorter (no provider/whisper steps), so its "done"
  // step index differs from the local path's. Compute labels from the
  // effective sequence so the progress indicator stays honest.
  const localSteps = ['Welcome', 'Setup mode', 'AI provider', 'Whisper model', 'Recordings'];
  const serverSteps = ['Welcome', 'Setup mode', 'Connect to server', 'Recordings'];
  const stepLabels = $derived(mode === 'server' ? serverSteps : localSteps);
  // For the progress bar we show the current step's label; the welcome step
  // and mode step are shared. We render dots for the *current path* once mode
  // is chosen; before that (welcome) we show a single dot.
  const totalSteps = $derived(stepLabels.length);

  function next() {
    if (step < totalSteps - 1) step += 1;
  }
  function skip() {
    next();
  }
  function chooseMode(m: 'local' | 'server') {
    mode = m;
    next();
  }
  async function finish() {
    await settings.updateField('onboarding_completed', true);
  }
</script>

<div class="onboarding-overlay">
  <div class="onboarding-card">
    <div class="progress" aria-hidden="true">
      {#if mode !== null}
        {#each Array(totalSteps) as _, i (i)}
          <span class="dot" class:active={i === step} class:done={i < step}></span>
        {/each}
      {:else}
        <span class="dot active"></span>
      {/if}
    </div>

    <div class="step-body">
      {#if step === 0}
        <StepWelcome onContinue={next} />
      {:else if step === 1}
        <StepMode onChoose={chooseMode} onSkip={skip} />
      {:else if mode === 'local' && step === 2}
        <StepProvider onNext={next} onSkip={skip} />
      {:else if mode === 'local' && step === 3}
        <StepModel onNext={next} onSkip={skip} />
      {:else if mode === 'server' && step === 2}
        <StepPair onNext={next} onSkip={skip} />
      {:else if (mode === 'local' && step === 4) || (mode === 'server' && step === 3)}
        <StepFolder onDone={finish} />
      {/if}
    </div>
  </div>
</div>

<style>
  .onboarding-overlay {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    background-color: var(--bg-primary, #1a1a1a);
    padding: 24px;
  }
  .onboarding-card {
    width: 100%;
    max-width: 560px;
    max-height: 90vh;
    overflow-y: auto;
    background-color: var(--bg-card, #242424);
    border: 1px solid var(--border, #333);
    border-radius: var(--radius-lg, 12px);
    padding: 32px;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }
  .progress {
    display: flex;
    gap: 8px;
    justify-content: center;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background-color: var(--border-strong, #444);
    transition: background-color 0.15s ease, width 0.15s ease;
  }
  .dot.active {
    background-color: var(--accent, #3b82f6);
    width: 24px;
    border-radius: 4px;
  }
  .dot.done {
    background-color: var(--success, #22c55e);
  }
  .step-body {
    flex: 1;
    min-height: 0;
  }
</style>
