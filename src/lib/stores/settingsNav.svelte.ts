export type SettingsSection = 'general' | 'prompts' | 'models' | 'audio' | 'backup' | 'sharing' | 'training-corpus' | 'letter-audiences' | 'about';

interface SettingsNavState {
  /** When non-null the Settings dialog should open and navigate to this section. */
  requestedSection: SettingsSection | null;
}

class SettingsNavStore {
  state = $state<SettingsNavState>({ requestedSection: null });

  /** Request that the Settings dialog open and navigate to `section`. */
  navigateTo(section: SettingsSection): void {
    this.state = { requestedSection: section };
  }

  /** Called by SettingsContent once it has consumed the navigation request. */
  clear(): void {
    this.state = { requestedSection: null };
  }
}

export const settingsNav = new SettingsNavStore();
