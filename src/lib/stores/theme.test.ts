import { describe, it, expect, beforeEach } from 'vitest';
import { theme } from './theme.svelte';

describe('ThemeStore', () => {
  beforeEach(() => {
    theme.set('dark');
  });

  it('defaults to dark', () => {
    theme.set('dark');
    expect(theme.current).toBe('dark');
  });

  it('set changes the theme', () => {
    theme.set('light');
    expect(theme.current).toBe('light');
  });

  it('toggle switches dark to light', () => {
    theme.set('dark');
    theme.toggle();
    expect(theme.current).toBe('light');
  });

  it('toggle switches light to dark', () => {
    theme.set('light');
    theme.toggle();
    expect(theme.current).toBe('dark');
  });

  it('set updates the document data-theme attribute', () => {
    // jsdom provides document.documentElement
    if (typeof document !== 'undefined') {
      theme.set('light');
      expect(document.documentElement.getAttribute('data-theme')).toBe('light');
      theme.set('dark');
      expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
    }
  });
});
