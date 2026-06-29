import { describe, it, expect, beforeEach } from 'vitest';
import { settingsNav } from './settingsNav.svelte';

describe('SettingsNavStore', () => {
  beforeEach(() => {
    settingsNav.clear();
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
});
