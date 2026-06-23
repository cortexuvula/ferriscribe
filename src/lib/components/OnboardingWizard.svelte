<script lang="ts">
  import StepWelcome from './onboarding/StepWelcome.svelte';
  import StepUpdates from './onboarding/StepUpdates.svelte';
  import StepLanguage from './onboarding/StepLanguage.svelte';
  import StepMode from './onboarding/StepMode.svelte';
  import StepProvider from './onboarding/StepProvider.svelte';
  import StepModel from './onboarding/StepModel.svelte';
  import StepPair from './onboarding/StepPair.svelte';
  import StepFolder from './onboarding/StepFolder.svelte';
  import { settings } from '../stores/settings.svelte';
  import { setOnboardingStarted } from '../api/settings';

  ///   0 = Welcome
  ///   1 = Updates (consent)
  ///   2 = Language
  ///   3 = Mode (branch point)
  ///   4 = Provider (local) / Pair (server)
  ///   5 = Model (local) / Folder+Done (server — skips ahead)
  ///   6 = Folder+Done (local)
  type Mode = 'local' | 'server' | null;
  let step = $state(0);
  let mode = $state<Mode>(null);

  const localSteps = ['Welcome', 'Updates', 'Language', 'Setup mode', 'AI provider', 'Whisper model', 'Recordings'];
  const serverSteps = ['Welcome', 'Updates', 'Language', 'Setup mode', 'Connect to server', 'Recordings'];
  const stepLabels = $derived(mode === 'server' ? serverSteps : localSteps);
  const totalSteps = $derived(stepLabels.length);

  function next() {
    if (step === 0) {
      // First transition past Welcome: stamp the onboarding_started sentinel so
      // an interrupted wizard reappears on next launch rather than being
      // silently auto-marked complete (see get_settings auto-mark logic).
      // Fire-and-forget; idempotent.
      void setOnboardingStarted();
    }
    if (step < totalSteps - 1) step += 1;
  }
  function skip() {
    // Skipping the Mode step without choosing would leave the next step with
    // mode === null and no render branch matches → blank card, stuck.
    // Default to local so the rest of the wizard renders; the user can
    // still skip every subsequent step.
    if (step === 3 && mode === null) {
      mode = 'local';
    }
    next();
  }
  function chooseMode(m: 'local' | 'server') {
    mode = m;
    next();
  }
  let finishing = $state(false);
  let finishError = $state<string | null>(null);
  async function finish() {
    if (finishing) return;
    finishing = true;
    finishError = null;
    try {
      await settings.updateField('onboarding_completed', true);
    } catch (e) {
      finishError = e instanceof Error ? e.message : String(e);
      finishing = false; // allow retry
    }
    // On success, the reactive gate in App.svelte unmounts this component,
    // so there's no `finishing = false` to set in the happy path.
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
        <StepUpdates onNext={next} onSkip={skip} />
      {:else if step === 2}
        <StepLanguage onNext={next} onSkip={skip} />
      {:else if step === 3}
        <StepMode onChoose={chooseMode} onSkip={skip} />
      {:else if mode === 'local' && step === 4}
        <StepProvider onNext={next} onSkip={skip} />
      {:else if mode === 'local' && step === 5}
        <StepModel onNext={next} onSkip={skip} />
      {:else if mode === 'server' && step === 4}
        <StepPair onNext={next} onSkip={skip} />
      {:else if (mode === 'local' && step === 6) || (mode === 'server' && step === 5)}
        <StepFolder onDone={finish} {finishing} {finishError} />
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
