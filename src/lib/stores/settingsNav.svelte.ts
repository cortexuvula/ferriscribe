export type SettingsSection = 'general' | 'prompts' | 'models' | 'audio' | 'backup' | 'sharing' | 'training-corpus' | 'letter-audiences' | 'about' | 'vocabulary' | 'advanced';

export const settingsNavGroups: { label: string; items: { id: SettingsSection; label: string }[] }[] = [
  { label: 'Everyday', items: [
    { id: 'general', label: 'General' },
    { id: 'audio', label: 'Recording & Transcription' },
    { id: 'models', label: 'AI Models' },
  ] },
  { label: 'Clinical content', items: [
    { id: 'prompts', label: 'Prompts & Specialties' },
    { id: 'vocabulary', label: 'Vocabulary & Templates' },
    { id: 'letter-audiences', label: 'Letter Audiences' },
  ] },
  { label: 'System', items: [
    { id: 'backup', label: 'Storage & Backup' },
    { id: 'sharing', label: 'Sharing' },
    { id: 'advanced', label: 'Advanced' },
    { id: 'about', label: 'About & Updates' },
  ] },
];

interface SettingsNavState {
  /** When non-null the Settings dialog should open and navigate here. */
  requestedSection: SettingsSection | null;
  /** Distinguishes even repeated requests to the same destination. */
  requestId: number;
  /** Survives dialog unmount; updated only after successful navigation. */
  lastSection: SettingsSection;
}

class SettingsNavStore {
  state = $state<SettingsNavState>({ requestedSection: null, requestId: 0, lastSection: 'general' });

  navigateTo(section: SettingsSection): void {
    this.state = { ...this.state, requestedSection: section, requestId: this.state.requestId + 1 };
  }

  /** Async consumers must only clear the exact request they handled. */
  clear(requestId?: number): void {
    if (requestId !== undefined && requestId !== this.state.requestId) return;
    this.state = { ...this.state, requestedSection: null };
  }
}

export const settingsNav = new SettingsNavStore();
