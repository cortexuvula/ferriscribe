import { describe, it, expect, beforeEach } from 'vitest';
import { settingsNav } from './settingsNav.svelte';

describe('SettingsNavStore', () => {
  beforeEach(() => {
    settingsNav.clear();
    settingsNav.state.lastSection = 'general';
  });

  it('starts with no requested section', () => {
    expect(settingsNav.state.requestedSection).toBeNull();
  });

  it('navigateTo sets the requested section', () => {
    settingsNav.navigateTo('models');
    expect(settingsNav.state.requestedSection).toBe('models');
  });

  it('clear resets to null', () => {
    settingsNav.navigateTo('about');
    settingsNav.clear();
    expect(settingsNav.state.requestedSection).toBeNull();
  });

  it('lastSection survives a clear (dialog close) so the dialog reopens there', () => {
    settingsNav.state.lastSection = 'audio';
    settingsNav.navigateTo('prompts');
    settingsNav.clear();
    expect(settingsNav.state.requestedSection).toBeNull();
    expect(settingsNav.state.lastSection).toBe('audio');
  });
});
