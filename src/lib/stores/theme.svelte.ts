type Theme = 'light' | 'dark';

class ThemeStore {
  current = $state<Theme>('dark');

  set(value: Theme) {
    if (typeof document !== 'undefined') {
      document.documentElement.setAttribute('data-theme', value);
    }
    this.current = value;
  }

  toggle() {
    const next: Theme = this.current === 'dark' ? 'light' : 'dark';
    this.set(next);
  }
}

export const theme = new ThemeStore();
