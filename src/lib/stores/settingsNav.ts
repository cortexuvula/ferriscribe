import { writable } from 'svelte/store';

export type SettingsSection = 'general' | 'prompts' | 'models' | 'audio' | 'sharing' | 'training-corpus';

interface SettingsNavState {
  /** When non-null the Settings dialog should open and navigate to this section. */
  requestedSection: SettingsSection | null;
}

function createSettingsNavStore() {
  const { subscribe, set } = writable<SettingsNavState>({ requestedSection: null });

  return {
    subscribe,

    /** Request that the Settings dialog open and navigate to `section`. */
    navigateTo(section: SettingsSection): void {
      set({ requestedSection: section });
    },

    /** Called by SettingsContent once it has consumed the navigation request. */
    clear(): void {
      set({ requestedSection: null });
    },
  };
}

export const settingsNav = createSettingsNavStore();
