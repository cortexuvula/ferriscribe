<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import General from './settings/General.svelte';
  import Prompts from './settings/Prompts.svelte';
  import Models from './settings/Models.svelte';
  import Audio from './settings/Audio.svelte';
  import StorageSettings from './settings/StorageSettings.svelte';
  import VocabularySettings from './settings/VocabularySettings.svelte';
  import Advanced from './settings/Advanced.svelte';
  import Sharing from './settings/Sharing.svelte';
  import TrainingCorpus from './settings/TrainingCorpus.svelte';
  import LetterAudiences from './settings/LetterAudiences.svelte';
  import About from './settings/About.svelte';
  import Callout from './settings/Callout.svelte';
  import { settings } from '../stores/settings.svelte';
  import { settingsNav, settingsNavGroups, type SettingsSection } from '../stores/settingsNav.svelte';
  import './settings/settings.css';

  let activeSection = $state<SettingsSection>(settingsNav.state.lastSection);
  let prompts: Prompts | null = $state(null);
  let transitionId = 0;
  let mounted = true;
  let pendingDiscard: Promise<boolean> | null = null;

  // A single confirmation covers competing navigation/close intents. Only
  // the latest intent may commit; an obsolete async result cannot close the
  // dialog or overwrite a newer destination. Same-section requests are no-ops.
  async function transition(next: SettingsSection | null): Promise<boolean> {
    const id = ++transitionId;
    if (next === activeSection) return true;
    if (activeSection === 'prompts' && prompts) {
      const confirmation = pendingDiscard ??= prompts.confirmDiscard();
      let ok: boolean;
      try {
        ok = await confirmation;
      } finally {
        if (pendingDiscard === confirmation) pendingDiscard = null;
      }
      if (!ok) return false;
    }
    if (!mounted || id !== transitionId) return false;
    if (next !== null) {
      activeSection = next;
      settingsNav.state.lastSection = next;
    }
    return true;
  }

  /** Close, Escape and backdrop use the same guard as all navigation. */
  export function confirmDiscardEdits(): Promise<boolean> {
    return transition(null);
  }

  function switchSection(next: SettingsSection) {
    void transition(next);
  }

  $effect(() => {
    const section = settingsNav.state.requestedSection;
    const requestId = settingsNav.state.requestId;
    if (section) {
      untrack(() => {
        void transition(section).finally(() => settingsNav.clear(requestId));
      });
    }
  });

  onDestroy(() => {
    mounted = false;
    transitionId++;
  });
</script>

<div class="settings-layout">
  <nav class="settings-nav" aria-label="Settings sections">
    {#each settingsNavGroups as group (group.label)}
      <div class="nav-group">
        <h2 class="nav-group-title">{group.label}</h2>
        {#each group.items as item (item.id)}
          <button
            class="nav-item"
            class:active={activeSection === item.id || (item.id === 'advanced' && activeSection === 'training-corpus')}
            aria-current={activeSection === item.id || (item.id === 'advanced' && activeSection === 'training-corpus') ? 'true' : undefined}
            onclick={() => switchSection(item.id)}
          >{item.label}</button>
        {/each}
      </div>
    {/each}
  </nav>

  <div class="settings-main">
    {#if settings.saveError}
      <div class="settings-warning">
        <Callout kind="danger">{settings.saveError} Check the values before trying again. Unsaved prompt edits remain in the editor.</Callout>
      </div>
    {/if}
    {#if settings.state.allow_public_endpoint}
      <div class="settings-warning" role="status">
        <Callout kind="danger">⚠ <strong>Public endpoints enabled.</strong> AI / STT requests may leave your device.</Callout>
      </div>
    {/if}
    <div class="settings-content">
      <div class="settings-page">
        {#if activeSection === 'general' || activeSection === 'models'}
          <p class="section-desc">Changes apply immediately and are saved automatically. There is no Save button.</p>
        {:else if activeSection === 'prompts'}
          <p class="section-desc">Prompt edits are saved only when you select Save. Specialty selection applies immediately.</p>
        {/if}
        {#if activeSection === 'general'}
          <General />
        {:else if activeSection === 'prompts'}
          <Prompts bind:this={prompts} />
        {:else if activeSection === 'models'}
          <Models />
        {:else if activeSection === 'audio'}
          <Audio />
        {:else if activeSection === 'backup'}
          <StorageSettings />
        {:else if activeSection === 'vocabulary'}
          <VocabularySettings />
        {:else if activeSection === 'advanced'}
          <Advanced onNavigate={switchSection} />
        {:else if activeSection === 'sharing'}
          <Sharing />
        {:else if activeSection === 'training-corpus'}
          <button class="settings-back" onclick={() => switchSection('advanced')}>← Advanced</button>
          <TrainingCorpus />
        {:else if activeSection === 'letter-audiences'}
          <LetterAudiences />
        {:else if activeSection === 'about'}
          <About />
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .settings-layout { display: flex; height: 100%; min-height: 0; min-width: 0; overflow: hidden; }
  .settings-nav { width: 220px; flex-shrink: 0; background: var(--bg-secondary); border-right: 1px solid var(--border); padding: 16px 10px; overflow-y: auto; }
  .nav-group + .nav-group { margin-top: 16px; }
  .nav-group-title { margin: 0 10px 6px; font-size: 11px; font-weight: 600; color: var(--text-secondary); text-transform: uppercase; letter-spacing: 0.06em; }
  .nav-item { width: 100%; min-height: 44px; text-align: left; padding: 10px; font-size: 13px; line-height: 1.4; color: var(--text-secondary); border-radius: var(--radius-sm); }
  .nav-item:hover { background: var(--bg-hover); color: var(--text-primary); }
  .nav-item.active { background: var(--bg-active); color: var(--settings-link); font-weight: 600; }
  .settings-main { flex: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; }
  .settings-warning { flex-shrink: 0; padding: 12px 24px 0; }
  .settings-content { flex: 1; min-height: 0; overflow-y: auto; padding: 24px 32px; container-type: inline-size; }
  .settings-page { max-width: 840px; margin: 0 auto; min-width: 0; }
  .settings-back { min-height: 44px; color: var(--accent); margin-bottom: 12px; }
  @media (max-width: 700px) {
    .settings-nav { width: 200px; padding: 12px 6px; }
    .settings-content { padding: 24px; }
  }
  @media (max-width: 520px) {
    .settings-layout { flex-direction: column; }
    .settings-nav { width: 100%; max-height: 180px; border-right: 0; border-bottom: 1px solid var(--border); }
    .settings-content { padding: 20px; }
    .settings-warning { padding: 12px 20px 0; }
  }
</style>
