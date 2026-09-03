<script lang="ts">
  import { untrack } from 'svelte';
  import General from './settings/General.svelte';
  import Prompts from './settings/Prompts.svelte';
  import Models from './settings/Models.svelte';
  import Audio from './settings/Audio.svelte';
  import Backup from './settings/Backup.svelte';
  import Sharing from './settings/Sharing.svelte';
  import TrainingCorpus from './settings/TrainingCorpus.svelte';
  import LetterAudiences from './settings/LetterAudiences.svelte';
  import About from './settings/About.svelte';
  import { settingsNav, type SettingsSection } from '../stores/settingsNav.svelte';

  type Section = SettingsSection;
  let activeSection = $state<Section>('general');
  // Instance of the Prompts pane — the only settings pane with unsaved
  // draft state — so section switches away from it can run the discard
  // guard. Null whenever the pane isn't mounted.
  let prompts: Prompts | null = $state(null);

  /**
   * Guard for leaving settings with unsaved draft edits (currently only the
   * Prompts editor). Called by the section nav here and by the settings
   * dialog's close path (×, Escape, backdrop). Returns true when it is safe
   * to leave.
   */
  export function confirmDiscardEdits(): boolean {
    if (activeSection === 'prompts') {
      return prompts?.confirmDiscard() ?? true;
    }
    return true;
  }

  // Consume navigation requests from settingsNav store (e.g. from the
  // EndpointOfflineDialog "Open Settings" button). The write-back
  // (settingsNav.clear()) is untracked so this effect doesn't re-trigger
  // itself by reading and writing the same reactive source.
  $effect(() => {
    if (settingsNav.state.requestedSection) {
      activeSection = settingsNav.state.requestedSection;
      untrack(() => settingsNav.clear());
    }
  });

  const navItems: { id: Section; label: string }[] = [
    { id: 'general', label: 'General' },
    { id: 'prompts', label: 'Prompts' },
    { id: 'models', label: 'AI Models' },
    { id: 'audio', label: 'Audio / STT' },
    { id: 'backup', label: 'Backup' },
    { id: 'sharing', label: 'Sharing' },
    { id: 'training-corpus', label: 'Training Corpus' },
    { id: 'letter-audiences', label: 'Letter Audiences' },
    { id: 'about', label: 'About' },
  ];
</script>

<div class="settings-layout">
  <nav class="settings-nav">
    {#each navItems as item (item.id)}
      <button
        class="nav-item"
        class:active={activeSection === item.id}
        aria-current={activeSection === item.id ? 'true' : undefined}
        onclick={() => {
          if (activeSection !== item.id && confirmDiscardEdits()) {
            activeSection = item.id;
          }
        }}
      >
        {item.label}
      </button>
    {/each}
  </nav>

  <div class="settings-content">
    {#if activeSection === 'general'}
      <General />

    {:else if activeSection === 'prompts'}
      <Prompts bind:this={prompts} />

    {:else if activeSection === 'models'}
      <Models />

    {:else if activeSection === 'audio'}
      <Audio />

    {:else if activeSection === 'backup'}
      <Backup />

    {:else if activeSection === 'sharing'}
      <Sharing />

    {:else if activeSection === 'training-corpus'}
      <TrainingCorpus />

    {:else if activeSection === 'letter-audiences'}
      <LetterAudiences />

    {:else if activeSection === 'about'}
      <About />
    {/if}
  </div>
</div>

<style>
  .settings-layout {
    display: flex;
    height: 100%;
    min-height: 400px;
  }

  .settings-nav {
    width: 130px;
    flex-shrink: 0;
    background-color: var(--bg-secondary);
    border-right: 1px solid var(--border);
    padding: 8px 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .nav-item {
    width: 100%;
    text-align: left;
    padding: 8px 14px;
    font-size: 13px;
    color: var(--text-secondary);
    border-radius: 0;
    transition: background-color 0.15s ease, color 0.15s ease;
  }

  .nav-item:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .nav-item.active {
    background-color: var(--bg-active);
    color: var(--accent);
    font-weight: 500;
  }

  .settings-content {
    flex: 1;
    overflow-y: auto;
    padding: 20px;
  }
</style>
